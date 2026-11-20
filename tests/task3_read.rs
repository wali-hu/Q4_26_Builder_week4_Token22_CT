//! Task 3: state is read through StateWithExtensions, never raw unpack.
mod common;

use {
    common::world,
    solana_program_pack::Pack,
    spl_token_2022_interface::{
        extension::{ExtensionType, StateWithExtensions},
        state::{Account, Mint},
    },
    token22_ct::read::{read_account, read_mint},
};

#[test]
fn task3_raw_unpack_fails_on_a_token2022_mint_but_state_with_extensions_works() {
    let w = world();
    let data = w.env.data(&w.mint_key());
    assert!(data.len() > Mint::LEN, "{} bytes, not 82", data.len());
    // Raw Pack::unpack expects exactly 82 bytes, so it rejects a mint with extensions.
    assert!(Mint::unpack(&data).is_err());
    // StateWithExtensions reads the base and the TLV extensions after it.
    let state = StateWithExtensions::<Mint>::unpack(&data).unwrap();
    assert_eq!(state.base.decimals, 6);
}

#[test]
fn task3_account_view_lists_extensions_added_by_the_ata_program() {
    let mut w = world();
    let (_u, acct) = w.kyc_user();
    let data = w.env.data(&acct);
    assert!(Account::unpack(&data).is_err(), "raw unpack fails here too");
    let a = read_account(&w.env, &acct).unwrap();
    // A token account for a fee mint carries TransferFeeAmount; an ATA is also ImmutableOwner.
    assert!(a.extensions.contains(&ExtensionType::TransferFeeAmount));
    assert!(a.extensions.contains(&ExtensionType::ImmutableOwner));
    println!("token account: {} bytes, extensions {:?}", a.data_len, a.extensions);
    let m = read_mint(&w.env, &w.mint_key()).unwrap();
    println!("mint: {} bytes, extensions {:?}", m.data_len, m.extensions);
}
