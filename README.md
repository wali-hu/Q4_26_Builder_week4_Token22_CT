# Turbin3 Week 4: A Token-2022 Remittance Stablecoin with Confidential Transfers

A Rust client for a remittance stablecoin ("Remit USD", rUSD) built entirely from Token-2022 extensions, tested end to end with LiteSVM:

- a **protocol fee** on every transfer, collected by the issuer,
- **frozen-by-default** accounts until KYC clears,
- **on-chain metadata** inside the mint,
- a **close authority** to decommission the mint,
- then a re-issued mint that adds **confidential transfers** (hidden amounts) and a **permanent delegate** (seizure), plus a test-backed answer to what happens when the two collide.

**30 tests pass** with LiteSVM against the Token-2022 binary deployed on mainnet (program@v11.1.0). Full output: [`test-output.txt`](test-output.txt).

## Where this runs, and why not on devnet (checked 20 Nov 2026)

Confidential transfers depend on the **ZK ElGamal proof program**, and today it is switched off on the public clusters:

| Feature | devnet | mainnet-beta |
|---|---|---|
| `disable_zk_elgamal_proof_program` (`zkdoVwnSFnSLtGJG7irJPEYUpmb4i7sGMGcnN6T9rnC`) | active | active |
| `reenable_zk_elgamal_proof_program` (`zkesAyFB19sTkX8i9ReoKaMNDA4YNTPYJpZKPDt7FMW`) | inactive | inactive |

Checked with `solana feature status <id> --url devnet|mainnet-beta`. The proof program (agave 3.1.14 source) refuses every instruction with `zk-elgamal-proof program is temporarily disabled` while the first is active and the second is not. Since `ConfigureAccount` itself needs a proof, a confidential account cannot even be configured on devnet today.

So everything runs in **LiteSVM**, which activates every feature, including the re-enable one. Two tests make this explicit:
- `zk_proof_program_is_disabled_with_todays_devnet_feature_set`: the same proof fails with the devnet feature pair.
- `zk_proof_program_runs_when_reenable_feature_is_active`: it passes with LiteSVM's default.

**The Token-2022 binary matters too.** LiteSVM 0.10 ships Token-2022 v10.0.0 built **without** the `zk-ops` feature: `Deposit`, `Withdraw`, `Transfer`, `ApplyPendingBalance` and `TransferWithFee` return `InvalidInstructionData` there (`ConfigureAccount` still works). The program actually deployed on devnet and mainnet is **program@v11.1.0 with `zk-ops`**. I dumped it read-only (`solana program dump TokenzQdBNbLqP5VEhdkAS6EPFLC1PHnBqCXEpPxuEb ... --url mainnet-beta`, sha256 `15a2e615...42e2d`) into `programs/` and every test loads it. Test: `litesvm_bundled_token2022_lacks_zk_ops_but_the_deployed_program_has_it`.

## Run it

```bash
cargo test                      # all tests, LiteSVM, about 1 minute after the first build
cargo test --test task5_6_confidential -- --nocapture
```

No validator, no devnet transactions, no keypair files.

## Code layout

| File | What it does |
|---|---|
| `src/stablecoin.rs` | tasks 1, 2, 4: one-transaction mint creation (and task 5 with more extensions), fee transfers, fee harvest/withdraw, freeze and KYC thaw |
| `src/read.rs` | task 3: every read through `StateWithExtensions::unpack` + `get_extension` |
| `src/confidential.rs` | tasks 5, 6: configure, approve, deposit, apply pending, withdraw, transfer with fee, proof context accounts |
| `src/env.rs` | LiteSVM wrapper, the mainnet Token-2022 binary, the devnet ZK feature set |
| `tests/` | one file per task, every failure test asserts its exact error |

---

## Task 1: the mint, in one transaction

| Extension | Why the stablecoin needs it |
|---|---|
| TransferFeeConfig | protocol fee on every transfer (issuer revenue): 50 bps, capped at 5 rUSD |
| MetadataPointer (to the mint itself) | wallets read name/symbol/uri from the mint, no off-chain registry to trust |
| TokenMetadata | the metadata itself, stored inside the mint |
| DefaultAccountState = Frozen | every new account starts frozen until KYC |
| MintCloseAuthority | the issuer can close the mint once supply is zero |

