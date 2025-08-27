use anchor_lang::prelude::*;
use anchor_spl::token::{self, Token, TokenAccount, Mint, Transfer, MintTo};

declare_id!("4kQhF3BfLPRXVN4m3YpZHrG4T5X3ZPk3JUj2L8TdN7W2");

#[program]
pub mod lp_staking {
    use super::*;

    pub fn initialize_pool(
        ctx: Context<InitializePool>,
        rewards_per_second: u64,
        min_stake_duration: i64,
    ) -> Result<()> {
        let pool = &mut ctx.accounts.staking_pool;
        
        pool.authority = ctx.accounts.authority.key();
        pool.lp_token_mint = ctx.accounts.lp_token_mint.key();
        pool.reward_token_mint = ctx.accounts.reward_token_mint.key();
        pool.reward_token_vault = ctx.accounts.reward_vault.key();
        pool.total_staked = 0;
        pool.rewards_per_second = rewards_per_second;
        pool.min_stake_duration = min_stake_duration;
        pool.last_update_time = Clock::get()?.unix_timestamp;
        pool.accumulated_rewards_per_share = 0;
        
        Ok(())
    }

    pub fn stake(ctx: Context<StakeTokens>, amount: u64) -> Result<()> {
        let pool = &mut ctx.accounts.staking_pool;
        let user_stake = &mut ctx.accounts.user_stake_account;
        let clock = Clock::get()?;
        
        // Update pool rewards
        update_pool_rewards(pool, clock.unix_timestamp)?;
        
        // Initialize user stake if first time
        if user_stake.amount == 0 {
            user_stake.user = ctx.accounts.user.key();
            user_stake.pool = pool.key();
            user_stake.stake_time = clock.unix_timestamp;
        }
        
        // Calculate pending rewards before staking
        let pending = calculate_pending_rewards(user_stake, pool)?;
        user_stake.pending_rewards = user_stake.pending_rewards
            .checked_add(pending)
            .ok_or(StakingError::MathOverflow)?;
        
        // Transfer LP tokens to pool
        // Validate program ID before CPI call
        require!(
            ctx.accounts.token_program.key() == anchor_spl::token::ID,
            StakingError::InvalidProgramId
        );
        
        let cpi_ctx = CpiContext::new(
            ctx.accounts.token_program.to_account_info(),
            Transfer {
                from: ctx.accounts.user_lp_tokens.to_account_info(),
                to: ctx.accounts.pool_lp_tokens.to_account_info(),
                authority: ctx.accounts.user.to_account_info(),
            },
        );
        token::transfer(cpi_ctx, amount)?;
        
        // **CRITICAL FIX**: Update user stake with checked arithmetic
        user_stake.amount = user_stake.amount
            .checked_add(amount)
            .ok_or(StakingError::MathOverflow)?;
        user_stake.reward_debt = user_stake.amount
            .checked_mul(pool.accumulated_rewards_per_share)
            .ok_or(StakingError::MathOverflow)?
            .checked_div(PRECISION)
            .ok_or(StakingError::MathOverflow)?;
        
        // **CRITICAL FIX**: Update pool total with checked arithmetic
        pool.total_staked = pool.total_staked
            .checked_add(amount)
            .ok_or(StakingError::MathOverflow)?;
        
        emit!(StakeEvent {
            user: ctx.accounts.user.key(),
            amount,
            total_staked: user_stake.amount,
        });
        
        Ok(())
    }

