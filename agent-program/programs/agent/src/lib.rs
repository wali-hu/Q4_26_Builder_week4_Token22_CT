//! Optional extension (task 9): an "agent" program that moves a user's tokens through an
//! `Approve` delegation, and two helper paths that show what Token-2022's CPI Guard blocks.
//!
//! CPI Guard (Token-2022 v11.1.0 `processor.rs`): when `lock_cpi` is on and the call comes
//! from a CPI, a transfer or burn is refused only if the AUTHORITY IS THE OWNER, and every
//! Approve is refused. A transfer signed by a delegate (this program's PDA) is not blocked.

use anchor_lang::prelude::*;
use anchor_spl::token_interface::{
    approve, transfer_checked, Approve, Mint, TokenAccount, TokenInterface, TransferChecked,
};

declare_id!("D9Tz9UR6PzAJW7ftF8g51tSW3YmL8wJkRCkrSLYxX7sL");

pub const CONFIG_SEED: &[u8] = b"config";
pub const AGENT_SEED: &[u8] = b"agent";

#[program]
pub mod agent {
    use super::*;

    /// Records who may run the agent. Without this, anyone could spend every allowance
    /// users granted to the agent PDA.
    pub fn initialize(ctx: Context<Initialize>, operator: Pubkey) -> Result<()> {
        ctx.accounts.config.set_inner(Config {
            operator,
            config_bump: ctx.bumps.config,
            agent_bump: ctx.bumps.agent,
        });
        Ok(())
    }

    /// The automation path: the operator triggers it, the agent PDA signs as the
    /// delegate the user approved. The user signs nothing here.
    pub fn delegated_transfer(ctx: Context<DelegatedTransfer>, amount: u64) -> Result<()> {
        let seeds: &[&[u8]] = &[AGENT_SEED, &[ctx.accounts.config.agent_bump]];
        transfer_checked(
            CpiContext::new_with_signer(
                ctx.accounts.token_program.key(),
                TransferChecked {
                    from: ctx.accounts.source.to_account_info(),
                    mint: ctx.accounts.mint.to_account_info(),
                    to: ctx.accounts.destination.to_account_info(),
                    authority: ctx.accounts.agent.to_account_info(),
                },
                &[seeds],
            ),
            amount,
            ctx.accounts.mint.decimals,
        )
    }

    /// For comparison: the OWNER signs the transaction and the program forwards that
    /// signature in a CPI. This is the pattern CPI Guard exists to block.
    pub fn owner_transfer_via_cpi(ctx: Context<OwnerTransfer>, amount: u64) -> Result<()> {
        transfer_checked(
            CpiContext::new(
                ctx.accounts.token_program.key(),
                TransferChecked {
                    from: ctx.accounts.source.to_account_info(),
                    mint: ctx.accounts.mint.to_account_info(),
                    to: ctx.accounts.destination.to_account_info(),
                    authority: ctx.accounts.owner.to_account_info(),
                },
            ),
            amount,
            ctx.accounts.mint.decimals,
        )
    }

    /// For comparison: the owner asks the program to grant the agent an allowance
    /// through a CPI. With CPI Guard on, every Approve in a CPI is refused.
    pub fn approve_via_cpi(ctx: Context<ApproveViaCpi>, amount: u64) -> Result<()> {
        approve(
            CpiContext::new(
                ctx.accounts.token_program.key(),
                Approve {
                    to: ctx.accounts.source.to_account_info(),
                    delegate: ctx.accounts.agent.to_account_info(),
                    authority: ctx.accounts.owner.to_account_info(),
                },
            ),
            amount,
        )
    }
}

#[account]
#[derive(InitSpace)]
pub struct Config {
    pub operator: Pubkey,
    pub config_bump: u8,
    pub agent_bump: u8,
}

#[derive(Accounts)]
pub struct Initialize<'info> {
    #[account(mut)]
    pub payer: Signer<'info>,
    #[account(
        init,
        payer = payer,
        seeds = [CONFIG_SEED],
        bump,
        space = Config::DISCRIMINATOR.len() + Config::INIT_SPACE
    )]
    pub config: Account<'info, Config>,
    /// CHECK: the agent PDA holds no data; it only signs as a delegate.
    #[account(seeds = [AGENT_SEED], bump)]
    pub agent: UncheckedAccount<'info>,
    pub system_program: Program<'info, System>,
}

#[derive(Accounts)]
pub struct DelegatedTransfer<'info> {
    pub operator: Signer<'info>,
    #[account(seeds = [CONFIG_SEED], bump = config.config_bump, has_one = operator)]
    pub config: Account<'info, Config>,
    /// CHECK: PDA that the user approved as delegate; signs via seeds.
    #[account(seeds = [AGENT_SEED], bump = config.agent_bump)]
    pub agent: UncheckedAccount<'info>,
    #[account(mut, token::mint = mint, token::token_program = token_program)]
    pub source: InterfaceAccount<'info, TokenAccount>,
    #[account(mint::token_program = token_program)]
    pub mint: InterfaceAccount<'info, Mint>,
    #[account(mut, token::mint = mint, token::token_program = token_program)]
    pub destination: InterfaceAccount<'info, TokenAccount>,
    pub token_program: Interface<'info, TokenInterface>,
}

#[derive(Accounts)]
pub struct OwnerTransfer<'info> {
    pub owner: Signer<'info>,
    #[account(mut, token::mint = mint, token::authority = owner, token::token_program = token_program)]
    pub source: InterfaceAccount<'info, TokenAccount>,
    #[account(mint::token_program = token_program)]
    pub mint: InterfaceAccount<'info, Mint>,
    #[account(mut, token::mint = mint, token::token_program = token_program)]
    pub destination: InterfaceAccount<'info, TokenAccount>,
    pub token_program: Interface<'info, TokenInterface>,
}

#[derive(Accounts)]
pub struct ApproveViaCpi<'info> {
    pub owner: Signer<'info>,
    #[account(mut, token::authority = owner, token::token_program = token_program)]
    pub source: InterfaceAccount<'info, TokenAccount>,
    /// CHECK: the agent PDA becomes the delegate.
    #[account(seeds = [AGENT_SEED], bump)]
    pub agent: UncheckedAccount<'info>,
    pub token_program: Interface<'info, TokenInterface>,
}
