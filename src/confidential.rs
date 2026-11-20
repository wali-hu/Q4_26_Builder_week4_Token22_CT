//! Tasks 5 and 6: the confidential lifecycle with real zero-knowledge proofs.
//!
//! Balances in a confidential account are ElGamal ciphertexts. The owner keeps two keys:
//! - an ElGamal keypair: incoming amounts are encrypted to its public key,
//! - an AES key (AeKey): a private, cheap-to-decrypt copy of the available balance.
//! Every instruction that spends encrypted value carries proofs that the ZK ElGamal
//! proof program verifies. Small proofs fit inside the same transaction; bigger ones
//! are verified first into "context state" accounts that the Token-2022 instruction reads.

use {
    crate::{
        env::Env,
        read::{read_account, read_mint},
        stablecoin::{token_program, DECIMALS},
    },
    bytemuck::Pod,
    solana_account::Account as SolAccount,
    solana_keypair::Keypair,
    solana_pubkey::Pubkey,
    solana_signer::Signer,
    solana_zk_sdk::{
        encryption::{
            auth_encryption::{AeCiphertext, AeKey},
            elgamal::{ElGamalCiphertext, ElGamalKeypair, ElGamalPubkey},
        },
        zk_elgamal_proof_program::{
            self,
            instruction::{close_context_state, ContextStateInfo, ProofInstruction},
            proof_data::{PubkeyValidityProofData, ZkProofData},
            state::ProofContextState,
        },
    },
    spl_token_2022_interface::{
        extension::{
            confidential_transfer::{
                instruction as ct_ix, ConfidentialTransferAccount,
                DEFAULT_MAXIMUM_PENDING_BALANCE_CREDIT_COUNTER, PENDING_BALANCE_LO_BIT_LENGTH,
            },
            ExtensionType,
        },
        instruction as token_ix,
    },
    spl_token_confidential_transfer_proof_extraction::instruction::ProofLocation,
    spl_token_confidential_transfer_proof_generation::{
        transfer_with_fee::transfer_with_fee_split_proof_data, withdraw::withdraw_proof_data,
    },
};

/// A wallet that holds a confidential token account and its two encryption keys.
pub struct CtUser {
    pub owner: Keypair,
    pub account: Pubkey,
    pub elgamal: ElGamalKeypair,
    pub ae: AeKey,
}

impl CtUser {
    /// Keys are random here. A real wallet derives both deterministically from a
    /// signature by the owner (`ElGamalKeypair::new_from_signer`, `AeKey::new_from_signer`),
    /// so they can be recovered from the wallet alone.
    pub fn new(owner: Keypair, account: Pubkey) -> Self {
        Self { owner, account, elgamal: ElGamalKeypair::new_rand(), ae: AeKey::new_rand() }
    }
}

/// Step 0: give the token account room for the confidential extensions. Only the owner
/// can reallocate. A fee mint also needs ConfidentialTransferFeeAmount (encrypted
/// withheld fees), which ConfigureAccount initializes.
pub fn add_ct_space(env: &mut Env, user: &CtUser) -> Result<(), String> {
    let ix = token_ix::reallocate(
        &token_program(),
        &user.account,
        &user.owner.pubkey(),
        &user.owner.pubkey(),
        &[],
        &[ExtensionType::ConfidentialTransferAccount, ExtensionType::ConfidentialTransferFeeAmount],
    )
    .unwrap();
    env.send(&[ix], &user.owner, &[]).map(|_| ())
}

/// ConfigureAccount: the OWNER registers their ElGamal public key, with a proof that
/// they know its secret key (PubkeyValidity, small enough to sit in the same transaction).
/// `signer` is who signs as the authority: the owner normally; anyone else is refused.
pub fn configure(env: &mut Env, user: &CtUser, mint: &Pubkey, signer: &Keypair) -> Result<(), String> {
    let proof = PubkeyValidityProofData::new(&user.elgamal).unwrap();
    let ixs = ct_ix::configure_account(
        &token_program(),
        &user.account,
        mint,
        &user.ae.encrypt(0).into(),
        DEFAULT_MAXIMUM_PENDING_BALANCE_CREDIT_COUNTER,
        &signer.pubkey(),
        &[],
        ProofLocation::InstructionOffset(1.try_into().unwrap(), &proof),
    )
    .unwrap();
    env.send(&ixs, signer, &[]).map(|_| ())
}