    pub fn unstake(ctx: Context<UnstakeTokens>, amount: u64) -> Result<()> {
        let pool = &mut ctx.accounts.staking_pool;
        let user_stake = &mut ctx.accounts.user_stake_account;
        let clock = Clock::get()?;
        
        // Check minimum stake duration
        require!(
            clock.unix_timestamp >= user_stake.stake_time + pool.min_stake_duration,
            StakingError::StakeDurationNotMet
        );
        
        require!(
            user_stake.amount >= amount,
            StakingError::InsufficientStake
        );
        
        // Update pool rewards
        update_pool_rewards(pool, clock.unix_timestamp)?;
        
        // Calculate pending rewards
        let pending = calculate_pending_rewards(user_stake, pool)?;
        user_stake.pending_rewards = user_stake.pending_rewards
            .checked_add(pending)
            .ok_or(StakingError::MathOverflow)?;
        
        // Transfer LP tokens back to user
        let pool_key = pool.key();
        let seeds = &[
            b"vault_authority",
            pool_key.as_ref(),
            &[ctx.bumps.vault_authority],
        ];
        let signer_seeds = &[&seeds[..]];
        
        // Validate program ID before CPI call
        require!(
            ctx.accounts.token_program.key() == anchor_spl::token::ID,
            StakingError::InvalidProgramId
        );
        
        let cpi_ctx = CpiContext::new_with_signer(
            ctx.accounts.token_program.to_account_info(),
            Transfer {
                from: ctx.accounts.pool_lp_tokens.to_account_info(),
                to: ctx.accounts.user_lp_tokens.to_account_info(),
                authority: ctx.accounts.vault_authority.to_account_info(),
            },
            signer_seeds,
        );
        token::transfer(cpi_ctx, amount)?;
        
        // Update user stake
        user_stake.amount -= amount;
        user_stake.reward_debt = user_stake.amount
            .checked_mul(pool.accumulated_rewards_per_share)
            .ok_or(StakingError::MathOverflow)?
            .checked_div(PRECISION)
            .ok_or(StakingError::MathOverflow)?;
        
        // Update pool total
        pool.total_staked -= amount;
        
        emit!(UnstakeEvent {
            user: ctx.accounts.user.key(),
            amount,
            remaining_staked: user_stake.amount,
        });
        
        Ok(())
    }

    pub fn claim_rewards(ctx: Context<ClaimRewards>) -> Result<()> {
        let pool = &mut ctx.accounts.staking_pool;
        let user_stake = &mut ctx.accounts.user_stake_account;
        let clock = Clock::get()?;
        
        // Update pool rewards
        update_pool_rewards(pool, clock.unix_timestamp)?;
        
        // Calculate total rewards
        let pending = calculate_pending_rewards(user_stake, pool)?;
        let total_rewards = user_stake.pending_rewards
            .checked_add(pending)
            .ok_or(StakingError::MathOverflow)?;
        
        require!(total_rewards > 0, StakingError::NoRewards);
        
        // Mint reward tokens to user
        let pool_key = pool.key();
        let seeds = &[
            b"reward_authority",
            pool_key.as_ref(),
            &[ctx.bumps.reward_authority],
        ];
        let signer_seeds = &[&seeds[..]];
        
        // Validate program ID before CPI call
        require!(
            ctx.accounts.token_program.key() == anchor_spl::token::ID,
            StakingError::InvalidProgramId
        );
        
        let cpi_ctx = CpiContext::new_with_signer(
            ctx.accounts.token_program.to_account_info(),
            MintTo {
                mint: ctx.accounts.reward_token_mint.to_account_info(),
                to: ctx.accounts.user_reward_tokens.to_account_info(),
                authority: ctx.accounts.reward_authority.to_account_info(),
            },
            signer_seeds,
        );
        token::mint_to(cpi_ctx, total_rewards)?;
        
        // Reset user rewards
        user_stake.pending_rewards = 0;
        user_stake.reward_debt = user_stake.amount
            .checked_mul(pool.accumulated_rewards_per_share)
            .ok_or(StakingError::MathOverflow)?
            .checked_div(PRECISION)
            .ok_or(StakingError::MathOverflow)?;
        
        emit!(ClaimEvent {
            user: ctx.accounts.user.key(),
            amount: total_rewards,
        });
        
        Ok(())
    }

    pub fn update_rewards_rate(
        ctx: Context<UpdateRewardsRate>,
        new_rewards_per_second: u64,
    ) -> Result<()> {
        let pool = &mut ctx.accounts.staking_pool;
        let clock = Clock::get()?;
        
        // Update accumulated rewards before changing rate
        update_pool_rewards(pool, clock.unix_timestamp)?;
        
        pool.rewards_per_second = new_rewards_per_second;
        
        Ok(())
    }
}

// Helper functions
fn update_pool_rewards(pool: &mut Account<StakingPool>, current_time: i64) -> Result<()> {
    // **CRITICAL FIX**: Check for division by zero
    if pool.total_staked == 0 {
        pool.last_update_time = current_time;
        return Ok(());
    }
    
    let time_elapsed = current_time - pool.last_update_time;
    if time_elapsed <= 0 {
        return Ok(());
    }
    
    // **CRITICAL FIX**: Cap time elapsed to prevent overflow (max 24 hours)
    let max_time_gap = 86400; // 24 hours in seconds
    let safe_time_elapsed = std::cmp::min(time_elapsed, max_time_gap);
    
    let rewards = (safe_time_elapsed as u64)
        .checked_mul(pool.rewards_per_second)
        .ok_or(StakingError::MathOverflow)?;
    
    // **CRITICAL FIX**: Additional validation before precision multiplication
    require!(pool.total_staked > 0, StakingError::NoStakedTokens);
    
    let reward_per_share = rewards
        .checked_mul(PRECISION)
        .ok_or(StakingError::MathOverflow)?
        .checked_div(pool.total_staked)
        .ok_or(StakingError::MathOverflow)?;
    
    pool.accumulated_rewards_per_share = pool.accumulated_rewards_per_share
        .checked_add(reward_per_share)
        .ok_or(StakingError::MathOverflow)?;
    
    pool.last_update_time = current_time;
    
    Ok(())
}

