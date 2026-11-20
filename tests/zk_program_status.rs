//! Where the confidential lifecycle can run. On 20 Nov 2026 devnet and mainnet both have
//! `disable_zk_elgamal_proof_program` (zkdoVwnSFnSLtGJG7irJPEYUpmb4i7sGMGcnN6T9rnC) active
//! and `reenable_zk_elgamal_proof_program` (zkesAyFB19sTkX8i9ReoKaMNDA4YNTPYJpZKPDt7FMW)
//! inactive (`solana feature status <id> --url devnet|mainnet-beta`). The proof program
//! refuses every instruction in that state, so tasks 5 and 6 run in LiteSVM, where every
//! feature (including the re-enable one) is active.
use {
    solana_signer::Signer,
    solana_zk_sdk::{
        encryption::elgamal::ElGamalKeypair,
        zk_elgamal_proof_program::{instruction::ProofInstruction, proof_data::PubkeyValidityProofData},
    },
    token22_ct::env::Env,
};

fn verify_pubkey_validity(env: &mut Env) -> Result<(), String> {
    let payer = env.wallet();
    let proof = PubkeyValidityProofData::new(&ElGamalKeypair::new_rand()).unwrap();
    let ix = ProofInstruction::VerifyPubkeyValidity.encode_verify_proof(None, &proof);
    env.send(&[ix], &payer, &[]).map(|_| ()).map(|_| { let _ = payer.pubkey(); })
}

#[test]
fn zk_proof_program_runs_when_reenable_feature_is_active() {
    verify_pubkey_validity(&mut Env::new()).unwrap();
}

#[test]
fn zk_proof_program_is_disabled_with_todays_devnet_feature_set() {
    let err = verify_pubkey_validity(&mut Env::with_devnet_zk_features()).expect_err("must fail");
    assert!(err.contains("zk-elgamal-proof program is temporarily disabled"), "{err}");
    assert!(err.contains("InvalidInstructionData"), "{err}");
}

#[test]
fn litesvm_bundled_token2022_lacks_zk_ops_but_the_deployed_program_has_it() {
    use token22_ct::{
        confidential::{add_ct_space, approve, configure, deposit, CtUser},
        stablecoin::{create_ata, create_mint, kyc_thaw, mint_to, ConfidentialParams, MintParams, ONE},
    };
    // Same setup on two Token-2022 binaries; only Deposit differs.
    for (label, mut env) in [("bundled v10.0.0", Env::with_bundled_token2022()), ("mainnet v11.1.0", Env::new())] {
        let issuer = env.wallet();
        let mint = solana_keypair::Keypair::new();
        let mut p = MintParams::remittance_usd();
        p.confidential = Some(ConfidentialParams {
            auto_approve_new_accounts: false,
            auditor: None,
            withheld_fee_authority: *ElGamalKeypair::new_rand().pubkey(),
        });
        create_mint(&mut env, &issuer, &mint, &p).unwrap();
        let owner = env.wallet();
        let acct = create_ata(&mut env, &owner, &owner.pubkey(), &mint.pubkey()).unwrap();
        kyc_thaw(&mut env, &issuer, &acct, &mint.pubkey()).unwrap();
        let user = CtUser::new(owner.insecure_clone(), acct);
        add_ct_space(&mut env, &user).unwrap();
        configure(&mut env, &user, &mint.pubkey(), &owner).unwrap(); // not gated by zk-ops
        approve(&mut env, &issuer, &acct, &mint.pubkey()).unwrap();
        mint_to(&mut env, &issuer, &mint.pubkey(), &acct, ONE).unwrap();
        let res = deposit(&mut env, &user, &mint.pubkey(), ONE);
        println!("{label}: deposit -> {}", res.as_ref().map(|_| "ok".to_string()).unwrap_or_else(|e| e.lines().next().unwrap().to_string()));
        if label.starts_with("bundled") {
            assert!(res.unwrap_err().contains("InvalidInstructionData"));
        } else {
            res.unwrap();
        }
    }
}
