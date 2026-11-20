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
