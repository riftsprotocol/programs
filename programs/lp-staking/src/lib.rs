use anchor_lang::prelude::*;
use anchor_spl::token::{self, Token, TokenAccount, Mint, Transfer};
use anchor_lang::solana_program::program_option::COption;

declare_id!("9dNaLvEDeq3mo4TS2GDuJTeYQqz7GdeKYnyGmcKcWCr2");

// **SECURITY FIX**: Define precision constant for reward calculations
const PRECISION: u64 = 1_000_000_000_000; // 1e12 for high precision math

#[program]
pub mod lp_staking {
    use super::*;

    pub fn initialize_pool(
        ctx: Context<InitializePool>,
        rewards_per_second: u64,
        min_stake_duration: i64,
        rifts_protocol: Pubkey,
    ) -> Result<()> {
        let pool = &mut ctx.accounts.staking_pool;
        
        pool.authority = ctx.accounts.authority.key();
        pool.lp_token_mint = ctx.accounts.lp_token_mint.key();
        pool.reward_token_mint = ctx.accounts.reward_token_mint.key();
        pool.reward_token_vault = ctx.accounts.reward_vault.key();
        pool.total_staked = 0;
        
        // **SECURITY FIX**: Validate rewards_per_second bounds to prevent economic attacks
        require!(
            rewards_per_second > 0 && rewards_per_second <= 1_000_000_000_000, // Max 1M tokens/second
            StakingError::InvalidRewardsRate
        );
        pool.rewards_per_second = rewards_per_second;
        
        // **SECURITY FIX**: Validate minimum stake duration
        require!(
            min_stake_duration >= 0 && min_stake_duration <= 365 * 24 * 3600, // Max 1 year
            StakingError::InvalidStakeDuration
        );
        pool.min_stake_duration = min_stake_duration;
        pool.last_update_time = Clock::get()?.unix_timestamp;
        pool.is_paused = false; // **CRITICAL FIX**: Initialize pause state
        pool.accumulated_rewards_per_share = 0;
        pool.rifts_protocol = rifts_protocol; // Set authorized RIFTS protocol
        pool.total_rewards_available = 0;
        pool.last_reward_deposit = 0;
        
        Ok(())
    }

