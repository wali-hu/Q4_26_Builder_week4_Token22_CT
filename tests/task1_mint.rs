//! Task 1: one transaction creates the mint with TransferFeeConfig, MetadataPointer
//! (pointing at the mint), DefaultAccountState(Frozen) and MintCloseAuthority, sized with
//! ExtensionType::try_calculate_account_len, every extension init before InitializeMint,
//! then the token metadata is written into the mint.
mod common;

use {
    common::world,
    solana_keypair::Keypair,
    solana_signer::Signer,
    spl_token_2022_interface::{
        extension::{transfer_fee::instruction as fee_ix, ExtensionType},
        instruction as token_ix,
        state::{AccountState, Mint},
    },
    token22_ct::{
        env::Env,
        read::read_mint,
        stablecoin::{mint_to, token_program, DECIMALS, ONE},
    },
};

#[test]
fn task1_mint_has_four_extensions_and_metadata_in_one_transaction() {
    let w = world();
    let m = read_mint(&w.env, &w.mint_key()).unwrap();
    let issuer = w.issuer.pubkey();

    for ext in [
        ExtensionType::TransferFeeConfig,
        ExtensionType::MetadataPointer,
        ExtensionType::DefaultAccountState,
        ExtensionType::MintCloseAuthority,
        ExtensionType::TokenMetadata,
    ] {
        assert!(m.extensions.contains(&ext), "missing {ext:?}");
    }
    assert_eq!(m.decimals, DECIMALS);
    assert_eq!(m.mint_authority, Some(issuer));
    assert_eq!(m.freeze_authority, Some(issuer), "default-frozen needs a freeze authority");
    let fee = m.transfer_fee_config.unwrap();
    assert_eq!(u16::from(fee.newer_transfer_fee.transfer_fee_basis_points), 50);
    assert_eq!(u64::from(fee.newer_transfer_fee.maximum_fee), 5 * ONE);
    assert_eq!(m.metadata_pointer_target, Some(w.mint_key()), "pointer targets the mint itself");
    assert_eq!(m.default_state, Some(AccountState::Frozen));
    assert_eq!(m.close_authority, Some(issuer));
    let md = m.metadata.unwrap();
    assert_eq!((md.name.as_str(), md.symbol.as_str()), ("Remit USD", "rUSD"));
    assert_eq!(md.mint, w.mint_key());

    // Sizes: CreateAccount allocated only the fixed part; the metadata init grew the
    // account by exactly the metadata TLV size, paid for by the lamports deposited up front.
    assert_eq!(m.data_len, w.sizes.fixed_len + w.sizes.metadata_tlv_len);
    assert_eq!(w.env.lamports(&w.mint_key()), w.env.rent(m.data_len));
    println!(
        "mint sizes: fixed {} bytes + metadata {} bytes = {} bytes, rent {} lamports (LiteSVM rate); tlv_size_of() said {}",
        w.sizes.fixed_len, w.sizes.metadata_tlv_len, m.data_len, w.sizes.lamports, w.sizes.generic_tlv_size_of
    );
    assert_eq!(w.sizes.generic_tlv_size_of, w.sizes.metadata_tlv_len + 8, "tlv_size_of over-counts by 8 bytes");
}

#[test]
fn task1_extension_init_after_initialize_mint_fails() {
    // Wrong order: InitializeMint2 first, then an extension init. Token-2022 refuses.
    let mut env = Env::new();
    let issuer = env.wallet();
    let mint = Keypair::new();
    let program = token_program();
    let len = ExtensionType::try_calculate_account_len::<Mint>(&[ExtensionType::TransferFeeConfig]).unwrap();
    let ixs = vec![
        solana_system_interface::instruction::create_account(&issuer.pubkey(), &mint.pubkey(), env.rent(len), len as u64, &program),
        token_ix::initialize_mint2(&program, &mint.pubkey(), &issuer.pubkey(), None, DECIMALS).unwrap(),
        fee_ix::initialize_transfer_fee_config(&program, &mint.pubkey(), None, None, 50, 5 * ONE).unwrap(),
    ];
    let err = env.send(&ixs, &issuer, &[&mint]).expect_err("must fail");
    println!("order error: {}", err.lines().next().unwrap());
    assert!(err.contains("InstructionError(1"), "InitializeMint2 itself fails: the account is sized for an extension that was never initialized\n{err}");
}

#[test]
fn task1_account_too_small_for_the_extensions_fails() {
    // Allocate for 3 extensions but initialize 4: the 4th has no space.
    let mut env = Env::new();
    let issuer = env.wallet();
    let mint = Keypair::new();
    let program = token_program();
    let len = ExtensionType::try_calculate_account_len::<Mint>(&[
        ExtensionType::TransferFeeConfig,
        ExtensionType::MetadataPointer,
        ExtensionType::DefaultAccountState,
    ])
    .unwrap();
    let ixs = vec![
        solana_system_interface::instruction::create_account(&issuer.pubkey(), &mint.pubkey(), env.rent(len), len as u64, &program),
        fee_ix::initialize_transfer_fee_config(&program, &mint.pubkey(), None, None, 50, 5 * ONE).unwrap(),
        spl_token_2022_interface::extension::metadata_pointer::instruction::initialize(&program, &mint.pubkey(), None, Some(mint.pubkey())).unwrap(),
        spl_token_2022_interface::extension::default_account_state::instruction::initialize_default_account_state(&program, &mint.pubkey(), &AccountState::Frozen).unwrap(),
        token_ix::initialize_mint_close_authority(&program, &mint.pubkey(), Some(&issuer.pubkey())).unwrap(),
        token_ix::initialize_mint2(&program, &mint.pubkey(), &issuer.pubkey(), Some(&issuer.pubkey()), DECIMALS).unwrap(),
    ];
    let err = env.send(&ixs, &issuer, &[&mint]).expect_err("must fail");
    println!("size error: {}", err.lines().next().unwrap());
    assert!(err.contains("InstructionError(4"), "the 4th extension init (index 4) fails\n{err}");
}

#[test]
fn task1_close_authority_closes_mint_only_with_zero_supply() {
    let mut w = world();
    let mint = w.mint_key();
    let dest = w.env.wallet();
    let close = |w: &mut common::World, dest: &Keypair| {
        let ix = token_ix::close_account(&token_program(), &mint, &dest.pubkey(), &w.issuer.pubkey(), &[]).unwrap();
        let issuer = w.issuer.insecure_clone();
        w.env.send(&[ix], &issuer, &[])
    };

    // With supply the mint cannot be closed: MintHasSupply = Custom(28)
    let (_u, acct) = w.kyc_user();
    let issuer = w.issuer.insecure_clone();
    mint_to(&mut w.env, &issuer, &mint, &acct, ONE).unwrap();
    token22_ct::env::assert_custom_error(close(&mut w, &dest), 28);

    // Burn the supply back to zero, then the close authority decommissions the mint.
    let ix = token_ix::burn_checked(&token_program(), &acct, &mint, &_u.pubkey(), &[], ONE, DECIMALS).unwrap();
    w.env.send(&[ix], &_u, &[]).unwrap();
    let before = w.env.lamports(&dest.pubkey());
    let mint_lamports = w.env.lamports(&mint);
    close(&mut w, &dest).unwrap();
    assert_eq!(w.env.lamports(&mint), 0, "mint closed");
    assert_eq!(w.env.lamports(&dest.pubkey()), before + mint_lamports, "rent returned");
}
