//! Task 4: new accounts are frozen by default; the freeze authority thaws one account
//! after KYC. Thawing one account does not change the mint's default state.
mod common;

use {
    common::world,
    solana_signer::Signer,
    spl_token_2022_interface::state::AccountState,
    token22_ct::{
        env::assert_custom_error,
        read::{read_account, read_mint},
        stablecoin::{freeze, kyc_thaw, mint_to, transfer_with_fee, ONE},
    },
};

#[test]
fn task4_new_accounts_start_frozen() {
    let mut w = world();
    let (_u, acct) = w.frozen_user();
    assert!(read_account(&w.env, &acct).unwrap().frozen);
}

#[test]
fn task4_frozen_account_cannot_receive_fails_account_frozen() {
    let mut w = world();
    let mint = w.mint_key();
    let issuer = w.issuer.insecure_clone();
    let (alice, a) = w.kyc_user();
    let (_bob, b) = w.frozen_user();
    // AccountFrozen = Custom(17), for minting into it and for transfers into it.
    assert_custom_error(mint_to(&mut w.env, &issuer, &mint, &b, ONE), 17);
    mint_to(&mut w.env, &issuer, &mint, &a, 10 * ONE).unwrap();
    assert_custom_error(transfer_with_fee(&mut w.env, &mint, &a, &b, &alice, ONE), 17);
}

#[test]
fn task4_frozen_account_cannot_send_fails_account_frozen() {
    let mut w = world();
    let mint = w.mint_key();
    let issuer = w.issuer.insecure_clone();
    let (alice, a) = w.kyc_user();
    let (_bob, b) = w.kyc_user();
    mint_to(&mut w.env, &issuer, &mint, &a, 10 * ONE).unwrap();
    freeze(&mut w.env, &issuer, &a, &mint).unwrap(); // e.g. KYC revoked
    assert_custom_error(transfer_with_fee(&mut w.env, &mint, &a, &b, &alice, ONE), 17);
}

#[test]
fn task4_kyc_thaw_unfreezes_one_account_and_keeps_the_default_frozen() {
    let mut w = world();
    let mint = w.mint_key();
    let issuer = w.issuer.insecure_clone();
    let (_alice, a) = w.frozen_user();
    kyc_thaw(&mut w.env, &issuer, &a, &mint).unwrap();
    assert!(!read_account(&w.env, &a).unwrap().frozen, "alice passed KYC");

    // The mint-level default is untouched: the next new account is frozen again.
    assert_eq!(read_mint(&w.env, &mint).unwrap().default_state, Some(AccountState::Frozen));
    let (_carol, c) = w.frozen_user();
    assert!(read_account(&w.env, &c).unwrap().frozen);
}

#[test]
fn task4_only_the_freeze_authority_can_thaw_fails_owner_mismatch() {
    let mut w = world();
    let mint = w.mint_key();
    let (bob, b) = w.frozen_user();
    // Bob tries to thaw his own account: OwnerMismatch = Custom(4)
    assert_custom_error(kyc_thaw(&mut w.env, &bob, &b, &mint), 4);
    let _ = bob.pubkey();
}