    pub fn stake(ctx: Context<StakeTokens>, amount: u64) -> Result<()> {
        let pool = &mut ctx.accounts.staking_pool;
        
        // **CRITICAL FIX**: Check if pool is paused
        require!(!pool.is_paused, StakingError::PoolPaused);
        
        let user_stake = &mut ctx.accounts.user_stake_account;
        let clock = Clock::get()?;
        
        // Update pool rewards
        update_pool_rewards(pool, clock.unix_timestamp)?;
        
        // **SECURITY FIX**: Initialize user stake with additional validation
        if user_stake.amount == 0 {
            // Ensure account is properly zeroed on first initialization
            require!(
                user_stake.user == Pubkey::default(),
                StakingError::AccountAlreadyInitialized
            );
            user_stake.user = ctx.accounts.user.key();
            user_stake.pool = pool.key();
            user_stake.stake_time = clock.unix_timestamp;
        } else {
            // Validate account ownership for existing stakes
            require!(
                user_stake.user == ctx.accounts.user.key(),
                StakingError::UnauthorizedAccess
            );
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
        
        // **CRITICAL REENTRANCY FIX**: Update state BEFORE CPI call (checks-effects-interactions pattern)

        // **CRITICAL FIX**: Update user stake with checked arithmetic FIRST
        let new_user_amount = user_stake.amount
            .checked_add(amount)
            .ok_or(StakingError::MathOverflow)?;
        let new_reward_debt = (u128::from(new_user_amount))
            .checked_mul(pool.accumulated_rewards_per_share)
            .ok_or(StakingError::MathOverflow)?
            .checked_div(u128::from(PRECISION))
            .ok_or(StakingError::MathOverflow)?
            .try_into()
            .map_err(|_| StakingError::MathOverflow)?;

        // **CRITICAL FIX**: Update pool total with checked arithmetic FIRST
        let new_pool_total = pool.total_staked
            .checked_add(amount)
            .ok_or(StakingError::MathOverflow)?;

        // Update state variables (effects)
        user_stake.amount = new_user_amount;
        user_stake.reward_debt = new_reward_debt;
        pool.total_staked = new_pool_total;

        // Transfer LP tokens from user to pool vault (interactions)
        let cpi_ctx = CpiContext::new(
            ctx.accounts.token_program.to_account_info(),
            Transfer {
                from: ctx.accounts.user_lp_tokens.to_account_info(),
                to: ctx.accounts.pool_lp_tokens.to_account_info(),
                authority: ctx.accounts.user.to_account_info(),
            },
        );
        token::transfer(cpi_ctx, amount)?;
        
        emit!(StakeEvent {
            user: ctx.accounts.user.key(),
            amount,
            total_staked: user_stake.amount,
        });
        
        Ok(())
    }

    pub fn unstake(ctx: Context<UnstakeTokens>, amount: u64) -> Result<()> {
        let pool = &mut ctx.accounts.staking_pool;
        
        // **CRITICAL FIX**: Check if pool is paused
        require!(!pool.is_paused, StakingError::PoolPaused);
        
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
        
        // **CRITICAL REENTRANCY FIX**: Update state BEFORE CPI call (checks-effects-interactions pattern)

        // **CRITICAL FIX**: Update user stake with checked arithmetic FIRST
        let new_user_amount = user_stake.amount
            .checked_sub(amount)
            .ok_or(StakingError::MathOverflow)?;
        let new_reward_debt = (u128::from(new_user_amount))
            .checked_mul(pool.accumulated_rewards_per_share)
            .ok_or(StakingError::MathOverflow)?
            .checked_div(u128::from(PRECISION))
            .ok_or(StakingError::MathOverflow)?
            .try_into()
            .map_err(|_| StakingError::MathOverflow)?;

        // **CRITICAL FIX**: Update pool total with checked arithmetic FIRST
        let new_pool_total = pool.total_staked
            .checked_sub(amount)
            .ok_or(StakingError::MathOverflow)?;

        // Update state variables (effects)
        user_stake.amount = new_user_amount;
        user_stake.reward_debt = new_reward_debt;
        pool.total_staked = new_pool_total;

        // Transfer LP tokens from pool vault to user (interactions)
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
        
        emit!(UnstakeEvent {
            user: ctx.accounts.user.key(),
            amount,
            remaining_staked: user_stake.amount,
        });
        
        Ok(())
    }

    /// Deposit RIFTS rewards from the fee distribution system
    /// This allows the RIFTS protocol to send actual tokens to be distributed to stakers
    pub fn deposit_rewards(ctx: Context<DepositRewards>, amount: u64) -> Result<()> {
        let pool = &mut ctx.accounts.staking_pool;
        
        // Only authorized depositors (RIFTS protocol) can deposit rewards
        require!(
            ctx.accounts.depositor_authority.key() == pool.rifts_protocol,
            StakingError::UnauthorizedDepositor
        );
        
        require!(amount > 0, StakingError::InvalidAmount);
        
        // Transfer RIFTS tokens from depositor to pool reward vault
        let cpi_accounts = Transfer {
            from: ctx.accounts.depositor_token_account.to_account_info(),
            to: ctx.accounts.pool_reward_vault.to_account_info(),
            authority: ctx.accounts.depositor_authority.to_account_info(),
        };
        let cpi_program = ctx.accounts.token_program.to_account_info();
        let cpi_ctx = CpiContext::new(cpi_program, cpi_accounts);
        token::transfer(cpi_ctx, amount)?;
        
        // Update pool's available rewards
        pool.total_rewards_available = pool.total_rewards_available
            .checked_add(amount)
            .ok_or(StakingError::MathOverflow)?;
        
        // Update rewards per share if there are stakers
        if pool.total_staked > 0 {
            let rewards_per_share_increment = amount
                .checked_mul(PRECISION)
                .ok_or(StakingError::MathOverflow)?
                .checked_div(pool.total_staked)
                .ok_or(StakingError::MathOverflow)?;
                
            pool.accumulated_rewards_per_share = pool.accumulated_rewards_per_share
                .checked_add(rewards_per_share_increment.into())
                .ok_or(StakingError::MathOverflow)?;
        }
        
        pool.last_reward_deposit = Clock::get()?.unix_timestamp;
        
        emit!(RewardsDeposited {
            pool: pool.key(),
            depositor: ctx.accounts.depositor_authority.key(),
            amount,
            total_available: pool.total_rewards_available,
        });
        
        msg!("💰 Deposited {} RIFTS rewards to LP staking pool", amount);
        
        Ok(())
    }

    pub fn claim_rewards(ctx: Context<ClaimRewards>) -> Result<()> {
        let pool = &mut ctx.accounts.staking_pool;
        
        // **CRITICAL FIX**: Check if pool is paused
        require!(!pool.is_paused, StakingError::PoolPaused);
        
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
        
        // Check pool has enough rewards in vault
        require!(
            pool.total_rewards_available >= total_rewards,
            StakingError::InsufficientRewardsInVault
        );
        
        // Transfer reward tokens from vault to user (instead of minting)
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
        
        // Transfer from pool reward vault to user
        let cpi_ctx = CpiContext::new_with_signer(
            ctx.accounts.token_program.to_account_info(),
            Transfer {
                from: ctx.accounts.pool_reward_vault.to_account_info(),
                to: ctx.accounts.user_reward_tokens.to_account_info(),
                authority: ctx.accounts.reward_authority.to_account_info(),
            },
            signer_seeds,
        );
        // **CRITICAL REENTRANCY FIX**: Update state BEFORE CPI call

        // Update pool's available rewards FIRST
        let new_pool_rewards = pool.total_rewards_available
            .checked_sub(total_rewards)
            .ok_or(StakingError::MathOverflow)?;

        // Calculate new reward debt FIRST
        let new_reward_debt = (u128::from(user_stake.amount))
            .checked_mul(pool.accumulated_rewards_per_share)
            .ok_or(StakingError::MathOverflow)?
            .checked_div(u128::from(PRECISION))
            .ok_or(StakingError::MathOverflow)?
            .try_into()
            .map_err(|_| StakingError::MathOverflow)?;

        // Update state variables (effects)
        pool.total_rewards_available = new_pool_rewards;
        user_stake.pending_rewards = 0;
        user_stake.reward_debt = new_reward_debt;

        // Transfer reward tokens from vault to user (interactions)
        token::transfer(cpi_ctx, total_rewards)?;
        
        emit!(ClaimEvent {
            user: ctx.accounts.user.key(),
            amount: total_rewards,
        });
        
        Ok(())
    }

    /// Reset rewards accumulator when it approaches bounds (governance only)
    pub fn reset_rewards_accumulator(ctx: Context<ResetAccumulator>) -> Result<()> {
        let pool = &mut ctx.accounts.staking_pool;
        
        require!(!pool.is_paused, StakingError::PoolPaused);
        
        // **GOVERNANCE SAFETY**: Only allow reset when accumulator is actually large
        const RESET_THRESHOLD: u128 = (u128::MAX / 10) / 2; // 50% of max allowed
        require!(
            pool.accumulated_rewards_per_share > RESET_THRESHOLD,
            StakingError::InvalidRewardsRate
        );
        
        // Log the reset for transparency
        msg!("🔄 GOVERNANCE RESET: Rewards accumulator reset from {} to 0", 
             pool.accumulated_rewards_per_share);
        
        // Reset to zero - all pending rewards should be claimed first
        pool.accumulated_rewards_per_share = 0;
        
        emit!(AccumulatorReset {
            pool: pool.key(),
            authority: ctx.accounts.governance_authority.key(),
            reset_time: Clock::get()?.unix_timestamp,
        });
        
        Ok(())
    }

    /// **SECURITY FIX**: Emergency withdraw function for governance
    pub fn emergency_withdraw(
        ctx: Context<EmergencyWithdraw>,
        amount: u64,
    ) -> Result<()> {
        let pool = &mut ctx.accounts.staking_pool;
        let user_stake = &mut ctx.accounts.user_stake_account;
        
        // **CRITICAL FIX**: Only allow emergency withdraw when pool is paused
        require!(pool.is_paused, StakingError::PoolNotPaused);
        
        // Validate authority (governance only)
        require!(
            ctx.accounts.authority.key() == pool.authority,
            StakingError::Unauthorized
        );
        
        // Validate withdrawal amount
        require!(amount <= user_stake.amount, StakingError::InsufficientStake);
        
        // Update user stake
        user_stake.amount = user_stake.amount
            .checked_sub(amount)
            .ok_or(StakingError::MathOverflow)?;
        
        // Update pool total
        pool.total_staked = pool.total_staked
            .checked_sub(amount)
            .ok_or(StakingError::MathOverflow)?;
        
        // Transfer LP tokens back to user (emergency bypass of normal unstaking logic)
        let pool_key = pool.key();
        let seeds = &[
            b"vault_authority",
            pool_key.as_ref(),
            &[ctx.bumps.vault_authority],
        ];
        let signer_seeds = &[&seeds[..]];
        
        let cpi_ctx = CpiContext::new_with_signer(
            ctx.accounts.token_program.to_account_info(),
            token::Transfer {
                from: ctx.accounts.pool_lp_tokens.to_account_info(),
                to: ctx.accounts.user_lp_tokens.to_account_info(),
                authority: ctx.accounts.vault_authority.to_account_info(),
            },
            signer_seeds,
        );
        token::transfer(cpi_ctx, amount)?;
        
        emit!(EmergencyWithdrawEvent {
            user: ctx.accounts.user.key(),
            pool: pool.key(),
            amount,
            timestamp: Clock::get()?.unix_timestamp,
        });
        
        Ok(())
    }

    pub fn update_rewards_rate(
        ctx: Context<UpdateRewardsRate>,
        new_rewards_per_second: u64,
    ) -> Result<()> {
        let pool = &mut ctx.accounts.staking_pool;
        let clock = Clock::get()?;
        
        // **ENHANCED FIX**: Validate reward rate bounds to prevent economic attacks
        // Use actual reward token decimals with additional safety checks
        let token_decimals = ctx.accounts.reward_token_mint.decimals;
        require!(token_decimals <= 18, StakingError::InvalidRewardsRate); // Sanity check on decimals

        let max_rewards_per_second = 100_000u64 // Reduced from 1M for additional safety
            .checked_mul(10u64.pow(u32::from(token_decimals)))
            .ok_or(StakingError::MathOverflow)?;
        const MIN_REWARDS_PER_SECOND: u64 = 1; // Minimum 1 lamport per second
        
        require!(
            new_rewards_per_second >= MIN_REWARDS_PER_SECOND && 
            new_rewards_per_second <= max_rewards_per_second,
            StakingError::InvalidRewardsRate
        );
        
        // **CRITICAL FIX**: Validate reward rate doesn't exceed reasonable bounds
        let estimated_daily_rewards = new_rewards_per_second
            .checked_mul(86_400) // seconds in day
            .ok_or(StakingError::MathOverflow)?;
        
        // Additional security: prevent extremely high rates that could drain rewards quickly
        let max_daily_rewards = 100_000_000u64
            .checked_mul(10u64.pow(u32::from(token_decimals)))
            .ok_or(StakingError::MathOverflow)?; // Max 100M tokens per day
        require!(
            estimated_daily_rewards <= max_daily_rewards,
            StakingError::InvalidRewardsRate
        );
        
        // Update accumulated rewards before changing rate
        update_pool_rewards(pool, clock.unix_timestamp)?;
        
        pool.rewards_per_second = new_rewards_per_second;
        
        Ok(())
    }
}

// Re-export account types for CPI - removed duplicate export

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
    
