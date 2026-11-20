//! Task 7: the permanent delegate seizes public balances.
//! Task 8 (written finding, tested): a sanctioned user who moves funds into the
//! confidential balance first is out of the delegate's reach; freezing only locks them.
mod common;

use {
    common::ct_world,
    solana_signer::Signer,
    spl_token_2022_interface::extension::transfer_fee::instruction as fee_ix,
    spl_token_2022_interface::instruction as token_ix,
    token22_ct::{
        confidential::{apply_pending, available, deposit, pending, transfer_with_fee, withdraw},
        env::assert_custom_error,
        read::read_account,
        stablecoin::{balance, freeze, token_program, transfer_with_fee as public_transfer, DECIMALS, ONE},
    },
};

#[test]
fn task7_permanent_delegate_seizes_a_public_balance() {
    let mut c = ct_world();
    let mint = c.w.mint_key();
    let sanctioned = c.ct_user(100 * ONE);
    let (_t, treasury) = c.w.kyc_user();
    let seizer = c.seizer.insecure_clone();
    // The delegate signs instead of the owner; the owner signs nothing.
    let fee = public_transfer(&mut c.w.env, &mint, &sanctioned.account, &treasury, &seizer, 100 * ONE).unwrap();
    assert_eq!(balance(&c.w.env, &sanctioned.account), 0);
    assert_eq!(balance(&c.w.env, &treasury), 100 * ONE - fee);
}

#[test]
fn task7_delegate_cannot_move_from_a_frozen_account_unless_thaw_and_refreeze_in_one_tx() {
    let mut c = ct_world();
    let mint = c.w.mint_key();
    let sanctioned = c.ct_user(100 * ONE);
    let (_t, treasury) = c.w.kyc_user();
    let issuer = c.w.issuer.insecure_clone();
    let seizer = c.seizer.insecure_clone();
    freeze(&mut c.w.env, &issuer, &sanctioned.account, &mint).unwrap();
    // AccountFrozen = Custom(17): the frozen check runs before the delegate check.
    assert_custom_error(public_transfer(&mut c.w.env, &mint, &sanctioned.account, &treasury, &seizer, 100 * ONE), 17);

    // Atomic seizure: thaw, delegate transfer, freeze again, in ONE transaction.
    let p = token_program();
    let ixs = vec![
        token_ix::thaw_account(&p, &sanctioned.account, &mint, &issuer.pubkey(), &[]).unwrap(),
        fee_ix::transfer_checked_with_fee(&p, &sanctioned.account, &mint, &treasury, &seizer.pubkey(), &[], 100 * ONE, DECIMALS, 500_000).unwrap(),
        token_ix::freeze_account(&p, &sanctioned.account, &mint, &issuer.pubkey(), &[]).unwrap(),
    ];
    c.w.env.send(&ixs, &issuer, &[&seizer]).unwrap();
    let a = read_account(&c.w.env, &sanctioned.account).unwrap();
    assert_eq!((a.amount, a.frozen), (0, true));
}

#[test]
fn task8_funds_moved_into_the_confidential_balance_escape_the_permanent_delegate() {
    let mut c = ct_world();
    let mint = c.w.mint_key();
    let sanctioned = c.ct_user(100 * ONE);
    let (_t, treasury) = c.w.kyc_user();
    let seizer = c.seizer.insecure_clone();

    // The user sees the sanction coming and moves everything into the confidential balance.
    deposit(&mut c.w.env, &sanctioned, &mint, 100 * ONE).unwrap();
    apply_pending(&mut c.w.env, &sanctioned).unwrap();
    assert_eq!(balance(&c.w.env, &sanctioned.account), 0);

    // 1. The delegate's public transfer finds nothing: InsufficientFunds = Custom(1).
    assert_custom_error(public_transfer(&mut c.w.env, &mint, &sanctioned.account, &treasury, &seizer, ONE), 1);

    // 2. Even with VALID proofs (built here with the user's own keys, which a real issuer
    //    would not have), the delegate cannot withdraw from the confidential balance:
    //    Withdraw only accepts the account owner. OwnerMismatch = Custom(4).
    assert_custom_error(withdraw(&mut c.w.env, &sanctioned, &mint, ONE, &seizer), 4);
    assert_eq!(available(&c.w.env, &sanctioned), 100 * ONE, "the encrypted balance is untouched");
}

#[test]
fn task8_freezing_locks_the_confidential_balance_but_does_not_seize_it() {
    let mut c = ct_world();
    let mint = c.w.mint_key();
    let sanctioned = c.ct_user(100 * ONE);
    let friend = c.ct_user(0);
    let issuer = c.w.issuer.insecure_clone();
    deposit(&mut c.w.env, &sanctioned, &mint, 100 * ONE).unwrap();
    apply_pending(&mut c.w.env, &sanctioned).unwrap();

    freeze(&mut c.w.env, &issuer, &sanctioned.account, &mint).unwrap();
    let owner = sanctioned.owner.insecure_clone();
    // The owner can no longer move it out: AccountFrozen = Custom(17) for withdraw and transfer.
    assert_custom_error(withdraw(&mut c.w.env, &sanctioned, &mint, ONE, &owner), 17);
    let (aud, wh) = (*c.auditor.pubkey(), *c.withheld.pubkey());
    let res = transfer_with_fee(&mut c.w.env, &sanctioned, &friend, &mint, ONE, Some(&aud), &wh).map(|_| ());
    assert_custom_error(res, 17);
    // The value is still there, encrypted; the issuer holds a frozen balance, not the funds.
    assert_eq!(available(&c.w.env, &sanctioned), 100 * ONE);
    assert_eq!(pending(&c.w.env, &friend), 0);
}
