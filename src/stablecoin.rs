//! Tasks 1, 2 and 4: create the stablecoin mint, move it with the protocol fee,
//! and run the frozen-until-KYC flow. Task 5 reuses `create_mint` with more extensions.

use {
    crate::{
        env::Env,
        read::{read_account, read_mint},
    },
    solana_instruction::Instruction,
    solana_keypair::Keypair,
    solana_pubkey::Pubkey,
    solana_signer::Signer,
    solana_zk_sdk::encryption::{elgamal::ElGamalPubkey, pod::elgamal::PodElGamalPubkey},
    spl_associated_token_account_interface::{
        address::get_associated_token_address_with_program_id,
        instruction::create_associated_token_account,
    },
    spl_token_2022_interface::{
        extension::{
            confidential_transfer::instruction as ct_ix,
            confidential_transfer_fee::instruction as ct_fee_ix,
            default_account_state::instruction::initialize_default_account_state,
            metadata_pointer::instruction as metadata_pointer_ix,
            transfer_fee::instruction as fee_ix, ExtensionType,
        },
        instruction as token_ix,
        state::{AccountState, Mint},
    },
    spl_token_metadata_interface::state::TokenMetadata,
    spl_type_length_value::variable_len_pack::VariableLenPack,
};

pub const DECIMALS: u8 = 6;
pub const ONE: u64 = 1_000_000; // 1 rUSD in base units

pub fn token_program() -> Pubkey {
    spl_token_2022_interface::id()
}

/// Everything the issuer chooses when creating the mint.
pub struct MintParams {
    pub name: String,
    pub symbol: String,
    pub uri: String,
    /// Fee on every transfer, in basis points of the amount.
    pub fee_bps: u16,
    /// Cap on the fee per transfer, in base units.
    pub max_fee: u64,
    /// Task 5 only: the seizure authority.
    pub permanent_delegate: Option<Pubkey>,
    /// Task 5 only: confidential transfers.
    pub confidential: Option<ConfidentialParams>,
}

pub struct ConfidentialParams {
    /// false = manual approve policy: the CT authority must ApproveAccount.
    pub auto_approve_new_accounts: bool,
    /// Optional auditor that can decrypt transfer amounts.
    pub auditor: Option<ElGamalPubkey>,
    /// ElGamal key that can decrypt confidential withheld fees (required with a fee mint).
    pub withheld_fee_authority: ElGamalPubkey,
}

impl MintParams {
    pub fn remittance_usd() -> Self {
        Self {
            name: "Remit USD".into(),
            symbol: "rUSD".into(),
            uri: "https://example.com/rusd.json".into(),
            fee_bps: 50,         // 0.50%
            max_fee: 5 * ONE,    // never more than 5 rUSD per transfer
            permanent_delegate: None,
            confidential: None,
        }
    }

    /// Fixed-size extensions on the mint, in the order their init instructions run.
    pub fn extension_types(&self) -> Vec<ExtensionType> {
        let mut v = vec![
            ExtensionType::TransferFeeConfig,
            ExtensionType::MetadataPointer,
            ExtensionType::DefaultAccountState,
            ExtensionType::MintCloseAuthority,
        ];
        if self.permanent_delegate.is_some() {
            v.push(ExtensionType::PermanentDelegate);
        }
        if self.confidential.is_some() {
            v.push(ExtensionType::ConfidentialTransferMint);
            // A fee mint with confidential transfers also needs this one, so fees
            // taken from confidential transfers can be withheld in encrypted form.
            v.push(ExtensionType::ConfidentialTransferFeeConfig);
        }
        v
    }

    pub fn metadata(&self, mint: &Pubkey, update_authority: &Pubkey) -> TokenMetadata {
        TokenMetadata {
            update_authority: Some(*update_authority).try_into().unwrap(),
            mint: *mint,
            name: self.name.clone(),
            symbol: self.symbol.clone(),
            uri: self.uri.clone(),
            additional_metadata: vec![],
        }
    }
}

/// Sizes and rent of a created mint, for the README.
#[derive(Debug, Clone)]
pub struct MintSizes {
    /// Space allocated by CreateAccount: base mint + the fixed-size extensions.
    pub fixed_len: usize,
    /// Extra bytes the metadata adds, allocated later by the Token-2022 program itself
    /// when the metadata is initialized: 2-byte extension type + 2-byte length + Borsh body.
    pub metadata_tlv_len: usize,
    /// What `TokenMetadata::tlv_size_of()` returns: it assumes the generic 8-byte
    /// discriminator + 4-byte length header, so it is 8 bytes larger than Token-2022 needs.
    pub generic_tlv_size_of: usize,
    /// Lamports deposited up front: rent for fixed_len + metadata_tlv_len.
    pub lamports: u64,
}