    // **CRITICAL FIX**: Cap time elapsed to prevent overflow and timestamp manipulation
    let max_time_gap = 86400; // 24 hours in seconds
    
    // **CRITICAL FIX**: Prevent timestamp manipulation by applying bounds to ALL time deviations
    // Previous code only applied limits when deviation > 5 minutes, allowing manipulation within that window
    const MAX_TIMESTAMP_DEVIATION: i64 = 300; // 5 minutes max deviation
    const MIN_TIMESTAMP_DEVIATION: i64 = -300; // Also prevent backward manipulation
    
    // Apply bounds to ALL timestamp deviations, not just large ones
    let bounded_time_elapsed = if time_elapsed > MAX_TIMESTAMP_DEVIATION {
        MAX_TIMESTAMP_DEVIATION // Cap forward manipulation
    } else if time_elapsed < MIN_TIMESTAMP_DEVIATION {
        MIN_TIMESTAMP_DEVIATION // Cap backward manipulation
    } else {
        std::cmp::min(time_elapsed, max_time_gap) // Apply max gap to legitimate deviations too
    };
    
    let safe_time_elapsed = bounded_time_elapsed;
    
    // **CRITICAL FIX**: Use proper checked conversion instead of truncating cast
    let time_elapsed_u64 = u64::try_from(safe_time_elapsed)
        .map_err(|_| StakingError::MathOverflow)?;
    let rewards = time_elapsed_u64
        .checked_mul(pool.rewards_per_second)
        .ok_or(StakingError::MathOverflow)?;
    