**Instruction order** (one transaction, `src/stablecoin.rs::create_mint`):

1. `CreateAccount`: space = `ExtensionType::try_calculate_account_len::<Mint>(&[TransferFeeConfig, MetadataPointer, DefaultAccountState, MintCloseAuthority])`, lamports = rent for the **final** size including the metadata, owner = Token-2022.
2. `InitializeTransferFeeConfig`
3. `InitializeMetadataPointer` (metadata address = the mint)
4. `InitializeDefaultAccountState(Frozen)`
5. `InitializeMintCloseAuthority`
6. `InitializeMint2` (with a freeze authority; DefaultAccountState(Frozen) requires one)
7. `TokenMetadataInitialize` (name, symbol, uri)

**Why lamports are paid up front:** `CreateAccount` allocates only the fixed part (387 bytes). The metadata init then grows the account itself, but it cannot take lamports from anyone, so the account must already hold rent for the final size.

**How the metadata space is computed:** a Token-2022 TLV entry is a 2-byte extension type + a 2-byte length + the value, so metadata space = `4 + TokenMetadata::get_packed_len()` = **126 bytes**. `TokenMetadata::tlv_size_of()` returns 134, because it assumes the generic 8-byte discriminator + 4-byte length header: 8 bytes too many (harmless, but not exact). The test asserts the exact final size.

What breaks (tests): extension init **after** `InitializeMint2` makes `InitializeMint2` itself fail (`InvalidAccountData`: the account was sized for an extension that was never initialized). An account sized for 3 extensions fails on the 4th init (`InvalidAccountData`). Closing a mint with supply fails with `MintHasSupply` (28); with zero supply the close authority gets the rent back.

## Account sizes and rent

| Account | Bytes | Extensions | Rent on devnet today | Rent in LiteSVM |
|---|---|---|---|---|
| Task 1 mint | 513 | fee, pointer, default state, close, metadata | 3,256,280 | 4,461,360 |
| Token account (ATA) | 182 | ImmutableOwner, TransferFeeAmount | 1,574,800 | |
| Task 5 mint | 751 | + PermanentDelegate, ConfidentialTransferMint, ConfidentialTransferFeeConfig | 4,465,320 | |
| Confidential token account | 549 | + ConfidentialTransferAccount, ConfidentialTransferFeeAmount | 3,439,160 | |

Devnet rent from `solana rent <bytes> --url devnet` (= (128 + bytes) x 5,080). LiteSVM 0.10 still uses the older 6,960 lamports per byte.

## Task 2: transfer with the fee

```
fee = min( ceil(amount * fee_bps / 10_000), maximum_fee )
```

**Worked example** (50 bps, max 5 rUSD, 6 decimals): sending 100 rUSD = 100,000,000 base units: fee = ceil(100,000,000 x 50 / 10,000) = **500,000** (0.5 rUSD). The source pays 100 rUSD, the destination receives **99.5 rUSD**, and 0.5 rUSD is **withheld inside the destination account**. Sending 10,000 rUSD: 0.5% is 50 rUSD, capped at **5 rUSD**.

`transfer_with_fee` reads the mint through `StateWithExtensions`, takes the **current epoch** from the Clock sysvar, and calls `TransferFeeConfig::calculate_epoch_fee(epoch, amount)`, then sends `transfer_checked_with_fee`, which fails with `FeeMismatch` (32) if the fee is wrong. Why never cache the rate: `SetTransferFee` only takes effect two epochs later, so the config holds an older and a newer fee. The test changes the fee to 1%, shows the old 0.5% is still the right one in the same epoch (using 1% fails with 32), then moves the clock 2 epochs and the same function charges 1%.

**Issuer revenue:** withheld fees sit in each recipient's account. `HarvestWithheldTokensToMint` (anyone can call it) moves them into the mint; then only the withdraw-withheld authority can `WithdrawWithheldTokensFromMint` to a treasury (anyone else: `OwnerMismatch`, 4). Test: 0.5 + 1.0 rUSD harvested and withdrawn to the treasury.

## Task 3: reading state

All reads use `StateWithExtensions::<Mint>::unpack` / `StateWithExtensions::<Account>::unpack` and `get_extension::<T>()` (`get_variable_len_extension::<TokenMetadata>()` for the metadata). A raw `Mint::unpack` expects exactly 82 bytes and fails on this 513-byte mint (tested), and would ignore every extension even if it did not.