/// Task 1 (and task 5 with more extensions): ONE transaction that
/// 1. creates the account at the length computed from the extension list,
///    funded for the final size including the variable-length metadata,
/// 2. runs every extension's initialize instruction,
/// 3. runs InitializeMint2 (always after the extensions),
/// 4. writes the token metadata into the mint itself.
pub fn create_mint(
    env: &mut Env,
    issuer: &Keypair,
    mint: &Keypair,
    p: &MintParams,
) -> Result<MintSizes, String> {
    let program = token_program();
    let issuer_key = issuer.pubkey();
    let mint_key = mint.pubkey();

    let fixed_len = ExtensionType::try_calculate_account_len::<Mint>(&p.extension_types())
        .map_err(|e| e.to_string())?;
    let metadata = p.metadata(&mint_key, &issuer_key);
    // Token-2022 TLV entry = ExtensionType (u16) + Length (u16) + value.
    let metadata_tlv_len = 4 + VariableLenPack::get_packed_len(&metadata).map_err(|e| e.to_string())?;
    let generic_tlv_size_of = metadata.tlv_size_of().map_err(|e| e.to_string())?;
    // Pay rent for the FINAL size now; the metadata init only reallocates, it
    // cannot take lamports from anyone.
    let lamports = env.rent(fixed_len + metadata_tlv_len);

    let mut ixs: Vec<Instruction> = vec![solana_system_interface::instruction::create_account(
        &issuer_key,
        &mint_key,
        lamports,
        fixed_len as u64,
        &program,
    )];
    ixs.push(
        fee_ix::initialize_transfer_fee_config(
            &program,
            &mint_key,
            Some(&issuer_key), // may change the fee
            Some(&issuer_key), // may withdraw withheld fees (issuer revenue)
            p.fee_bps,
            p.max_fee,
        )
        .unwrap(),
    );
    ixs.push(
        metadata_pointer_ix::initialize(&program, &mint_key, Some(issuer_key), Some(mint_key))
            .unwrap(),
    );
    ixs.push(initialize_default_account_state(&program, &mint_key, &AccountState::Frozen).unwrap());
    ixs.push(token_ix::initialize_mint_close_authority(&program, &mint_key, Some(&issuer_key)).unwrap());
    if let Some(delegate) = p.permanent_delegate {
        ixs.push(token_ix::initialize_permanent_delegate(&program, &mint_key, &delegate).unwrap());
    }
    if let Some(c) = &p.confidential {
        ixs.push(
            ct_ix::initialize_mint(
                &program,
                &mint_key,
                Some(issuer_key),
                c.auto_approve_new_accounts,
                c.auditor.map(PodElGamalPubkey::from),
            )
            .unwrap(),
        );
        ixs.push(
            ct_fee_ix::initialize_confidential_transfer_fee_config(
                &program,
                &mint_key,
                Some(issuer_key),
                &PodElGamalPubkey::from(c.withheld_fee_authority),
            )
            .unwrap(),
        );
    }
    // InitializeMint2 comes LAST among the account-shaping instructions.
    // A freeze authority is required by DefaultAccountState(Frozen).
    ixs.push(
        token_ix::initialize_mint2(&program, &mint_key, &issuer_key, Some(&issuer_key), DECIMALS)
            .unwrap(),
    );
    // Metadata lives in the mint (MetadataPointer points at the mint itself).
    ixs.push(spl_token_metadata_interface::instruction::initialize(
        &program,
        &mint_key,
        &issuer_key,
        &mint_key,
        &issuer_key,
        p.name.clone(),
        p.symbol.clone(),
        p.uri.clone(),
    ));

    env.send(&ixs, issuer, &[mint])?;
    Ok(MintSizes { fixed_len, metadata_tlv_len, generic_tlv_size_of, lamports })
}

pub fn ata(owner: &Pubkey, mint: &Pubkey) -> Pubkey {
    get_associated_token_address_with_program_id(owner, mint, &token_program())
}

/// Anyone can create anyone's ATA (the payer does not have to be the owner).
/// With DefaultAccountState(Frozen) the new account starts frozen.
pub fn create_ata(env: &mut Env, payer: &Keypair, owner: &Pubkey, mint: &Pubkey) -> Result<Pubkey, String> {
    let ix = create_associated_token_account(&payer.pubkey(), owner, mint, &token_program());
    env.send(&[ix], payer, &[])?;
    Ok(ata(owner, mint))
}