    // **CRITICAL FIX**: Additional validation before precision multiplication
    require!(pool.total_staked > 0, StakingError::NoStakedTokens);

    // **ENHANCED FIX**: Add maximum limits to prevent extreme calculations
    const MAX_REWARDS_PER_UPDATE: u64 = 1_000_000_000_000; // 1 trillion base units max
    require!(rewards <= MAX_REWARDS_PER_UPDATE, StakingError::RewardsAccumulationExceeded);

    // **CRITICAL FIX**: Use u128 math with additional bounds checking
    let reward_per_share_u128 = (u128::from(rewards))
        .checked_mul(u128::from(PRECISION))
        .ok_or(StakingError::MathOverflow)?
        .checked_div(u128::from(pool.total_staked))
        .ok_or(StakingError::MathOverflow)?;

    // **ADDITIONAL SAFETY**: Prevent single reward update from being too large
    const MAX_SINGLE_REWARD_INCREMENT: u128 = u128::MAX / 100; // Max 1% of u128 space per update
    require!(
        reward_per_share_u128 <= MAX_SINGLE_REWARD_INCREMENT,
        StakingError::RewardsAccumulationExceeded
    );
    
    let new_accumulated = pool.accumulated_rewards_per_share
        .checked_add(reward_per_share_u128)
        .ok_or(StakingError::MathOverflow)?;
    