## Task 4: frozen until KYC

- `DefaultAccountState(Frozen)`: every new account, including ATAs created by anyone, starts frozen.
- A frozen account cannot receive (mint or transfer into it: `AccountFrozen`, 17) and cannot send (17).
- After KYC the **freeze authority thaws that one account** (`ThawAccount`). The mint's default stays Frozen: the next new account is frozen again (tested).
- Only the freeze authority can thaw (`OwnerMismatch`, 4).

---

## Task 5: the re-issued mint

Confidential transfers cannot be added to an existing mint, so the issuer creates a new one: the same five extensions as task 1, plus:

| Added | Setting |
|---|---|
| PermanentDelegate | the seizure authority (a separate key) |
| ConfidentialTransferMint | `auto_approve_new_accounts = false` (manual approve policy), an auditor ElGamal key |
| ConfidentialTransferFeeConfig | ElGamal key that can decrypt fees withheld from confidential transfers |

**Extensions that need care together** (checked in the Token-2022 v11.1.0 source):
- **Transfer fee + confidential transfers** needs `ConfidentialTransferFeeConfig`, and confidential transfers must use the with-fee path (5 proofs instead of 3). `InitializeMint` does **not** enforce this: the mint is created fine without it, and the first confidential transfer fails when it looks the extension up. Easy to miss.
- **DefaultAccountState(Frozen) + confidential:** Deposit, Withdraw and Transfer all check `is_frozen`, so an account must be thawed (KYC) before it can use confidential balances. `ConfigureAccount` does not check freeze.
- **PermanentDelegate + freeze:** the frozen check runs **before** the delegate check, so the delegate cannot move funds out of a frozen account (task 7).
- **PermanentDelegate + confidential:** the delegate has no power over encrypted balances (task 8).

## Task 6: the confidential lifecycle

```
            public balance (amount field, visible to all)
                 |  Deposit (amount is public in this instruction)
                 v
   +------------------------+          +------------------------+
   |  PENDING  (encrypted)  |  ------> | AVAILABLE (encrypted)  |
   |  incoming deposits and |  Apply   | spendable; owner keeps |
   |  transfers land here   |  Pending | an AES copy to read it |
   +------------------------+  Balance +------------------------+
                 ^                          |              |
                 |   confidential Transfer  |              |  Withdraw
                 +---- (other account) <----+              v
                                                  public balance again
```

**Pending vs available:** anyone can send to your account at any time, so incoming amounts land in a separate pending balance; only the owner moves them to available (`ApplyPendingBalance`), giving the new AES-encrypted balance and the credit counter they saw. That way a sender can never change the ciphertext the owner is spending from. Withdraw spends **available** only: right after a deposit, available is 0, so a withdraw proof cannot even be generated (`NotEnoughFunds`), and a proof built on a claimed balance is rejected on-chain (`proof_verification failed: SigmaProof(Equality, AlgebraicRelation)`). After applying pending, the same withdraw works (tested).

| Step | Who signs | Proofs |
|---|---|---|
| Create ATA | anyone | none |
| Reallocate (+ CT extensions) | owner | none |
| ConfigureAccount | **owner only** (stranger: `OwnerMismatch`, 4) | PubkeyValidity (in the same tx) |
| ApproveAccount | CT authority (owner: `MissingRequiredSignature`); before approval, Deposit fails with `ConfidentialTransferAccountNotApproved` (24) | none |
| Deposit | owner | none |
| ApplyPendingBalance | owner | none |
| Transfer (fee mint) | owner | equality, 3-handle ciphertext validity, percentage-with-cap, 2-handle fee validity, 256-bit range |
| Withdraw | owner | equality, 64-bit range |

**Context state accounts:** each proof is verified first into its own account owned by the ZK program, then the Token-2022 instruction reads the verified context, then the accounts are closed and their rent returned. The 256-bit range proof data is **1,064 bytes**, which cannot fit in a 1,232-byte transaction, so it is verified from a record account (the ZK program reads a proof from any account at an offset; on a real cluster that account is written in chunks with the SPL Record program, in the test the bytes are placed directly). Verifying the U256 range proof costs 368,000 CU, so verify transactions raise the compute limit.

