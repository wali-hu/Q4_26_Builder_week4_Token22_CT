//! A small wrapper around LiteSVM: send transactions, read accounts, move the clock.

use {
    litesvm::{types::TransactionMetadata, LiteSVM},
    solana_clock::Clock,
    solana_keypair::Keypair,
    solana_message::Message,
    solana_signer::Signer,
    solana_transaction::Transaction,
    solana_instruction::Instruction,
    solana_pubkey::Pubkey,
};

/// Token-2022 as deployed on mainnet-beta, dumped with
/// `solana program dump TokenzQdBNbLqP5VEhdkAS6EPFLC1PHnBqCXEpPxuEb ... --url mainnet-beta`.
pub const MAINNET_TOKEN_2022: &[u8] = include_bytes!("../programs/token_2022_mainnet_2026-11-20.so");

pub struct Env {
    pub svm: LiteSVM,
}

impl Default for Env {
    fn default() -> Self {
        Self::new()
    }
}

impl Env {
    /// LiteSVM with every feature enabled (that includes BOTH
    /// `disable_zk_elgamal_proof_program` and `reenable_zk_elgamal_proof_program`, so the
    /// ZK ElGamal proof program runs here; on devnet and mainnet it does not).
    ///
    /// The Token-2022 program is the binary deployed on mainnet (program@v11.1.0, dumped
    /// on 20 Nov 2026), not LiteSVM's bundled v10.0.0, which was built without the
    /// `zk-ops` feature and refuses Deposit, Withdraw, Transfer and ApplyPendingBalance.
    pub fn new() -> Self {
        let mut svm = LiteSVM::new();
        svm.add_program(spl_token_2022_interface::id(), MAINNET_TOKEN_2022).unwrap();
        Self { svm }
    }

    /// LiteSVM exactly as shipped: its bundled Token-2022 v10.0.0 binary.
    pub fn with_bundled_token2022() -> Self {
        Self { svm: LiteSVM::new() }
    }

    /// Same as `new`, but with the ZK ElGamal feature pair set the way devnet and
    /// mainnet have it today: `disable_zk_elgamal_proof_program` active and
    /// `reenable_zk_elgamal_proof_program` NOT active. Builtins are loaded after the
    /// feature set, so the proof program sees these flags.
    pub fn with_devnet_zk_features() -> Self {
        let mut features = agave_feature_set::FeatureSet::all_enabled();
        features.deactivate(&agave_feature_set::reenable_zk_elgamal_proof_program::id());
        let svm = LiteSVM::default()
            .with_feature_set(features)
            .with_builtins()
            .with_lamports(1_000_000_000_000_000_000)
            .with_sysvars()
            .with_default_programs()
            .with_sigverify(true)
            .with_blockhash_check(true);
        let mut env = Self { svm };
        env.svm.add_program(spl_token_2022_interface::id(), MAINNET_TOKEN_2022).unwrap();
        env
    }

    /// A new wallet with 100 SOL.
    pub fn wallet(&mut self) -> Keypair {
        let kp = Keypair::new();
        self.svm.airdrop(&kp.pubkey(), 100_000_000_000).unwrap();
        kp
    }

    /// Send one transaction. `payer` pays the fee and signs; `signers` are the other signers.
    /// Err holds the transaction error plus the program logs.
    pub fn send(
        &mut self,
        ixs: &[Instruction],
        payer: &Keypair,
        signers: &[&Keypair],
    ) -> Result<TransactionMetadata, String> {
        let mut all: Vec<&Keypair> = vec![payer];
        for s in signers {
            if s.pubkey() != payer.pubkey() {
                all.push(s);
            }
        }
        let msg = Message::new(ixs, Some(&payer.pubkey()));
        let tx = Transaction::new(&all, msg, self.svm.latest_blockhash());
        let res = self
            .svm
            .send_transaction(tx)
            .map_err(|e| format!("{:?}\n{}", e.err, e.meta.logs.join("\n")));
        self.svm.expire_blockhash();
        res
    }

    pub fn data(&self, address: &Pubkey) -> Vec<u8> {
        self.svm.get_account(address).map(|a| a.data).unwrap_or_default()
    }

    pub fn lamports(&self, address: &Pubkey) -> u64 {
        self.svm.get_balance(address).unwrap_or(0)
    }

    pub fn rent(&self, len: usize) -> u64 {
        self.svm.minimum_balance_for_rent_exemption(len)
    }

    pub fn epoch(&self) -> u64 {
        self.svm.get_sysvar::<Clock>().epoch
    }

    /// Move the cluster to a later epoch (used to show the fee schedule changing).
    pub fn set_epoch(&mut self, epoch: u64) {
        let mut clock: Clock = self.svm.get_sysvar();
        clock.epoch = epoch;
        self.svm.set_sysvar(&clock);
    }
}

/// Assert a result failed with this custom program error number, e.g. `Custom(17)`.
pub fn assert_custom_error<T: std::fmt::Debug>(res: Result<T, String>, code: u32) {
    let err = res.expect_err("transaction should have failed");
    assert!(
        err.contains(&format!("Custom({code})")),
        "expected Custom({code}), got:\n{err}"
    );
}
