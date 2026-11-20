//! Task 5: the re-issued mint (same extensions + PermanentDelegate + confidential
//! transfers with manual approval). Task 6: the full confidential lifecycle with real proofs.
mod common;

use {
    common::ct_world,
    solana_signer::Signer,
    solana_zk_sdk::encryption::elgamal::ElGamalCiphertext,
    spl_token_2022_interface::extension::{confidential_transfer::PENDING_BALANCE_LO_BIT_LENGTH, ExtensionType},
    token22_ct::{
        confidential::{
            add_ct_space, apply_pending, approve, available, configure, deposit, pending, transfer_with_fee,
            withdraw, withdraw_claiming, CtUser,
        },
        env::assert_custom_error,
        read::{read_account, read_mint},
        stablecoin::{balance, create_ata, kyc_thaw, ONE},
    },
};

#[test]
fn task5_reissued_mint_has_the_same_extensions_plus_permanent_delegate_and_manual_ct() {
    let c = ct_world();
    let m = read_mint(&c.w.env, &c.w.mint_key()).unwrap();
    for ext in [
        ExtensionType::TransferFeeConfig,
        ExtensionType::MetadataPointer,
        ExtensionType::DefaultAccountState,
        ExtensionType::MintCloseAuthority,
        ExtensionType::TokenMetadata,
        ExtensionType::PermanentDelegate,
        ExtensionType::ConfidentialTransferMint,
        ExtensionType::ConfidentialTransferFeeConfig,
    ] {
        assert!(m.extensions.contains(&ext), "missing {ext:?}");
    }
    assert_eq!(m.permanent_delegate, Some(c.seizer.pubkey()));
    assert_eq!(m.ct_auto_approve, Some(false), "approve policy = manual");
    println!(
        "re-issued mint: fixed {} + metadata {} = {} bytes",
        c.w.sizes.fixed_len, c.w.sizes.metadata_tlv_len, m.data_len
    );
}

#[test]
fn task6_configure_is_owner_only_while_ata_creation_is_open_to_anyone() {
    let mut c = ct_world();
    let mint = c.w.mint_key();
    let stranger = c.w.env.wallet();
    let victim = c.w.env.wallet();
    // Anyone can create (and pay for) someone else's ATA.
    let acct = create_ata(&mut c.w.env, &stranger, &victim.pubkey(), &mint).unwrap();
    let issuer = c.w.issuer.insecure_clone();
    kyc_thaw(&mut c.w.env, &issuer, &acct, &mint).unwrap();
    let user = CtUser::new(victim.insecure_clone(), acct);
    add_ct_space(&mut c.w.env, &user).unwrap();
    // ...but only the owner can configure it for confidential transfers: OwnerMismatch = Custom(4)
    assert_custom_error(configure(&mut c.w.env, &user, &mint, &stranger), 4);
    configure(&mut c.w.env, &user, &mint, &victim).unwrap();
    assert!(read_account(&c.w.env, &acct).unwrap().ct.is_some());
}

#[test]
fn task6_unapproved_account_cannot_deposit_fails_not_approved() {
    let mut c = ct_world();
    let mint = c.w.mint_key();
    let (owner, account) = c.w.kyc_user();
    let user = CtUser::new(owner, account);
    add_ct_space(&mut c.w.env, &user).unwrap();
    let o = user.owner.insecure_clone();
    configure(&mut c.w.env, &user, &mint, &o).unwrap();
    let issuer = c.w.issuer.insecure_clone();
    token22_ct::stablecoin::mint_to(&mut c.w.env, &issuer, &mint, &account, 10 * ONE).unwrap();
    // Manual policy: not approved yet. ConfidentialTransferAccountNotApproved = Custom(24)
    assert_custom_error(deposit(&mut c.w.env, &user, &mint, ONE), 24);
    // Only the confidential-transfer authority can approve: the owner is refused with
    // MissingRequiredSignature (the authority account passed is not the mint's CT authority).
    let err = approve(&mut c.w.env, &o, &account, &mint).expect_err("must fail");
    assert!(err.contains("MissingRequiredSignature"), "{err}");
    approve(&mut c.w.env, &issuer, &account, &mint).unwrap();
    deposit(&mut c.w.env, &user, &mint, ONE).unwrap();
}