    // **ENHANCED FIX**: Prevent unbounded rewards accumulation with better bounds
    // If accumulator gets too large, it's time for a governance-controlled reset
    const MAX_ACCUMULATED_REWARDS: u128 = u128::MAX / 10; // Leave 10x safety margin for calculations
    require!(
        new_accumulated <= MAX_ACCUMULATED_REWARDS,
        StakingError::RewardsAccumulationExceeded
    );
    
    // **ADDITIONAL SAFETY**: Log warning when accumulator gets large (75% of max)
    const WARNING_THRESHOLD: u128 = (MAX_ACCUMULATED_REWARDS * 3) / 4;
    if new_accumulated > WARNING_THRESHOLD {
        msg!("⚠️  GOVERNANCE ALERT: Rewards accumulator at {}% capacity - consider reset", 
             (new_accumulated * 100 / MAX_ACCUMULATED_REWARDS));
    }
    
    pool.accumulated_rewards_per_share = new_accumulated;
    
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
    
    // **CRITICAL FIX**: Use u128 math to prevent overflow in reward calculations
    let accumulated_u128 = (u128::from(user_stake.amount))
        .checked_mul(pool.accumulated_rewards_per_share)
        .ok_or(StakingError::MathOverflow)?
        .checked_div(u128::from(PRECISION))
        .ok_or(StakingError::MathOverflow)?;
    