/// ApproveAccount: with auto-approve off, the confidential-transfer authority approves.
pub fn approve(env: &mut Env, ct_authority: &Keypair, account: &Pubkey, mint: &Pubkey) -> Result<(), String> {
    let ix = ct_ix::approve_account(&token_program(), account, mint, &ct_authority.pubkey(), &[]).unwrap();
    env.send(&[ix], ct_authority, &[]).map(|_| ())
}

/// Deposit: public balance -> PENDING confidential balance. No proof needed: the amount
/// is public in this instruction anyway.
pub fn deposit(env: &mut Env, user: &CtUser, mint: &Pubkey, amount: u64) -> Result<(), String> {
    let ix = ct_ix::deposit(&token_program(), &user.account, mint, amount, DECIMALS, &user.owner.pubkey(), &[]).unwrap();
    env.send(&[ix], &user.owner, &[]).map(|_| ())
}

fn ct_state(env: &Env, account: &Pubkey) -> ConfidentialTransferAccount {
    read_account(env, account).unwrap().ct.expect("account is not configured for CT")
}

/// Available balance, decrypted with the owner's AES key (fast, exact).
pub fn available(env: &Env, user: &CtUser) -> u64 {
    let ct = ct_state(env, &user.account);
    let c: AeCiphertext = ct.decryptable_available_balance.try_into().unwrap();
    user.ae.decrypt(&c).unwrap()
}

/// Pending balance = lo (16 bits) + hi << 16, each decrypted with the ElGamal secret key.
pub fn pending(env: &Env, user: &CtUser) -> u64 {
    let ct = ct_state(env, &user.account);
    let lo: ElGamalCiphertext = ct.pending_balance_lo.try_into().unwrap();
    let hi: ElGamalCiphertext = ct.pending_balance_hi.try_into().unwrap();
    let lo = user.elgamal.secret().decrypt_u32(&lo).unwrap();
    let hi = user.elgamal.secret().decrypt_u32(&hi).unwrap();
    lo + (hi << PENDING_BALANCE_LO_BIT_LENGTH)
}

/// ApplyPendingBalance: pending -> available. The owner supplies the new AES-encrypted
/// available balance and the credit counter they saw, so a credit that lands in between
/// is not lost (the program checks the counter).
pub fn apply_pending(env: &mut Env, user: &CtUser) -> Result<(), String> {
    let ct = ct_state(env, &user.account);
    let new_available = available(env, user) + pending(env, user);
    let ix = ct_ix::apply_pending_balance(
        &token_program(),
        &user.account,
        u64::from(ct.pending_balance_credit_counter),
        &user.ae.encrypt(new_available).into(),
        &user.owner.pubkey(),
        &[],
    )
    .unwrap();
    env.send(&[ix], &user.owner, &[]).map(|_| ())
}

/// Proof verification is expensive (the 256-bit range proof alone costs 368,000 CU,
/// `VERIFY_BATCHED_RANGE_PROOF_U256_COMPUTE_UNITS` in the proof program), above the
/// default 200,000 CU per instruction, so verify transactions raise the limit.
fn cu_limit() -> solana_instruction::Instruction {
    solana_compute_budget_interface::ComputeBudgetInstruction::set_compute_unit_limit(600_000)
}

/// Verify one proof into a new context-state account owned by the ZK program, so a
/// later Token-2022 instruction can read the verified context instead of the proof.
/// Two transactions: create the account, then verify into it (big proofs do not fit with
/// the create instruction).
fn verify_into_context<T, U>(env: &mut Env, payer: &Keypair, kind: ProofInstruction, data: &T) -> Result<Pubkey, String>
where
    T: Pod + ZkProofData<U>,
    U: Pod,
{
    let ctx = Keypair::new();
    let space = std::mem::size_of::<ProofContextState<U>>();
    let create = solana_system_interface::instruction::create_account(
        &payer.pubkey(),
        &ctx.pubkey(),
        env.rent(space),
        space as u64,
        &zk_elgamal_proof_program::id(),
    );
    env.send(&[create], payer, &[&ctx])?;
    let info = ContextStateInfo { context_state_account: &ctx.pubkey(), context_state_authority: &payer.pubkey() };
    let verify = kind.encode_verify_proof(Some(info), data);
    env.send(&[cu_limit(), verify], payer, &[])?;
    Ok(ctx.pubkey())
}