#[test]
fn task6_withdraw_before_apply_pending_fails() {
    let mut c = ct_world();
    let mint = c.w.mint_key();
    let alice = c.ct_user(100 * ONE);
    deposit(&mut c.w.env, &alice, &mint, 100 * ONE).unwrap();
    assert_eq!((pending(&c.w.env, &alice), available(&c.w.env, &alice)), (100 * ONE, 0));
    let owner = alice.owner.insecure_clone();

    // Client side: no proof can show a remaining balance >= 0 when available is 0.
    let err = withdraw(&mut c.w.env, &alice, &mint, 50 * ONE, &owner).expect_err("must fail");
    assert!(err.contains("NotEnoughFunds"), "{err}");

    // On-chain: claim the pending 100 is available. The proof is built for a balance the
    // account does not have, so the ZK proof program rejects it (InvalidInstructionData).
    let err = withdraw_claiming(&mut c.w.env, &alice, &mint, 50 * ONE, 100 * ONE, &owner).expect_err("must fail");
    assert!(err.contains("InvalidInstructionData") && err.contains("proof_verification failed"), "{err}");

    // After ApplyPendingBalance the same withdraw works.
    apply_pending(&mut c.w.env, &alice).unwrap();
    withdraw(&mut c.w.env, &alice, &mint, 50 * ONE, &owner).unwrap();
    assert_eq!(available(&c.w.env, &alice), 50 * ONE);
}

#[test]
fn task6_full_confidential_lifecycle_with_fee_and_auditor() {
    let mut c = ct_world();
    let mint = c.w.mint_key();
    let alice = c.ct_user(1_000 * ONE);
    let bob = c.ct_user(0);

    // 1. Deposit: public -> pending
    deposit(&mut c.w.env, &alice, &mint, 500 * ONE).unwrap();
    assert_eq!(balance(&c.w.env, &alice.account), 500 * ONE, "public balance went down by 500");
    assert_eq!(pending(&c.w.env, &alice), 500 * ONE);
    assert_eq!(available(&c.w.env, &alice), 0);

    // 2. ApplyPendingBalance: pending -> available
    apply_pending(&mut c.w.env, &alice).unwrap();
    assert_eq!((pending(&c.w.env, &alice), available(&c.w.env, &alice)), (0, 500 * ONE));

    // 3. Confidential transfer of 200 with the 0.5% fee (1 rUSD), five proofs.
    let auditor = *c.auditor.pubkey();
    let withheld = *c.withheld.pubkey();
    let out = transfer_with_fee(&mut c.w.env, &alice, &bob, &mint, 200 * ONE, Some(&auditor), &withheld).unwrap();
    assert_eq!(out.fee, ONE);
    assert_eq!(available(&c.w.env, &alice), 300 * ONE);
    assert_eq!(pending(&c.w.env, &bob), 199 * ONE, "bob receives amount - fee in pending");
    // Public balances did not move: the amount is not visible in the account's public field.
    assert_eq!(balance(&c.w.env, &alice.account), 500 * ONE);
    assert_eq!(balance(&c.w.env, &bob.account), 0);

    // The auditor (and only someone with the auditor key) can read the amount: lo + hi << 16.
    let dec = |ct: &ElGamalCiphertext| c.auditor.secret().decrypt_u32(ct).unwrap();
    let seen = dec(&out.auditor_ciphertext_lo) + (dec(&out.auditor_ciphertext_hi) << PENDING_BALANCE_LO_BIT_LENGTH);
    assert_eq!(seen, 200 * ONE);

    // 4. Bob applies his pending balance, 5. then withdraws to public.
    apply_pending(&mut c.w.env, &bob).unwrap();
    assert_eq!(available(&c.w.env, &bob), 199 * ONE);
    let bob_owner = bob.owner.insecure_clone();
    withdraw(&mut c.w.env, &bob, &mint, 150 * ONE, &bob_owner).unwrap();
    assert_eq!(balance(&c.w.env, &bob.account), 150 * ONE);
    assert_eq!(available(&c.w.env, &bob), 49 * ONE);
}
