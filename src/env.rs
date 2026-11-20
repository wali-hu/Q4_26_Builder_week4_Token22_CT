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

pub struct Env {
    pub svm: LiteSVM,
}

impl Default for Env {
    fn default() -> Self {
        Self::new()
    }
}

impl Env {
    /// LiteSVM with every feature enabled. That includes BOTH
    /// `disable_zk_elgamal_proof_program` and `reenable_zk_elgamal_proof_program`,
    /// so the ZK ElGamal proof program runs here (it does not on devnet or mainnet).
    pub fn new() -> Self {
        Self { svm: LiteSVM::new() }
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
