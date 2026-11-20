//! Task 2: transfers use transfer_checked_with_fee, and the expected fee comes from
//! TransferFeeConfig::calculate_epoch_fee(current_epoch, amount) read from the live mint.
//! Also: withheld fees and how the issuer collects them (harvest, then withdraw).
mod common;

use {
    common::world,
    solana_signer::Signer,
    token22_ct::{
        env::assert_custom_error,
        read::{mint_withheld, read_account},
        stablecoin::{
            balance, harvest_to_mint, mint_to, set_fee, transfer_with_fee, transfer_with_given_fee,
            withdraw_fees_from_mint, ONE,
        },
    },
};

#[test]
fn task2_transfer_charges_the_epoch_fee_and_withholds_it_at_the_destination() {
    let mut w = world();
    let mint = w.mint_key();
    let (alice, a) = w.kyc_user();
    let (_bob, b) = w.kyc_user();
    let issuer = w.issuer.insecure_clone();
    mint_to(&mut w.env, &issuer, &mint, &a, 1_000 * ONE).unwrap();

    // 100 rUSD at 50 bps: fee = ceil(100_000_000 * 50 / 10_000) = 500_000 (0.5 rUSD)
    let fee = transfer_with_fee(&mut w.env, &mint, &a, &b, &alice, 100 * ONE).unwrap();
    assert_eq!(fee, 500_000);
    assert_eq!(balance(&w.env, &a), 900 * ONE, "source pays the full amount");
    assert_eq!(balance(&w.env, &b), 100 * ONE - 500_000, "destination receives amount - fee");
    assert_eq!(read_account(&w.env, &b).unwrap().withheld_fee, 500_000, "fee withheld in the destination");
}

#[test]
fn task2_fee_is_capped_at_the_maximum() {
    let mut w = world();
    let mint = w.mint_key();
    let (alice, a) = w.kyc_user();
    let (_bob, b) = w.kyc_user();
    let issuer = w.issuer.insecure_clone();
    mint_to(&mut w.env, &issuer, &mint, &a, 10_000 * ONE).unwrap();
    // 0.5% of 10,000 = 50 rUSD, but the cap is 5 rUSD
    let fee = transfer_with_fee(&mut w.env, &mint, &a, &b, &alice, 10_000 * ONE).unwrap();
    assert_eq!(fee, 5 * ONE);
}

#[test]
fn task2_wrong_fee_fails_fee_mismatch() {
    let mut w = world();
    let mint = w.mint_key();
    let (alice, a) = w.kyc_user();
    let (_bob, b) = w.kyc_user();
    let issuer = w.issuer.insecure_clone();
    mint_to(&mut w.env, &issuer, &mint, &a, 1_000 * ONE).unwrap();
    // FeeMismatch = Custom(32)
    assert_custom_error(transfer_with_given_fee(&mut w.env, &mint, &a, &b, &alice, 100 * ONE, 499_999), 32);
}

#[test]
fn task2_fee_change_applies_two_epochs_later_so_a_cached_rate_is_wrong() {
    let mut w = world();
    let mint = w.mint_key();
    let (alice, a) = w.kyc_user();
    let (_bob, b) = w.kyc_user();
    let issuer = w.issuer.insecure_clone();
    mint_to(&mut w.env, &issuer, &mint, &a, 1_000 * ONE).unwrap();

    let e = w.env.epoch();
    set_fee(&mut w.env, &issuer, &mint, 100, 5 * ONE).unwrap(); // 1%, from epoch e + 2

    // Same epoch: the live config still answers 50 bps. Using the "new" 1% rate fails.
    assert_custom_error(transfer_with_given_fee(&mut w.env, &mint, &a, &b, &alice, 100 * ONE, ONE), 32);
    assert_eq!(transfer_with_fee(&mut w.env, &mint, &a, &b, &alice, 100 * ONE).unwrap(), 500_000);

    // Two epochs later the same function, reading the live mint, charges 1%.
    w.env.set_epoch(e + 2);
    assert_eq!(transfer_with_fee(&mut w.env, &mint, &a, &b, &alice, 100 * ONE).unwrap(), ONE);
}

#[test]
fn task2_issuer_harvests_and_withdraws_the_withheld_fees() {
    let mut w = world();
    let mint = w.mint_key();
    let (alice, a) = w.kyc_user();
    let (_bob, b) = w.kyc_user();
    let (_carol, c) = w.kyc_user();
    let issuer = w.issuer.insecure_clone();
    mint_to(&mut w.env, &issuer, &mint, &a, 1_000 * ONE).unwrap();
    transfer_with_fee(&mut w.env, &mint, &a, &b, &alice, 100 * ONE).unwrap(); // 0.5 withheld at b
    transfer_with_fee(&mut w.env, &mint, &a, &c, &alice, 200 * ONE).unwrap(); // 1.0 withheld at c

    // Step 1, permissionless: anyone can move withheld fees from accounts into the mint.
    let anyone = w.env.wallet();
    harvest_to_mint(&mut w.env, &anyone, &mint, &[&b, &c]).unwrap();
    assert_eq!(read_account(&w.env, &b).unwrap().withheld_fee, 0);
    assert_eq!(mint_withheld(&w.env, &mint), 1_500_000);

    // Step 2: only the withdraw-withheld authority can take them out (issuer revenue).
    let (_treasury_owner, treasury) = w.kyc_user();
    assert_custom_error(withdraw_fees_from_mint(&mut w.env, &anyone, &mint, &treasury), 4); // OwnerMismatch
    withdraw_fees_from_mint(&mut w.env, &issuer, &mint, &treasury).unwrap();
    assert_eq!(balance(&w.env, &treasury), 1_500_000);
    assert_eq!(mint_withheld(&w.env, &mint), 0);
    let _ = alice.pubkey();
}
