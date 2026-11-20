use {
    litesvm::LiteSVM,
    solana_keypair::Keypair,
    solana_message::Message,
    solana_signer::Signer,
    solana_transaction::Transaction,
    solana_zk_sdk::{
        encryption::{auth_encryption::AeKey, elgamal::ElGamalKeypair},
        zk_elgamal_proof_program::proof_data::PubkeyValidityProofData,
    },
    spl_token_2022_interface::{
        extension::{confidential_transfer::instruction as ct, ExtensionType},
        instruction as token_ix,
        state::{Account, Mint},
    },
    spl_token_confidential_transfer_proof_extraction::instruction::ProofLocation,
};

#[test]
fn zk_elgamal_proof_program_works_in_litesvm() {
    let mut svm = LiteSVM::new();
    let payer = Keypair::new();
    svm.airdrop(&payer.pubkey(), 10_000_000_000).unwrap();
    let pid = spl_token_2022_interface::id();

    let mint = Keypair::new();
    let mint_len = ExtensionType::try_calculate_account_len::<Mint>(&[ExtensionType::ConfidentialTransferMint]).unwrap();
    let ixs = vec![
        solana_system_interface::instruction::create_account(&payer.pubkey(), &mint.pubkey(), svm.minimum_balance_for_rent_exemption(mint_len), mint_len as u64, &pid),
        ct::initialize_mint(&pid, &mint.pubkey(), Some(payer.pubkey()), true, None).unwrap(),
        token_ix::initialize_mint2(&pid, &mint.pubkey(), &payer.pubkey(), None, 2).unwrap(),
    ];
    let tx = Transaction::new(&[&payer, &mint], Message::new(&ixs, Some(&payer.pubkey())), svm.latest_blockhash());
    svm.send_transaction(tx).unwrap();

    let acct = Keypair::new();
    let acct_len = ExtensionType::try_calculate_account_len::<Account>(&[ExtensionType::ConfidentialTransferAccount]).unwrap();
    let ixs = vec![
        solana_system_interface::instruction::create_account(&payer.pubkey(), &acct.pubkey(), svm.minimum_balance_for_rent_exemption(acct_len), acct_len as u64, &pid),
        token_ix::initialize_account3(&pid, &acct.pubkey(), &mint.pubkey(), &payer.pubkey()).unwrap(),
    ];
    let tx = Transaction::new(&[&payer, &acct], Message::new(&ixs, Some(&payer.pubkey())), svm.latest_blockhash());
    svm.send_transaction(tx).unwrap();

    let elgamal = ElGamalKeypair::new_rand();
    let ae = AeKey::new_rand();
    let proof = PubkeyValidityProofData::new(&elgamal).unwrap();
    let ixs = ct::configure_account(
        &pid, &acct.pubkey(), &mint.pubkey(), &ae.encrypt(0).into(), 65536, &payer.pubkey(), &[],
        ProofLocation::InstructionOffset(1.try_into().unwrap(), &proof),
    ).unwrap();
    let tx = Transaction::new(&[&payer], Message::new(&ixs, Some(&payer.pubkey())), svm.latest_blockhash());
    let res = svm.send_transaction(tx);
    match &res {
        Ok(m) => println!("CONFIGURE OK, CU = {}\n{}", m.compute_units_consumed, m.logs.join("\n")),
        Err(e) => println!("CONFIGURE FAILED: {:?}\n{}", e.err, e.meta.logs.join("\n")),
    }
    assert!(res.is_ok());
}