fn calculate_pending_rewards(
    user_stake: &Account<UserStakeAccount>,
    pool: &Account<StakingPool>,
) -> Result<u64> {
    if user_stake.amount == 0 {
        return Ok(0);
    }
    
    let accumulated = user_stake.amount
        .checked_mul(pool.accumulated_rewards_per_share)
        .ok_or(StakingError::MathOverflow)?
        .checked_div(PRECISION)
        .ok_or(StakingError::MathOverflow)?;
    
    if accumulated > user_stake.reward_debt {
        Ok(accumulated
            .checked_sub(user_stake.reward_debt)
            .ok_or(StakingError::MathOverflow)?)
    } else {
        Ok(0)
    }
}

// Constants
const PRECISION: u64 = 1_000_000_000_000; // 1e12

// Account structures
#[derive(Accounts)]
pub struct InitializePool<'info> {
    #[account(mut)]
    pub authority: Signer<'info>,
    
    #[account(
        init,
        payer = authority,
        space = 8 + std::mem::size_of::<StakingPool>(),
        seeds = [b"staking_pool", lp_token_mint.key().as_ref()],
        constraint = lp_token_mint.key() != Pubkey::default() @ StakingError::InvalidSeedComponent,
        bump,
        constraint = authority.lamports() >= rent.minimum_balance(8 + std::mem::size_of::<StakingPool>()) @ StakingError::InsufficientRentExemption
    )]
    pub staking_pool: Account<'info, StakingPool>,
    
    pub lp_token_mint: Account<'info, Mint>,
    pub reward_token_mint: Account<'info, Mint>,
    
    /// CHECK: Token account for LP tokens - will be initialized manually
    #[account(
        mut,
        seeds = [b"pool_lp_vault", staking_pool.key().as_ref()],
        bump
    )]
    pub pool_lp_tokens: UncheckedAccount<'info>,
    
    /// CHECK: Token account for rewards - will be initialized manually  
    #[account(
        mut,  
        seeds = [b"reward_vault", staking_pool.key().as_ref()],
        bump
    )]
    pub reward_vault: UncheckedAccount<'info>,
    
    /// CHECK: PDA for reward minting authority
    #[account(
        seeds = [b"reward_authority", staking_pool.key().as_ref()],
        bump
    )]
    pub reward_authority: UncheckedAccount<'info>,
    
    pub system_program: Program<'info, System>,
    pub token_program: Program<'info, Token>,
    pub rent: Sysvar<'info, Rent>,
}

#[derive(Accounts)]
pub struct StakeTokens<'info> {
    #[account(mut)]
    pub user: Signer<'info>,
    
    #[account(mut)]
    pub staking_pool: Account<'info, StakingPool>,
    
    #[account(
        init,
        payer = user,
        space = 8 + std::mem::size_of::<UserStakeAccount>(),
        seeds = [b"user_stake", staking_pool.key().as_ref(), user.key().as_ref()],
        constraint = staking_pool.key() != Pubkey::default() && user.key() != Pubkey::default() @ StakingError::InvalidSeedComponent,
        bump,
        constraint = user.lamports() >= Rent::get()?.minimum_balance(8 + std::mem::size_of::<UserStakeAccount>()) @ StakingError::InsufficientRentExemption
    )]
    pub user_stake_account: Account<'info, UserStakeAccount>,
    
    #[account(
        mut,
        constraint = user_lp_tokens.owner == user.key(),
        constraint = user_lp_tokens.mint == staking_pool.lp_token_mint
    )]
    pub user_lp_tokens: Account<'info, TokenAccount>,
    
    #[account(
        mut,
        seeds = [b"pool_lp_vault", staking_pool.key().as_ref()],
        bump
    )]
    pub pool_lp_tokens: Account<'info, TokenAccount>,
    
    pub system_program: Program<'info, System>,
    pub token_program: Program<'info, Token>,
}

