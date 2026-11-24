//! Task 9 (optional extension): an agent program moves tokens through an Approve
//! delegation; then the user enables CPI Guard. Runs on the mainnet Token-2022 binary.
use {
    agent::{AGENT_SEED, CONFIG_SEED},
    anchor_lang::{
        prelude::Pubkey,
        solana_program::{instruction::Instruction, system_program},
        InstructionData, ToAccountMetas,
    },
    litesvm::LiteSVM,
    solana_keypair::Keypair,
    solana_message::{Message, VersionedMessage},
    solana_signer::Signer,
    solana_transaction::versioned::VersionedTransaction,
    spl_token_2022_interface::{
        extension::{cpi_guard::instruction as guard_ix, ExtensionType, StateWithExtensions},
        instruction as token_ix,
        state::{Account, Mint},
    },
};

const DECIMALS: u8 = 6;
const ONE: u64 = 1_000_000;
const TOKEN_2022: &[u8] = include_bytes!("../../../../programs/token_2022_mainnet_2026-11-20.so");

struct World {
    svm: LiteSVM,
    pid: Pubkey,
    t22: Pubkey,
    operator: Keypair,
    user: Keypair,
    mint: Pubkey,
    source: Pubkey,
    destination: Pubkey,
    agent: Pubkey,
    config: Pubkey,
}

fn send(svm: &mut LiteSVM, ixs: &[Instruction], signers: &[&Keypair]) -> Result<(), String> {
    let msg = Message::new_with_blockhash(ixs, Some(&signers[0].pubkey()), &svm.latest_blockhash());
    let tx = VersionedTransaction::try_new(VersionedMessage::Legacy(msg), signers).unwrap();
    let res = svm.send_transaction(tx).map(|_| ()).map_err(|e| format!("{:?}\n{}", e.err, e.meta.logs.join("\n")));
    svm.expire_blockhash();
    res
}

fn assert_custom(res: Result<(), String>, code: u32) {
    let err = res.expect_err("transaction should have failed");
    assert!(err.contains(&format!("Custom({code})")), "expected Custom({code}), got:\n{err}");
}

/// Agent program + Token-2022 (mainnet binary). User holds 1,000 tokens in an account
/// that has room for the CpiGuard extension (not enabled yet).
fn world() -> World {
    let pid = agent::id();
    let t22 = spl_token_2022_interface::id();
    let mut svm = LiteSVM::new();
    svm.add_program(pid, include_bytes!(concat!(env!("CARGO_TARGET_TMPDIR"), "/../deploy/agent.so"))).unwrap();
    svm.add_program(t22, TOKEN_2022).unwrap();
    let (operator, user) = (Keypair::new(), Keypair::new());
    for k in [&operator, &user] {
        svm.airdrop(&k.pubkey(), 10_000_000_000).unwrap();
    }

    let mint = Keypair::new();
    let mint_len = ExtensionType::try_calculate_account_len::<Mint>(&[]).unwrap();
    let mint_rent = svm.minimum_balance_for_rent_exemption(mint_len);
    send(&mut svm, &[
        solana_system_interface::instruction::create_account(&user.pubkey(), &mint.pubkey(), mint_rent, mint_len as u64, &t22),
        token_ix::initialize_mint2(&t22, &mint.pubkey(), &user.pubkey(), None, DECIMALS).unwrap(),
    ], &[&user, &mint]).unwrap();

    let new_account = |svm: &mut LiteSVM, exts: &[ExtensionType], owner: &Pubkey| {
        let kp = Keypair::new();
        let len = ExtensionType::try_calculate_account_len::<Account>(exts).unwrap();
        let rent = svm.minimum_balance_for_rent_exemption(len);
        send(svm, &[
            solana_system_interface::instruction::create_account(&user.pubkey(), &kp.pubkey(), rent, len as u64, &t22),
            token_ix::initialize_account3(&t22, &kp.pubkey(), &mint.pubkey(), owner).unwrap(),
        ], &[&user, &kp]).unwrap();
        kp.pubkey()
    };
    let source = new_account(&mut svm, &[ExtensionType::CpiGuard], &user.pubkey());
    let destination = new_account(&mut svm, &[], &operator.pubkey());
    send(&mut svm, &[token_ix::mint_to_checked(&t22, &mint.pubkey(), &source, &user.pubkey(), &[], 1_000 * ONE, DECIMALS).unwrap()], &[&user]).unwrap();

    let agent = Pubkey::find_program_address(&[AGENT_SEED], &pid).0;
    let config = Pubkey::find_program_address(&[CONFIG_SEED], &pid).0;
    let ix = Instruction::new_with_bytes(
        pid,
        &agent::instruction::Initialize { operator: operator.pubkey() }.data(),
        agent::accounts::Initialize { payer: operator.pubkey(), config, agent, system_program: system_program::ID }.to_account_metas(None),
    );
    send(&mut svm, &[ix], &[&operator]).unwrap();
    World { svm, pid, t22, operator, user, mint: mint.pubkey(), source, destination, agent, config }
}