    let accumulated = u64::try_from(accumulated_u128)
        .map_err(|_| StakingError::MathOverflow)?;
    
    if accumulated > user_stake.reward_debt {
        Ok(accumulated
            .checked_sub(user_stake.reward_debt)
            .ok_or(StakingError::MathOverflow)?)
    } else {
        Ok(0)
    }
}

// Constants section moved to top of file

// Account structures
#[derive(Accounts)]
pub struct InitializePool<'info> {
    #[account(mut)]
    pub authority: Signer<'info>,
    
    #[account(
        init,
        payer = authority,
        space = StakingPool::INIT_SPACE,
        seeds = [b"staking_pool", lp_token_mint.key().as_ref()],
        constraint = lp_token_mint.key() != Pubkey::default() @ StakingError::InvalidSeedComponent,
        bump,
        constraint = authority.lamports() >= rent.minimum_balance(StakingPool::INIT_SPACE) @ StakingError::InsufficientRentExemption
    )]
    pub staking_pool: Account<'info, StakingPool>,
    
    pub lp_token_mint: Account<'info, Mint>,
    #[account(
        constraint = reward_token_mint.mint_authority == COption::Some(reward_authority.key()) @ StakingError::InvalidMintAuthority
    )]
    pub reward_token_mint: Account<'info, Mint>,
    
    /// LP tokens vault - properly initialized as TokenAccount
    #[account(
        init,
        payer = authority,
        token::mint = lp_token_mint,
        token::authority = vault_authority,
        seeds = [b"pool_lp_vault", staking_pool.key().as_ref()],
        bump
    )]
    pub pool_lp_tokens: Account<'info, TokenAccount>,
    
    /// Reward tokens vault - properly initialized as TokenAccount
    #[account(
        init,
        payer = authority,
        token::mint = reward_token_mint,
        token::authority = reward_authority,
        seeds = [b"reward_vault", staking_pool.key().as_ref()],
        bump
    )]
    pub reward_vault: Account<'info, TokenAccount>,
    
    /// CHECK: PDA for vault authority (controls LP vault)
    #[account(
        seeds = [b"vault_authority", staking_pool.key().as_ref()],
        bump
    )]
    pub vault_authority: UncheckedAccount<'info>,
    
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
        init_if_needed,
        payer = user,
        space = UserStakeAccount::INIT_SPACE,
        seeds = [b"user_stake", staking_pool.key().as_ref(), user.key().as_ref()],
        constraint = staking_pool.key() != Pubkey::default() && user.key() != Pubkey::default() @ StakingError::InvalidSeedComponent,
        bump,
        constraint = user.lamports() >= Rent::get()?.minimum_balance(UserStakeAccount::INIT_SPACE) @ StakingError::InsufficientRentExemption
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
        bump,
        constraint = pool_lp_tokens.mint == staking_pool.lp_token_mint @ StakingError::InvalidMint
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
        bump,
        constraint = pool_lp_tokens.mint == staking_pool.lp_token_mint @ StakingError::InvalidMint
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
    
    // **CRITICAL FIX**: Removed mint authority constraint since rewards are transferred from vault, not minted
    // The mint authority constraint was causing all reward claims to fail
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
    
    #[account(
        mut,
        constraint = pool_reward_vault.key() == staking_pool.reward_token_vault
    )]
    pub pool_reward_vault: Account<'info, TokenAccount>,
    
    pub token_program: Program<'info, Token>,
}

#[derive(Accounts)]
pub struct DepositRewards<'info> {
    /// Authority depositing rewards (must be RIFTS protocol)
    pub depositor_authority: Signer<'info>,
    
    #[account(mut)]
    pub staking_pool: Account<'info, StakingPool>,
    
    /// Token account containing RIFTS rewards to deposit
    #[account(
        mut,
        constraint = depositor_token_account.owner == depositor_authority.key()
    )]
    pub depositor_token_account: Account<'info, TokenAccount>,
    
    /// Pool's reward vault to receive the tokens
    #[account(
        mut,
        constraint = pool_reward_vault.key() == staking_pool.reward_token_vault
    )]
    pub pool_reward_vault: Account<'info, TokenAccount>,
    
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
    
    /// **CRITICAL FIX**: Added reward token mint to validate decimals
    pub reward_token_mint: Account<'info, Mint>,
}