/// The 256-bit batched range proof (about 1,064 bytes of proof data) does not fit in a
/// 1,232-byte transaction. The ZK program can read a proof from any account instead
/// (no owner check: the proof is self-verifying). On a real cluster that account is
/// written in chunks with the SPL Record program; LiteSVM lets the test place the
/// bytes directly. The verification itself is the real ZK program either way.
fn verify_from_record<T, U>(env: &mut Env, payer: &Keypair, kind: ProofInstruction, data: &T) -> Result<Pubkey, String>
where
    T: Pod + ZkProofData<U>,
    U: Pod,
{
    let record = Pubkey::new_unique();
    let bytes = bytemuck::bytes_of(data).to_vec();
    env.svm
        .set_account(
            record,
            SolAccount { lamports: env.rent(bytes.len()), data: bytes, owner: Pubkey::new_unique(), executable: false, rent_epoch: 0 },
        )
        .unwrap();
    let ctx = Keypair::new();
    let space = std::mem::size_of::<ProofContextState<U>>();
    let create = solana_system_interface::instruction::create_account(
        &payer.pubkey(),
        &ctx.pubkey(),
        env.rent(space),
        space as u64,
        &zk_elgamal_proof_program::id(),
    );
    env.send(&[create], payer, &[&ctx])?;
    let info = ContextStateInfo { context_state_account: &ctx.pubkey(), context_state_authority: &payer.pubkey() };
    let verify = kind.encode_verify_proof_from_account(Some(info), &record, 0);
    env.send(&[cu_limit(), verify], payer, &[])?;
    Ok(ctx.pubkey())
}

/// Close context-state accounts and return their rent to the payer.
fn close_contexts(env: &mut Env, payer: &Keypair, contexts: &[Pubkey]) -> Result<(), String> {
    let ixs: Vec<_> = contexts
        .iter()
        .map(|c| {
            close_context_state(
                ContextStateInfo { context_state_account: c, context_state_authority: &payer.pubkey() },
                &payer.pubkey(),
            )
        })
        .collect();
    env.send(&ixs, payer, &[]).map(|_| ())
}

/// Withdraw: AVAILABLE confidential balance -> public balance. Needs an equality proof
/// (the new balance ciphertext matches a commitment) and a 64-bit range proof (the
/// remaining balance is not negative). `authority` normally is the owner.
pub fn withdraw(env: &mut Env, user: &CtUser, mint: &Pubkey, amount: u64, authority: &Keypair) -> Result<(), String> {
    let current = available(env, user);
    withdraw_claiming(env, user, mint, amount, current, authority)
}

/// Same as `withdraw`, but the client CLAIMS an available balance instead of reading it.
/// Used to show that proofs built on a wrong balance (for example counting a pending
/// deposit that was never applied) are rejected on-chain.
pub fn withdraw_claiming(env: &mut Env, user: &CtUser, mint: &Pubkey, amount: u64, current: u64, authority: &Keypair) -> Result<(), String> {
    let ct = ct_state(env, &user.account);
    let avail_ct: ElGamalCiphertext = ct.available_balance.try_into().unwrap();
    let proofs = withdraw_proof_data(&avail_ct, current, amount, &user.elgamal).map_err(|e| format!("{e:?}"))?;

    let payer = authority;
    let eq = verify_into_context(env, payer, ProofInstruction::VerifyCiphertextCommitmentEquality, &proofs.equality_proof_data)?;
    let range = verify_into_context(env, payer, ProofInstruction::VerifyBatchedRangeProofU64, &proofs.range_proof_data)?;

    let ixs = ct_ix::withdraw(
        &token_program(),
        &user.account,
        mint,
        amount,
        DECIMALS,
        &user.ae.encrypt(current - amount).into(),
        &authority.pubkey(),
        &[],
        ProofLocation::ContextStateAccount(&eq),
        ProofLocation::ContextStateAccount(&range),
    )
    .unwrap();
    let res = env.send(&ixs, authority, &[]).map(|_| ());
    close_contexts(env, payer, &[eq, range])?;
    res
}

