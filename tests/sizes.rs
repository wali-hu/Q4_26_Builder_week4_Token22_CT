//! Prints the exact sizes used in the README (no assertions beyond basic sanity).
mod common;

use {
    common::{ct_world, world},
    solana_zk_sdk::zk_elgamal_proof_program::{
        proof_data::{
            BatchedGroupedCiphertext2HandlesValidityProofContext,
            BatchedGroupedCiphertext3HandlesValidityProofContext, BatchedRangeProofContext,
            BatchedRangeProofU256Data, BatchedRangeProofU64Data, CiphertextCommitmentEqualityProofContext,
            CiphertextCommitmentEqualityProofData, PercentageWithCapProofContext,
        },
        state::ProofContextState,
    },
    token22_ct::read::{read_account, read_mint},
};

#[test]
fn sizes_of_every_account_and_proof() {
    let mut w = world();
    let (_u, plain) = w.kyc_user();
    let m1 = read_mint(&w.env, &w.mint_key()).unwrap();
    let a1 = read_account(&w.env, &plain).unwrap();

    let mut c = ct_world();
    let user = c.ct_user(0);
    let m2 = read_mint(&c.w.env, &c.w.mint_key()).unwrap();
    let a2 = read_account(&c.w.env, &user.account).unwrap();

    println!("task1 mint: {} bytes {:?}", m1.data_len, m1.extensions);
    println!("task1 token account (ATA): {} bytes {:?}", a1.data_len, a1.extensions);
    println!("task5 mint: {} bytes {:?}", m2.data_len, m2.extensions);
    println!("task6 confidential token account: {} bytes {:?}", a2.data_len, a2.extensions);
    use std::mem::size_of;
    println!("proof data: equality {} bytes, range U64 {} bytes, range U256 {} bytes",
        size_of::<CiphertextCommitmentEqualityProofData>(), size_of::<BatchedRangeProofU64Data>(), size_of::<BatchedRangeProofU256Data>());
    println!("context state accounts: equality {}, 3-handle validity {}, percentage-with-cap {}, 2-handle validity {}, range {}",
        size_of::<ProofContextState<CiphertextCommitmentEqualityProofContext>>(),
        size_of::<ProofContextState<BatchedGroupedCiphertext3HandlesValidityProofContext>>(),
        size_of::<ProofContextState<PercentageWithCapProofContext>>(),
        size_of::<ProofContextState<BatchedGroupedCiphertext2HandlesValidityProofContext>>(),
        size_of::<ProofContextState<BatchedRangeProofContext>>());
    assert!(a2.data_len > a1.data_len);
}