// State accounts
impl StakingPool {
    pub const INIT_SPACE: usize = 8 + // discriminator
        32 + // authority
        32 + // lp_token_mint
        32 + // reward_token_mint  
        32 + // reward_token_vault
        8 +  // total_staked
        8 +  // rewards_per_second
        8 +  // min_stake_duration
        8 +  // last_update_time
        8 +  // accumulated_rewards_per_share
        1 +  // is_paused
        32 + // rifts_protocol
        8 +  // total_rewards_available
        8;   // last_reward_deposit
}

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
    pub accumulated_rewards_per_share: u128,
    pub is_paused: bool, // **CRITICAL FIX**: Emergency pause control
    pub rifts_protocol: Pubkey, // RIFTS protocol that can deposit rewards
    pub total_rewards_available: u64, // Total RIFTS tokens available for distribution
    pub last_reward_deposit: i64, // Timestamp of last reward deposit
}

impl UserStakeAccount {
    pub const INIT_SPACE: usize = 8 + // discriminator
        32 + // user
        32 + // pool
        8 +  // amount
        8 +  // stake_time
        8 +  // reward_debt
        8;   // pending_rewards
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

#[derive(Accounts)]
pub struct ResetAccumulator<'info> {
    #[account(mut)]
    pub governance_authority: Signer<'info>,
    
    #[account(
        mut,
        constraint = staking_pool.authority == governance_authority.key() @ StakingError::InvalidProgramId
    )]
    pub staking_pool: Account<'info, StakingPool>,
}

#[derive(Accounts)]
pub struct EmergencyWithdraw<'info> {
    #[account(mut)]
    pub authority: Signer<'info>,
    
    #[account(mut)]
    pub user: SystemAccount<'info>,
    
    #[account(mut)]
    pub staking_pool: Account<'info, StakingPool>,
    
    #[account(
        mut,
        seeds = [b"user_stake", staking_pool.key().as_ref(), user.key().as_ref()],
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
        bump,
        constraint = pool_lp_tokens.mint == staking_pool.lp_token_mint @ StakingError::InvalidMint
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

#[event]
pub struct AccumulatorReset {
    pub pool: Pubkey,
    pub authority: Pubkey,
    pub reset_time: i64,
}

#[event]
pub struct EmergencyWithdrawEvent {
    pub user: Pubkey,
    pub pool: Pubkey,
    pub amount: u64,
    pub timestamp: i64,
}

#[event]
pub struct RewardsDeposited {
    pub pool: Pubkey,
    pub depositor: Pubkey,
    pub amount: u64,
    pub total_available: u64,
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
    #[msg("Insufficient reward funds to sustain rate")]
    InsufficientRewardFunds,
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
    #[msg("Rewards accumulation exceeded maximum limit")]
    RewardsAccumulationExceeded,
    #[msg("Pool is paused for emergency maintenance")]
    PoolPaused,
    #[msg("Invalid mint authority - mint authority does not match expected PDA")]
    InvalidMintAuthority,
    #[msg("Invalid token mint")]
    InvalidMint,
    #[msg("Pool is not paused - emergency withdraw only allowed when paused")]
    PoolNotPaused,
    #[msg("Unauthorized access - only authority can perform this action")]
    Unauthorized,
    #[msg("Unauthorized depositor - only RIFTS protocol can deposit rewards")]
    UnauthorizedDepositor,
    #[msg("Invalid amount")]
    InvalidAmount,
    #[msg("Insufficient rewards in vault")]
    InsufficientRewardsInVault,
    #[msg("Invalid stake duration - maximum 1 year allowed")]
    InvalidStakeDuration,
    #[msg("Account already initialized with different user")]
    AccountAlreadyInitialized,
    #[msg("Unauthorized access to user stake account")]
    UnauthorizedAccess,
}