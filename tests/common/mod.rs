#![allow(dead_code)]
use {
    solana_keypair::Keypair,
    solana_pubkey::Pubkey,
    solana_signer::Signer,
    token22_ct::{
        env::Env,
        stablecoin::{create_ata, create_mint, kyc_thaw, MintParams, MintSizes},
    },
};

pub struct World {
    pub env: Env,
    pub issuer: Keypair,
    pub mint: Keypair,
    pub sizes: MintSizes,
}

/// The task 1 stablecoin: fee + metadata pointer + default frozen + close authority.
pub fn world() -> World {
    world_with(MintParams::remittance_usd())
}

pub fn world_with(params: MintParams) -> World {
    let mut env = Env::new();
    let issuer = env.wallet();
    let mint = Keypair::new();
    let sizes = create_mint(&mut env, &issuer, &mint, &params).expect("create mint");
    World { env, issuer, mint, sizes }
}

impl World {
    pub fn mint_key(&self) -> Pubkey {
        self.mint.pubkey()
    }

    /// A user whose token account was created (frozen by default) and then thawed
    /// by the issuer after "KYC".
    pub fn kyc_user(&mut self) -> (Keypair, Pubkey) {
        let user = self.env.wallet();
        let mint = self.mint_key();
        let account = create_ata(&mut self.env, &user, &user.pubkey(), &mint).unwrap();
        kyc_thaw(&mut self.env, &self.issuer, &account, &mint).unwrap();
        (user, account)
    }

    /// A user whose account exists but is still frozen (no KYC yet).
    pub fn frozen_user(&mut self) -> (Keypair, Pubkey) {
        let user = self.env.wallet();
        let mint = self.mint_key();
        let account = create_ata(&mut self.env, &user, &user.pubkey(), &mint).unwrap();
        (user, account)
    }
}

// ---------- confidential world (tasks 5 to 8) ----------

use {
    solana_zk_sdk::encryption::elgamal::ElGamalKeypair,
    token22_ct::{
        confidential::{add_ct_space, approve, configure, CtUser},
        stablecoin::{mint_to, ConfidentialParams},
    },
};

pub struct CtWorld {
    pub w: World,
    /// Permanent delegate: the seizure authority required by the regulator.
    pub seizer: Keypair,
    /// Auditor key: can decrypt every confidential transfer amount.
    pub auditor: ElGamalKeypair,
    /// Can decrypt confidential withheld fees.
    pub withheld: ElGamalKeypair,
}

/// Task 5: the re-issued mint. Same extensions as task 1, plus PermanentDelegate and
/// confidential transfers with manual approval (auto_approve_new_accounts = false).
pub fn ct_world() -> CtWorld {
    let seizer = Keypair::new();
    let auditor = ElGamalKeypair::new_rand();
    let withheld = ElGamalKeypair::new_rand();
    let mut params = MintParams::remittance_usd();
    params.permanent_delegate = Some(seizer.pubkey());
    params.confidential = Some(ConfidentialParams {
        auto_approve_new_accounts: false,
        auditor: Some(*auditor.pubkey()),
        withheld_fee_authority: *withheld.pubkey(),
    });
    let mut w = world_with(params);
    w.env.svm.airdrop(&seizer.pubkey(), 10_000_000_000).unwrap();
    CtWorld { w, seizer, auditor, withheld }
}

impl CtWorld {
    /// KYC'd user with a configured, approved confidential account and `public` tokens.
    pub fn ct_user(&mut self, public: u64) -> CtUser {
        let (owner, account) = self.w.kyc_user();
        let user = CtUser::new(owner, account);
        let mint = self.w.mint_key();
        add_ct_space(&mut self.w.env, &user).unwrap();
        let owner = user.owner.insecure_clone();
        configure(&mut self.w.env, &user, &mint, &owner).unwrap();
        let issuer = self.w.issuer.insecure_clone();
        approve(&mut self.w.env, &issuer, &user.account, &mint).unwrap();
        if public > 0 {
            mint_to(&mut self.w.env, &issuer, &mint, &user.account, public).unwrap();
        }
        user
    }
}