impl World {
    fn balance(&self, acct: &Pubkey) -> u64 {
        let data = self.svm.get_account(acct).unwrap().data;
        StateWithExtensions::<Account>::unpack(&data).unwrap().base.amount
    }
    fn delegated(&self) -> u64 {
        let data = self.svm.get_account(&self.source).unwrap().data;
        StateWithExtensions::<Account>::unpack(&data).unwrap().base.delegated_amount
    }
    /// The user approves the agent PDA directly (top-level instruction, not a CPI).
    fn approve_directly(&mut self, amount: u64) -> Result<(), String> {
        let ix = token_ix::approve_checked(&self.t22, &self.source, &self.mint, &self.agent, &self.user.pubkey(), &[], amount, DECIMALS).unwrap();
        let user = self.user.insecure_clone();
        send(&mut self.svm, &[ix], &[&user])
    }
    fn enable_cpi_guard(&mut self) {
        let ix = guard_ix::enable_cpi_guard(&self.t22, &self.source, &self.user.pubkey(), &[]).unwrap();
        let user = self.user.insecure_clone();
        send(&mut self.svm, &[ix], &[&user]).unwrap();
    }
    fn agent_transfer(&mut self, amount: u64) -> Result<(), String> {
        let ix = Instruction::new_with_bytes(
            self.pid,
            &agent::instruction::DelegatedTransfer { amount }.data(),
            agent::accounts::DelegatedTransfer {
                operator: self.operator.pubkey(),
                config: self.config,
                agent: self.agent,
                source: self.source,
                mint: self.mint,
                destination: self.destination,
                token_program: self.t22,
            }
            .to_account_metas(None),
        );
        let op = self.operator.insecure_clone();
        send(&mut self.svm, &[ix], &[&op])
    }
    fn owner_transfer_via_cpi(&mut self, amount: u64) -> Result<(), String> {
        let ix = Instruction::new_with_bytes(
            self.pid,
            &agent::instruction::OwnerTransferViaCpi { amount }.data(),
            agent::accounts::OwnerTransfer {
                owner: self.user.pubkey(),
                source: self.source,
                mint: self.mint,
                destination: self.destination,
                token_program: self.t22,
            }
            .to_account_metas(None),
        );
        let user = self.user.insecure_clone();
        send(&mut self.svm, &[ix], &[&user])
    }
    fn approve_via_cpi(&mut self, amount: u64) -> Result<(), String> {
        let ix = Instruction::new_with_bytes(
            self.pid,
            &agent::instruction::ApproveViaCpi { amount }.data(),
            agent::accounts::ApproveViaCpi { owner: self.user.pubkey(), source: self.source, agent: self.agent, token_program: self.t22 }
                .to_account_metas(None),
        );
        let user = self.user.insecure_clone();
        send(&mut self.svm, &[ix], &[&user])
    }
}

// TokenError indexes: InsufficientFunds 1, OwnerMismatch 4, CpiGuardTransferBlocked 42, CpiGuardApproveBlocked 45.

#[test]
fn task9_agent_spends_the_allowance_the_user_approved() {
    let mut w = world();
    w.approve_directly(100 * ONE).unwrap();
    w.agent_transfer(40 * ONE).unwrap();
    assert_eq!(w.balance(&w.destination), 40 * ONE);
    assert_eq!(w.delegated(), 60 * ONE, "allowance goes down");
}

#[test]
fn task9_delegated_path_still_works_after_cpi_guard_is_enabled() {
    let mut w = world();
    w.approve_directly(100 * ONE).unwrap();
    w.enable_cpi_guard();
    // The agent PDA signs as DELEGATE inside a CPI: CPI Guard does not block it.
    w.agent_transfer(30 * ONE).unwrap();
    assert_eq!(w.balance(&w.destination), 30 * ONE);
}

#[test]
fn task9_cpi_guard_blocks_an_owner_signed_transfer_inside_a_cpi() {
    let mut w = world();
    // Guard off: forwarding the owner's signature through a program works.
    w.owner_transfer_via_cpi(ONE).unwrap();
    // Guard on: the same call fails with CpiGuardTransferBlocked = Custom(42).
    w.enable_cpi_guard();
    assert_custom(w.owner_transfer_via_cpi(ONE), 42);
}

#[test]
fn task9_cpi_guard_blocks_approve_inside_a_cpi_but_not_a_direct_approve() {
    let mut w = world();
    w.enable_cpi_guard();
    // CpiGuardApproveBlocked = Custom(45): a program cannot set up the delegation for the user.
    assert_custom(w.approve_via_cpi(100 * ONE), 45);
    // The user approving directly still works, and then the agent can spend it.
    w.approve_directly(100 * ONE).unwrap();
    w.agent_transfer(10 * ONE).unwrap();
}

#[test]
fn task9_agent_cannot_spend_more_than_the_allowance_fails_insufficient_funds() {
    let mut w = world();
    w.approve_directly(10 * ONE).unwrap();
    assert_custom(w.agent_transfer(11 * ONE), 1);
}

#[test]
fn task9_revoke_stops_the_agent_fails_owner_mismatch() {
    let mut w = world();
    w.approve_directly(10 * ONE).unwrap();
    let ix = token_ix::revoke(&w.t22, &w.source, &w.user.pubkey(), &[]).unwrap();
    let user = w.user.insecure_clone();
    send(&mut w.svm, &[ix], &[&user]).unwrap();
    // The PDA is no longer the delegate, so it is checked as if it were the owner.
    assert_custom(w.agent_transfer(ONE), 4);
}

#[test]
fn task9_only_the_operator_can_run_the_agent_fails_has_one() {
    let mut w = world();
    w.approve_directly(10 * ONE).unwrap();
    let stranger = Keypair::new();
    w.svm.airdrop(&stranger.pubkey(), 1_000_000_000).unwrap();
    w.operator = stranger;
    // ConstraintHasOne = Custom(2001)
    assert_custom(w.agent_transfer(ONE), 2001);
}