/// What a confidential transfer revealed to whom, for the tests and the README.
pub struct TransferOutcome {
    pub fee: u64,
    /// Transfer amount encrypted to the auditor (lo, hi parts).
    pub auditor_ciphertext_lo: ElGamalCiphertext,
    pub auditor_ciphertext_hi: ElGamalCiphertext,
}

/// Confidential transfer on a FEE mint. Token-2022 requires the with-fee proof set:
/// 1. ciphertext-commitment equality (source's new balance),
/// 2. batched 3-handle ciphertext validity (amount encrypted to source, destination, auditor),
/// 3. percentage-with-cap (the fee is fee_bps of the amount, capped at maximum_fee),
/// 4. batched 2-handle ciphertext validity (fee encrypted to destination and withheld authority),
/// 5. 256-bit batched range proof (amounts, fee and remaining balance are in range).
pub fn transfer_with_fee(
    env: &mut Env,
    src: &CtUser,
    dst: &CtUser,
    mint: &Pubkey,
    amount: u64,
    auditor: Option<&ElGamalPubkey>,
    withheld_authority: &ElGamalPubkey,
) -> Result<TransferOutcome, String> {
    // Fee parameters from the LIVE mint for the current epoch (same rule as task 2).
    let m = read_mint(env, mint)?;
    let cfg = m.transfer_fee_config.ok_or("no fee config")?;
    let epoch_fee = cfg.get_epoch_fee(env.epoch());
    let bps = u16::from(epoch_fee.transfer_fee_basis_points);
    let max_fee = u64::from(epoch_fee.maximum_fee);
    let fee = cfg.calculate_epoch_fee(env.epoch(), amount).unwrap();

    let ct = ct_state(env, &src.account);
    let current = available(env, src);
    let avail_ct: ElGamalCiphertext = ct.available_balance.try_into().unwrap();
    let decryptable: AeCiphertext = ct.decryptable_available_balance.try_into().unwrap();
    let p = transfer_with_fee_split_proof_data(
        &avail_ct,
        &decryptable,
        amount,
        &src.elgamal,
        &src.ae,
        dst.elgamal.pubkey(),
        auditor,
        withheld_authority,
        bps,
        max_fee,
    )
    .map_err(|e| format!("{e:?}"))?;

    let payer = &src.owner;
    let eq = verify_into_context(env, payer, ProofInstruction::VerifyCiphertextCommitmentEquality, &p.equality_proof_data)?;
    let validity = verify_into_context(
        env,
        payer,
        ProofInstruction::VerifyBatchedGroupedCiphertext3HandlesValidity,
        &p.transfer_amount_ciphertext_validity_proof_data_with_ciphertext.proof_data,
    )?;
    let fee_sigma = verify_into_context(env, payer, ProofInstruction::VerifyPercentageWithCap, &p.percentage_with_cap_proof_data)?;
    let fee_validity = verify_into_context(
        env,
        payer,
        ProofInstruction::VerifyBatchedGroupedCiphertext2HandlesValidity,
        &p.fee_ciphertext_validity_proof_data,
    )?;
    let range = verify_from_record(env, payer, ProofInstruction::VerifyBatchedRangeProofU256, &p.range_proof_data)?;

    let auditor_lo = p.transfer_amount_ciphertext_validity_proof_data_with_ciphertext.ciphertext_lo;
    let auditor_hi = p.transfer_amount_ciphertext_validity_proof_data_with_ciphertext.ciphertext_hi;
    let ixs = ct_ix::transfer_with_fee(
        &token_program(),
        &src.account,
        mint,
        &dst.account,
        &src.ae.encrypt(current - amount).into(),
        &auditor_lo,
        &auditor_hi,
        &src.owner.pubkey(),
        &[],
        ProofLocation::ContextStateAccount(&eq),
        ProofLocation::ContextStateAccount(&validity),
        ProofLocation::ContextStateAccount(&fee_sigma),
        ProofLocation::ContextStateAccount(&fee_validity),
        ProofLocation::ContextStateAccount(&range),
    )
    .unwrap();
    let res = env.send(&ixs, &src.owner, &[]);
    close_contexts(env, payer, &[eq, validity, fee_sigma, fee_validity, range])?;
    res?;
    Ok(TransferOutcome {
        fee,
        auditor_ciphertext_lo: auditor_lo.try_into().unwrap(),
        auditor_ciphertext_hi: auditor_hi.try_into().unwrap(),
    })
}