/// Task 4: the freeze authority thaws ONE account after KYC. The mint's default
/// state stays Frozen, so every other new account is still frozen.
pub fn kyc_thaw(env: &mut Env, freeze_authority: &Keypair, account: &Pubkey, mint: &Pubkey) -> Result<(), String> {
    let ix = token_ix::thaw_account(&token_program(), account, mint, &freeze_authority.pubkey(), &[]).unwrap();
    env.send(&[ix], freeze_authority, &[]).map(|_| ())
}

pub fn freeze(env: &mut Env, freeze_authority: &Keypair, account: &Pubkey, mint: &Pubkey) -> Result<(), String> {
    let ix = token_ix::freeze_account(&token_program(), account, mint, &freeze_authority.pubkey(), &[]).unwrap();
    env.send(&[ix], freeze_authority, &[]).map(|_| ())
}

pub fn mint_to(env: &mut Env, mint_authority: &Keypair, mint: &Pubkey, to: &Pubkey, amount: u64) -> Result<(), String> {
    let ix = token_ix::mint_to_checked(&token_program(), mint, to, &mint_authority.pubkey(), &[], amount, DECIMALS).unwrap();
    env.send(&[ix], mint_authority, &[]).map(|_| ())
}

/// Task 2: transfer with the protocol fee. The fee is computed from the LIVE mint
/// on every call: read the TransferFeeConfig now, take the current epoch from the
/// Clock sysvar, and ask the config for that epoch's fee (a fee change only takes
/// effect two epochs later, so a cached rate can be wrong).
/// Returns the fee that was charged.
pub fn transfer_with_fee(
    env: &mut Env,
    mint: &Pubkey,
    source: &Pubkey,
    destination: &Pubkey,
    owner: &Keypair,
    amount: u64,
) -> Result<u64, String> {
    let m = read_mint(env, mint)?;
    let fee = m
        .transfer_fee_config
        .as_ref()
        .ok_or("mint has no TransferFeeConfig")?
        .calculate_epoch_fee(env.epoch(), amount)
        .ok_or("fee overflow")?;
    let ix = fee_ix::transfer_checked_with_fee(
        &token_program(),
        source,
        mint,
        destination,
        &owner.pubkey(),
        &[],
        amount,
        m.decimals,
        fee,
    )
    .unwrap();
    env.send(&[ix], owner, &[])?;
    Ok(fee)
}

/// Same instruction but with a fee value the caller supplies (to show a stale fee fails).
pub fn transfer_with_given_fee(
    env: &mut Env,
    mint: &Pubkey,
    source: &Pubkey,
    destination: &Pubkey,
    owner: &Keypair,
    amount: u64,
    fee: u64,
) -> Result<(), String> {
    let ix = fee_ix::transfer_checked_with_fee(&token_program(), source, mint, destination, &owner.pubkey(), &[], amount, DECIMALS, fee).unwrap();
    env.send(&[ix], owner, &[]).map(|_| ())
}

/// Fee authority schedules a new fee. Token-2022 applies it from epoch + 2.
pub fn set_fee(env: &mut Env, fee_authority: &Keypair, mint: &Pubkey, bps: u16, max_fee: u64) -> Result<(), String> {
    let ix = fee_ix::set_transfer_fee(&token_program(), mint, &fee_authority.pubkey(), &[], bps, max_fee).unwrap();
    env.send(&[ix], fee_authority, &[]).map(|_| ())
}

/// Issuer revenue, step 1 (permissionless): move fees withheld in token accounts into the mint.
pub fn harvest_to_mint(env: &mut Env, payer: &Keypair, mint: &Pubkey, sources: &[&Pubkey]) -> Result<(), String> {
    let ix = fee_ix::harvest_withheld_tokens_to_mint(&token_program(), mint, sources).unwrap();
    env.send(&[ix], payer, &[]).map(|_| ())
}

/// Issuer revenue, step 2: the withdraw-withheld authority moves the fees out of the mint.
pub fn withdraw_fees_from_mint(env: &mut Env, withdraw_authority: &Keypair, mint: &Pubkey, destination: &Pubkey) -> Result<(), String> {
    let ix = fee_ix::withdraw_withheld_tokens_from_mint(&token_program(), mint, destination, &withdraw_authority.pubkey(), &[]).unwrap();
    env.send(&[ix], withdraw_authority, &[]).map(|_| ())
}

/// Convenience used by tests: amount held by a token account (read with StateWithExtensions).
pub fn balance(env: &Env, account: &Pubkey) -> u64 {
    read_account(env, account).map(|a| a.amount).unwrap_or(0)
}