**Test result** (`task6_full_confidential_lifecycle_with_fee_and_auditor`): Alice deposits 500 and applies; sends Bob 200 confidentially with the 0.5% fee (1 rUSD); Alice's available is 300, Bob's pending is 199; **both public balances did not move**; the auditor decrypts the transfer amount (200) from the auditor ciphertexts; Bob applies, withdraws 150 to public, keeps 49 confidential.

## Task 7: seizure with the permanent delegate

The delegate signs a `transfer_checked_with_fee` from the sanctioned account to the issuer's treasury, without the owner. On a **frozen** account this fails with `AccountFrozen` (17), so the issuer does thaw, seize and freeze again **in one transaction** (tested; the account ends frozen and empty).

## The gap, and the written finding

**Question:** what happens if a sanctioned user moves their balance into the confidential system before the permanent delegate acts?

**Answer (tested, `task8_*`):** the money is out of the delegate's reach.
1. After Deposit + ApplyPendingBalance the public balance is 0, so the delegate's seizure transfer fails with `InsufficientFunds` (1).
2. The delegate cannot touch the encrypted balance at all. In Token-2022 v11.1.0, confidential `Withdraw` and `Transfer` call `validate_owner` against the **account owner only**; the permanent delegate path exists only in the public `Transfer` and `Burn`. The test even hands the delegate **valid proofs** (built with the user's keys, which a real issuer would not have): `OwnerMismatch` (4).
3. Withdrawing or transferring also needs the user's ElGamal secret key and AES key to build the proofs. The issuer does not have them.

**What the issuer can still do:**
- **Freeze** the account: Withdraw and Transfer then fail with `AccountFrozen` (17). The value is **locked, not seized**: it stays in the account, encrypted (tested).
- **Read, not move:** the auditor ElGamal key decrypts every confidential **transfer amount** (tested), and the withheld-fee key decrypts fees. Balances are encrypted to the owner's key, so the auditor sees flows, not the balance.
- **Act before the money moves:** manual approval (`auto_approve_new_accounts = false`) means only accounts the issuer approved can use confidential balances at all, so KYC'd users only; and seizing public balances happens before any deposit.
- **Off-chain:** a court order or the user's cooperation to withdraw.

**The gap:** the regulator wants seizure; confidential transfers give the owner exclusive control of encrypted value. On the same mint, the permanent delegate can seize only what is public. Freeze closes the exit but cannot transfer the value. Closing the gap would need a protocol change (an authority allowed to withdraw from a confidential balance with an auditor-held key), which is exactly what privacy users do not want.

## Tests

| File | Tests |
|---|---|
| `task1_mint.rs` (4) | four extensions + metadata in one tx, exact size and rent; extension init after InitializeMint fails; account too small fails; close only with zero supply (28) |
| `task2_fee.rs` (5) | epoch fee and withholding; fee cap; wrong fee (32); fee change applies two epochs later; harvest and withdraw (4 for non-authority) |
| `task3_read.rs` (2) | raw unpack fails, StateWithExtensions works; ATA extensions |
| `task4_kyc.rs` (5) | frozen by default; cannot receive (17); cannot send (17); thaw one, default stays Frozen; only freeze authority (4) |
| `task5_6_confidential.rs` (5) | re-issued mint; configure owner-only (4); approval required (24); withdraw before apply fails; full lifecycle with fee and auditor |
| `task7_8_seizure.rs` (4) | delegate seizes public balance; frozen account needs thaw-seize-freeze in one tx (17); funds in confidential escape the delegate (1, 4); freeze locks but does not seize (17) |
| `zk_program_status.rs` (3) | proof program disabled with devnet features; enabled in LiteSVM; bundled v10.0.0 lacks zk-ops, mainnet v11.1.0 has it |
| `probe.rs`, `sizes.rs` (2) | first proof check; prints every size in this README |

## Not done

The optional extension (an Anchor program acting through an `Approve` delegate, then CPI Guard) is not included.

## Security notes

- Every ElGamal and AES key in the tests is random; a real wallet derives both from a signature by the owner (`ElGamalKeypair::new_from_signer`, `AeKey::new_from_signer`).
- No keypair files exist or are committed.
- Not audited; LiteSVM only.
