//! Client code for a Token-2022 remittance stablecoin, run against LiteSVM in the tests.
//! Every read of mint or account state goes through `StateWithExtensions` (task 3).
pub mod confidential;
pub mod env;
pub mod read;
pub mod stablecoin;