#[derive(Accounts)]
pub struct UnstakeTokens<'info> {
    #[account(mut)]
    pub user: Signer<'info>,
    
    #[account(mut)]
    pub staking_pool: Account<'info, StakingPool>,
    
    #[account(
        mut,
        seeds = [b"user_stake", staking_pool.key().as_ref(), user.key().as_ref()],
        constraint = staking_pool.key() != Pubkey::default() && user.key() != Pubkey::default() @ StakingError::InvalidSeedComponent,
        bump,
        constraint = user_stake_account.user == user.key()
    )]
    pub user_stake_account: Account<'info, UserStakeAccount>,
    
    #[account(
        mut,
        constraint = user_lp_tokens.owner == user.key()
    )]
    pub user_lp_tokens: Account<'info, TokenAccount>,
    
    #[account(
        mut,
        seeds = [b"pool_lp_vault", staking_pool.key().as_ref()],
        bump
    )]
    pub pool_lp_tokens: Account<'info, TokenAccount>,
    
    /// CHECK: Vault authority PDA
    #[account(
        seeds = [b"vault_authority", staking_pool.key().as_ref()],
        bump
    )]
    pub vault_authority: UncheckedAccount<'info>,
    
    pub token_program: Program<'info, Token>,
}

#[derive(Accounts)]
pub struct ClaimRewards<'info> {
    #[account(mut)]
    pub user: Signer<'info>,
    
    #[account(mut)]
    pub staking_pool: Account<'info, StakingPool>,
    
    #[account(
        mut,
        seeds = [b"user_stake", staking_pool.key().as_ref(), user.key().as_ref()],
        constraint = staking_pool.key() != Pubkey::default() && user.key() != Pubkey::default() @ StakingError::InvalidSeedComponent,
        bump,
        constraint = user_stake_account.user == user.key()
    )]
    pub user_stake_account: Account<'info, UserStakeAccount>,
    
    #[account(mut)]
    pub reward_token_mint: Account<'info, Mint>,
    
    #[account(
        mut,
        constraint = user_reward_tokens.owner == user.key()
    )]
    pub user_reward_tokens: Account<'info, TokenAccount>,
    
    /// CHECK: PDA for reward minting
    #[account(
        seeds = [b"reward_authority", staking_pool.key().as_ref()],
        bump
    )]
    pub reward_authority: UncheckedAccount<'info>,
    
    pub token_program: Program<'info, Token>,
}

#[derive(Accounts)]
pub struct UpdateRewardsRate<'info> {
    #[account(
        constraint = authority.key() == staking_pool.authority
    )]
    pub authority: Signer<'info>,
    
    #[account(mut)]
    pub staking_pool: Account<'info, StakingPool>,
}

// State accounts
#[account]
pub struct StakingPool {
    pub authority: Pubkey,
    pub lp_token_mint: Pubkey,
    pub reward_token_mint: Pubkey,
    pub reward_token_vault: Pubkey,
    pub total_staked: u64,
    pub rewards_per_second: u64,
    pub min_stake_duration: i64,
    pub last_update_time: i64,
    pub accumulated_rewards_per_share: u64,
}

#[account]
pub struct UserStakeAccount {
    pub user: Pubkey,
    pub pool: Pubkey,
    pub amount: u64,
    pub stake_time: i64,
    pub reward_debt: u64,
    pub pending_rewards: u64,
}

// Events
#[event]
pub struct StakeEvent {
    pub user: Pubkey,
    pub amount: u64,
    pub total_staked: u64,
}

#[event]
pub struct UnstakeEvent {
    pub user: Pubkey,
    pub amount: u64,
    pub remaining_staked: u64,
}

#[event]
pub struct ClaimEvent {
    pub user: Pubkey,
    pub amount: u64,
}

// Errors
#[error_code]
pub enum StakingError {
    #[msg("Minimum stake duration not met")]
    StakeDurationNotMet,
    #[msg("Insufficient stake amount")]
    InsufficientStake,
    #[msg("No rewards to claim")]
    NoRewards,
    #[msg("Invalid rewards rate")]
    InvalidRewardsRate,
    #[msg("Insufficient rent exemption for account creation")]
    InsufficientRentExemption,
    #[msg("Invalid program ID in cross-program invocation")]
    InvalidProgramId,
    #[msg("Invalid seed component in PDA derivation")]
    InvalidSeedComponent,
    #[msg("Math overflow")]
    MathOverflow,
    #[msg("No tokens staked in pool")]
    NoStakedTokens,
}