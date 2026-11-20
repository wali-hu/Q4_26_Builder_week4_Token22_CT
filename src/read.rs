//! Task 3: every read of mint and account state goes through
//! `StateWithExtensions::<Mint|Account>::unpack` and `get_extension`. There is no raw
//! `Mint::unpack` anywhere: a Token-2022 account is longer than 82/165 bytes, and the
//! base layout alone would ignore (or reject) the extensions.

use {
    crate::env::Env,
    solana_pubkey::Pubkey,
    spl_token_2022_interface::{
        extension::{
            confidential_transfer::{ConfidentialTransferAccount, ConfidentialTransferMint},
            default_account_state::DefaultAccountState,
            metadata_pointer::MetadataPointer,
            mint_close_authority::MintCloseAuthority,
            permanent_delegate::PermanentDelegate,
            transfer_fee::{TransferFeeAmount, TransferFeeConfig},
            BaseStateWithExtensions, ExtensionType, StateWithExtensions,
        },
        state::{Account, AccountState, Mint},
    },
    spl_token_metadata_interface::state::TokenMetadata,
};

#[derive(Debug)]
pub struct MintView {
    pub data_len: usize,
    pub supply: u64,
    pub decimals: u8,
    pub mint_authority: Option<Pubkey>,
    pub freeze_authority: Option<Pubkey>,
    pub extensions: Vec<ExtensionType>,
    pub transfer_fee_config: Option<TransferFeeConfig>,
    pub metadata_pointer_target: Option<Pubkey>,
    pub default_state: Option<AccountState>,
    pub close_authority: Option<Pubkey>,
    pub permanent_delegate: Option<Pubkey>,
    pub metadata: Option<TokenMetadata>,
    pub ct_auto_approve: Option<bool>,
}

#[derive(Debug)]
pub struct AccountView {
    pub data_len: usize,
    pub owner: Pubkey,
    pub amount: u64,
    pub frozen: bool,
    pub extensions: Vec<ExtensionType>,
    pub withheld_fee: u64,
    pub ct: Option<ConfidentialTransferAccount>,
}

fn opt(p: solana_program_option::COption<Pubkey>) -> Option<Pubkey> {
    p.into()
}

fn nz(p: spl_pod::optional_keys::OptionalNonZeroPubkey) -> Option<Pubkey> {
    Option::<Pubkey>::from(p)
}

pub fn read_mint(env: &Env, mint: &Pubkey) -> Result<MintView, String> {
    let data = env.data(mint);
    let state = StateWithExtensions::<Mint>::unpack(&data).map_err(|e| e.to_string())?;
    Ok(MintView {
        data_len: data.len(),
        supply: state.base.supply,
        decimals: state.base.decimals,
        mint_authority: opt(state.base.mint_authority),
        freeze_authority: opt(state.base.freeze_authority),
        extensions: state.get_extension_types().map_err(|e| e.to_string())?,
        transfer_fee_config: state.get_extension::<TransferFeeConfig>().ok().copied(),
        metadata_pointer_target: state.get_extension::<MetadataPointer>().ok().and_then(|m| nz(m.metadata_address)),
        default_state: state
            .get_extension::<DefaultAccountState>()
            .ok()
            .map(|d| AccountState::try_from(d.state).unwrap()),
        close_authority: state.get_extension::<MintCloseAuthority>().ok().and_then(|m| nz(m.close_authority)),
        permanent_delegate: state.get_extension::<PermanentDelegate>().ok().and_then(|m| nz(m.delegate)),
        metadata: state.get_variable_len_extension::<TokenMetadata>().ok(),
        ct_auto_approve: state
            .get_extension::<ConfidentialTransferMint>()
            .ok()
            .map(|c| bool::from(c.auto_approve_new_accounts)),
    })
}

pub fn read_account(env: &Env, account: &Pubkey) -> Result<AccountView, String> {
    let data = env.data(account);
    let state = StateWithExtensions::<Account>::unpack(&data).map_err(|e| e.to_string())?;
    Ok(AccountView {
        data_len: data.len(),
        owner: state.base.owner,
        amount: state.base.amount,
        frozen: state.base.state == AccountState::Frozen,
        extensions: state.get_extension_types().map_err(|e| e.to_string())?,
        withheld_fee: state
            .get_extension::<TransferFeeAmount>()
            .map(|f| u64::from(f.withheld_amount))
            .unwrap_or(0),
        ct: state.get_extension::<ConfidentialTransferAccount>().ok().copied(),
    })
}

/// Fees withheld in the mint itself (after harvest).
pub fn mint_withheld(env: &Env, mint: &Pubkey) -> u64 {
    read_mint(env, mint)
        .ok()
        .and_then(|m| m.transfer_fee_config)
        .map(|c| u64::from(c.withheld_amount))
        .unwrap_or(0)
}
