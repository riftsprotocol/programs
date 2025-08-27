// Rifts Protocol - Hybrid Model: Keep 1:1 Wrap/Unwrap + Add LP Pool Trading
use anchor_lang::prelude::*;
use anchor_spl::token::{self, Token, TokenAccount, Transfer, Mint, MintTo, Burn};
use anchor_lang::solana_program::sysvar::rent::Rent;
use anchor_lang::solana_program;

// External program CPI imports
pub use fee_collector;
pub use governance;
pub use lp_staking;

declare_id!("8FX1CVcR4QZyvTYtV6rG42Ha1K2qyRNykKYcwVctspUh");

#[program]
pub mod rifts_protocol {
    use super::*;

    /// Create a new Rift with a vanity mint address (like pump.fun does with 'pump')
    /// This allows creating rifts with mint addresses ending in 'rift'
    /// KEEPS ORIGINAL 1:1 WRAP/UNWRAP FUNCTIONALITY
    pub fn create_rift_with_vanity_mint(
        ctx: Context<CreateRiftWithVanityMint>,
        burn_fee_bps: u16,
        partner_fee_bps: u16,
        partner_wallet: Option<Pubkey>,
        rift_name: Option<String>,
    ) -> Result<()> {
        let rift = &mut ctx.accounts.rift;
        
        // Validate fees (KEEP ORIGINAL)
        require!(burn_fee_bps <= 4500, ErrorCode::InvalidBurnFee);
        require!(partner_fee_bps <= 500, ErrorCode::InvalidPartnerFee);
        
        // Validate that the mint address ends with 'rift' (case insensitive)
        let mint_address = ctx.accounts.rift_mint.key().to_string();
        let lower_address = mint_address.to_lowercase();
        require!(
            lower_address.ends_with("rift") || 
            lower_address.ends_with("rifts") ||
            lower_address.ends_with("rift1") ||
            lower_address.ends_with("rift2") ||
            lower_address.ends_with("rift3") ||
            lower_address.ends_with("rift4") ||
            lower_address.ends_with("rift5") ||
            lower_address.ends_with("rift6") ||
            lower_address.ends_with("rift7") ||
            lower_address.ends_with("rift8") ||
            lower_address.ends_with("rift9"),
            ErrorCode::InvalidVanityAddress
        );
        
        // Set rift name
        if let Some(name) = rift_name {
            require!(name.len() <= 32, ErrorCode::NameTooLong);
            rift.name = name;
        } else {
            // Generate default name with RIFT suffix
            let underlying_symbol = format!("{}_RIFT", &ctx.accounts.underlying_mint.key().to_string()[0..8]);
            rift.name = underlying_symbol;
        }

        rift.creator = ctx.accounts.creator.key();
        rift.underlying_mint = ctx.accounts.underlying_mint.key();
        rift.rift_mint = ctx.accounts.rift_mint.key();
        
        // KEEP ORIGINAL VAULT SYSTEM
        let rift_key = rift.key();
        let vault_seeds = &[b"vault", rift_key.as_ref()];
        let (vault_pda, _) = Pubkey::find_program_address(vault_seeds, &crate::ID);
        rift.vault = vault_pda;
        
        // KEEP ORIGINAL FIELDS
        rift.burn_fee_bps = burn_fee_bps;
        rift.partner_fee_bps = partner_fee_bps;
        rift.partner_wallet = partner_wallet;
        rift.total_wrapped = 0;
        rift.total_burned = 0;
        rift.backing_ratio = 10000; // 1.0000x in basis points
        rift.last_rebalance = Clock::get()?.unix_timestamp;
        rift.created_at = Clock::get()?.unix_timestamp;
        
        // ADD LP POOL FIELDS (NEW)
        let pool_seeds = &[b"liquidity_pool", rift_key.as_ref()];
        let (pool_pda, _) = Pubkey::find_program_address(pool_seeds, &crate::ID);
        rift.liquidity_pool = Some(pool_pda);
        rift.lp_token_supply = 0;
        rift.pool_trading_fee_bps = 30; // 0.3% default trading fee for LP pool
        rift.total_liquidity_underlying = 0;
        rift.total_liquidity_rift = 0;
        
        // Initialize hybrid oracle system (KEEP ORIGINAL)
        rift.oracle_prices = [PriceData::default(); 10];
        rift.price_index = 0;
        rift.oracle_update_interval = 30 * 60;
        rift.max_rebalance_interval = 24 * 60 * 60;
        rift.arbitrage_threshold_bps = 200;
        rift.last_oracle_update = Clock::get()?.unix_timestamp;
        
        // Initialize advanced metrics (KEEP ORIGINAL)
        rift.total_volume_24h = 0;
        rift.price_deviation = 0;
        rift.arbitrage_opportunity_bps = 0;
        rift.rebalance_count = 0;
        
        // Initialize RIFTS token distribution tracking (KEEP ORIGINAL)
        rift.total_fees_collected = 0;
        rift.rifts_tokens_distributed = 0;
        rift.rifts_tokens_burned = 0;
        
        // Initialize LP staking (KEEP ORIGINAL)
        rift.total_lp_staked = 0;
        rift.pending_rewards = 0;
        rift.last_reward_distribution = Clock::get()?.unix_timestamp;
        
        // Initialize reentrancy protection (KEEP ORIGINAL)
        rift.reentrancy_guard = false;
        
        // Initialize emergency controls (KEEP ORIGINAL)
        rift.is_paused = false;
        rift.pause_timestamp = 0;
        
        // Initialize external program integration (KEEP ORIGINAL)
        rift.pending_fee_distribution = 0;
        
        // Initialize governance integration (KEEP ORIGINAL)
        rift.last_governance_update = Clock::get()?.unix_timestamp;
        
        emit!(RiftCreated {
            rift: rift.key(),
            creator: rift.creator,
            underlying_mint: rift.underlying_mint,
            burn_fee_bps,
            partner_fee_bps,
        });

        Ok(())
    }

    /// Initialize a new Rift (wrapped token vault) - ORIGINAL VERSION WITH 0.7% FEES
    pub fn create_rift(
        ctx: Context<CreateRift>,
        burn_fee_bps: u16,
        partner_fee_bps: u16,
        partner_wallet: Option<Pubkey>,
        rift_name: Option<String>,
    ) -> Result<()> {
        let rift = &mut ctx.accounts.rift;
        
        // Validate fees (KEEP ORIGINAL)
        require!(burn_fee_bps <= 4500, ErrorCode::InvalidBurnFee);
        require!(partner_fee_bps <= 500, ErrorCode::InvalidPartnerFee);
        
        // Validate and set rift name
        if let Some(name) = rift_name {
            require!(name.len() <= 32, ErrorCode::NameTooLong);
            require!(name.ends_with("_RIFT") || name.ends_with("RIFT"), ErrorCode::InvalidRiftName);
            rift.name = name;
        } else {
            // Generate default name with RIFT suffix
            let underlying_symbol = format!("{}_RIFT", &ctx.accounts.underlying_mint.key().to_string()[0..8]);
            rift.name = underlying_symbol;
        }

        rift.creator = ctx.accounts.creator.key();
        rift.underlying_mint = ctx.accounts.underlying_mint.key();
        rift.rift_mint = ctx.accounts.rift_mint.key();
        
        // KEEP ORIGINAL VAULT SYSTEM
        let rift_key = rift.key();
        let vault_seeds = &[b"vault", rift_key.as_ref()];
        let (vault_pda, _) = Pubkey::find_program_address(vault_seeds, &crate::ID);
        rift.vault = vault_pda;
        rift.burn_fee_bps = burn_fee_bps;
        rift.partner_fee_bps = partner_fee_bps;
        rift.partner_wallet = partner_wallet;
        rift.total_wrapped = 0;
        rift.total_burned = 0;
        rift.backing_ratio = 10000; // 1.0000x in basis points
        rift.last_rebalance = Clock::get()?.unix_timestamp;
        rift.created_at = Clock::get()?.unix_timestamp;
        
        // ADD LP POOL FIELDS (NEW)
        let pool_seeds = &[b"liquidity_pool", rift_key.as_ref()];
        let (pool_pda, _) = Pubkey::find_program_address(pool_seeds, &crate::ID);
        rift.liquidity_pool = Some(pool_pda);
        rift.lp_token_supply = 0;
        rift.pool_trading_fee_bps = 30; // 0.3% default trading fee for LP pool
        rift.total_liquidity_underlying = 0;
        rift.total_liquidity_rift = 0;
        
        // Initialize all other original fields (same as vanity mint version)
        rift.oracle_prices = [PriceData::default(); 10];
        rift.price_index = 0;
        rift.oracle_update_interval = 30 * 60;
        rift.max_rebalance_interval = 24 * 60 * 60;
        rift.arbitrage_threshold_bps = 200;
        rift.last_oracle_update = Clock::get()?.unix_timestamp;
        
        rift.total_volume_24h = 0;
        rift.price_deviation = 0;
        rift.arbitrage_opportunity_bps = 0;
        rift.rebalance_count = 0;
        
        rift.total_fees_collected = 0;
        rift.rifts_tokens_distributed = 0;
        rift.rifts_tokens_burned = 0;
        
        rift.total_lp_staked = 0;
        rift.pending_rewards = 0;
        rift.last_reward_distribution = Clock::get()?.unix_timestamp;
        
        rift.reentrancy_guard = false;
        rift.is_paused = false;
        rift.pause_timestamp = 0;
        rift.pending_fee_distribution = 0;
        rift.last_governance_update = Clock::get()?.unix_timestamp;
        
        emit!(RiftCreated {
            rift: rift.key(),
            creator: rift.creator,
            underlying_mint: rift.underlying_mint,
            burn_fee_bps,
            partner_fee_bps,
        });

        Ok(())
    }

    /// ORIGINAL wrap_tokens function with 0.7% fees - UNCHANGED
    pub fn wrap_tokens(
        ctx: Context<WrapTokens>,
        amount: u64,
    ) -> Result<()> {
        
        let rift = &mut ctx.accounts.rift;
        
        // Check if rift is paused
        require!(!rift.is_paused, ErrorCode::RiftPaused);
        
        // **CRITICAL FIX**: Add reentrancy protection
        require!(!rift.reentrancy_guard, ErrorCode::ReentrancyDetected);
        rift.reentrancy_guard = true;
        
        // **CRITICAL FIX**: Enhanced input validation
        require!(amount > 0, ErrorCode::InvalidAmount);
        require!(amount <= 1_000_000_000_000_000, ErrorCode::AmountTooLarge); // Max 1 million tokens
        require!(amount >= 10000, ErrorCode::AmountTooSmall); // Minimum for non-zero fees
        
        // **CRITICAL FIX**: Validate backing ratio is positive before use
        require!(rift.backing_ratio > 0, ErrorCode::InvalidBackingRatio);
        require!(rift.backing_ratio <= 1_000_000_000_000, ErrorCode::BackingRatioTooLarge);
        
        // **KEEP ORIGINAL 0.7% FEE**
        let wrap_fee = amount
            .checked_mul(70)
            .ok_or(ErrorCode::MathOverflow)?
            .checked_div(10000)
            .ok_or(ErrorCode::MathOverflow)?;
        
        require!(wrap_fee > 0 || amount < 143, ErrorCode::FeeTooSmall);
        
        let amount_after_fee = amount
            .checked_sub(wrap_fee)
            .ok_or(ErrorCode::MathOverflow)?;

        // Transfer underlying tokens to vault
        require!(
            ctx.accounts.token_program.key() == anchor_spl::token::ID,
            ErrorCode::InvalidProgramId
        );
        
        let transfer_ctx = CpiContext::new(
            ctx.accounts.token_program.to_account_info(),
            Transfer {
                from: ctx.accounts.user_underlying.to_account_info(),
                to: ctx.accounts.vault.to_account_info(),
                authority: ctx.accounts.user.to_account_info(),
            },
        );
        token::transfer(transfer_ctx, amount)?;

        // Calculate rift tokens to mint with 1:1 ratio
        let rift_tokens_to_mint = amount_after_fee
            .checked_mul(10000)
            .ok_or(ErrorCode::MathOverflow)?
            .checked_div(rift.backing_ratio)
            .ok_or(ErrorCode::MathOverflow)?;
        
        require!(rift_tokens_to_mint > 0, ErrorCode::MintAmountTooSmall);
        require!(rift_tokens_to_mint <= 1_000_000_000_000_000, ErrorCode::MintAmountTooLarge);

        // Mint rift tokens to user
        let rift_key = rift.key();
        let rift_mint_auth_seeds = &[
            b"rift_mint_auth",
            rift_key.as_ref(),
            &[ctx.bumps.rift_mint_authority],
        ];
        let signer_seeds = &[&rift_mint_auth_seeds[..]];
        
        let mint_ctx = CpiContext::new_with_signer(
            ctx.accounts.token_program.to_account_info(),
            MintTo {
                mint: ctx.accounts.rift_mint.to_account_info(),
                to: ctx.accounts.user_rift_tokens.to_account_info(),
                authority: ctx.accounts.rift_mint_authority.to_account_info(),
            },
            signer_seeds,
        );
        token::mint_to(mint_ctx, rift_tokens_to_mint)?;

        // Update rift state with checked arithmetic
        rift.total_wrapped = rift.total_wrapped
            .checked_add(amount_after_fee)
            .ok_or(ErrorCode::MathOverflow)?;
        
        rift.total_fees_collected = rift.total_fees_collected
            .checked_add(wrap_fee)
            .ok_or(ErrorCode::MathOverflow)?;
        
        rift.total_volume_24h = rift.total_volume_24h
            .checked_add(amount)
            .ok_or(ErrorCode::MathOverflow)?;
        
        rift.last_oracle_update = Clock::get()?.unix_timestamp;

        // Process fee distribution BEFORE clearing reentrancy guard
        if wrap_fee > 0 {
            rift.process_fee_immediately(wrap_fee)?;
        }
        
        // Clear reentrancy guard at the end
        rift.reentrancy_guard = false;

        emit!(WrapExecuted {
            rift: rift.key(),
            user: ctx.accounts.user.key(),
            amount,
            fee_amount: wrap_fee,
            tokens_minted: rift_tokens_to_mint,
        });

        Ok(())
    }

    /// ORIGINAL unwrap_tokens function with 0.7% fees - UNCHANGED
    pub fn unwrap_tokens(
        ctx: Context<UnwrapTokens>,
        rift_token_amount: u64,
    ) -> Result<()> {
        
        let rift = &mut ctx.accounts.rift;
        
        require!(!rift.is_paused, ErrorCode::RiftPaused);
        require!(rift_token_amount > 0, ErrorCode::InvalidAmount);
        
        // Calculate underlying tokens with 1:1 ratio
        require!(rift.backing_ratio > 0, ErrorCode::InvalidBackingRatio);
        let underlying_amount = rift_token_amount
            .checked_mul(rift.backing_ratio)
            .ok_or(ErrorCode::MathOverflow)?
            .checked_div(10000)
            .ok_or(ErrorCode::MathOverflow)?;
        
        // **KEEP ORIGINAL 0.7% FEE**
        let unwrap_fee = underlying_amount
            .checked_mul(70)
            .ok_or(ErrorCode::MathOverflow)?
            .checked_div(10000)
            .ok_or(ErrorCode::MathOverflow)?;
        let amount_after_fee = underlying_amount
            .checked_sub(unwrap_fee)
            .ok_or(ErrorCode::MathOverflow)?;

        // Burn rift tokens
        require!(
            ctx.accounts.token_program.key() == anchor_spl::token::ID,
            ErrorCode::InvalidProgramId
        );
        
        let burn_ctx = CpiContext::new(
            ctx.accounts.token_program.to_account_info(),
            anchor_spl::token::Burn {
                mint: ctx.accounts.rift_mint.to_account_info(),
                from: ctx.accounts.user_rift_tokens.to_account_info(),
                authority: ctx.accounts.user.to_account_info(),
            },
        );
        anchor_spl::token::burn(burn_ctx, rift_token_amount)?;

        // Transfer underlying tokens back to user
        let rift_key = rift.key();
        let vault_auth_seeds = &[
            b"vault_auth",
            rift_key.as_ref(),
            &[ctx.bumps.vault_authority],
        ];
        let signer_seeds = &[&vault_auth_seeds[..]];
        
        let transfer_ctx = CpiContext::new_with_signer(
            ctx.accounts.token_program.to_account_info(),
            Transfer {
                from: ctx.accounts.vault.to_account_info(),
                to: ctx.accounts.user_underlying.to_account_info(),
                authority: ctx.accounts.vault_authority.to_account_info(),
            },
            signer_seeds,
        );
        token::transfer(transfer_ctx, amount_after_fee)?;
        
        // Update total wrapped - decrease by the underlying amount returned
        rift.total_wrapped = rift.total_wrapped.saturating_sub(underlying_amount);
        
        rift.total_fees_collected = rift.total_fees_collected
            .checked_add(unwrap_fee)
            .ok_or(ErrorCode::MathOverflow)?;
        
        rift.total_burned = rift.total_burned
            .checked_add(rift_token_amount)
            .ok_or(ErrorCode::MathOverflow)?;
        
        rift.total_volume_24h = rift.total_volume_24h
            .checked_add(underlying_amount)
            .ok_or(ErrorCode::MathOverflow)?;
        
        rift.last_oracle_update = Clock::get()?.unix_timestamp;

        require!(!rift.reentrancy_guard, ErrorCode::ReentrancyDetected);
        rift.reentrancy_guard = true;
        
        rift.process_fee_immediately(unwrap_fee)?;
        rift.reentrancy_guard = false;

        emit!(UnwrapExecuted {
            rift: rift.key(),
            user: ctx.accounts.user.key(),
            rift_token_amount,
            fee_amount: unwrap_fee,
            underlying_returned: amount_after_fee,
        });

        Ok(())
    }

    // NEW LP POOL FUNCTIONS (ADDED ON TOP OF ORIGINAL)

    /// Create LP pool for existing Rift - allows trading wrapped tokens
    pub fn create_lp_pool(
        ctx: Context<CreateLPPool>,
        initial_underlying_amount: u64,
        initial_rift_amount: u64,
        trading_fee_bps: u16,
    ) -> Result<()> {
        let rift = &mut ctx.accounts.rift;
        
        require!(initial_underlying_amount > 0, ErrorCode::InvalidAmount);
        require!(initial_rift_amount > 0, ErrorCode::InvalidAmount);
        require!(trading_fee_bps <= 100, ErrorCode::InvalidTradingFee); // Max 1% for LP trading
        require!(rift.lp_token_supply == 0, ErrorCode::PoolAlreadyExists);
        
        // Update LP pool state
        rift.total_liquidity_underlying = initial_underlying_amount;
        rift.total_liquidity_rift = initial_rift_amount;
        rift.pool_trading_fee_bps = trading_fee_bps;
        rift.lp_token_supply = (initial_underlying_amount as u128 * initial_rift_amount as u128).sqrt() as u64;
        
        emit!(LPPoolCreated {
            rift: rift.key(),
            creator: ctx.accounts.creator.key(),
            initial_underlying_amount,
            initial_rift_amount,
            trading_fee_bps,
        });

        Ok(())
    }

    /// Add liquidity to existing Rift LP pool 
    pub fn add_liquidity_to_pool(
        ctx: Context<AddLiquidityToPool>,
        underlying_amount: u64,
        rift_amount: u64,
        min_lp_tokens: u64,
    ) -> Result<()> {
        let rift = &mut ctx.accounts.rift;
        
        require!(!rift.is_paused, ErrorCode::RiftPaused);
        require!(!rift.reentrancy_guard, ErrorCode::ReentrancyDetected);
        rift.reentrancy_guard = true;
        
        require!(underlying_amount > 0, ErrorCode::InvalidAmount);
        require!(rift_amount > 0, ErrorCode::InvalidAmount);
        require!(rift.lp_token_supply > 0, ErrorCode::NoLiquidityPool);
        
        // Calculate LP tokens using existing pool ratio
        let underlying_ratio = underlying_amount as u128 * rift.lp_token_supply as u128 / rift.total_liquidity_underlying as u128;
        let rift_ratio = rift_amount as u128 * rift.lp_token_supply as u128 / rift.total_liquidity_rift as u128;
        let lp_tokens_to_mint = std::cmp::min(underlying_ratio, rift_ratio) as u64;
        
        require!(lp_tokens_to_mint >= min_lp_tokens, ErrorCode::SlippageTooHigh);
        
        // Transfer tokens from user to pool
        let transfer_underlying_ctx = CpiContext::new(
            ctx.accounts.token_program.to_account_info(),
            Transfer {
                from: ctx.accounts.user_underlying.to_account_info(),
                to: ctx.accounts.pool_underlying.to_account_info(),
                authority: ctx.accounts.user.to_account_info(),
            },
        );
        token::transfer(transfer_underlying_ctx, underlying_amount)?;
        
        let transfer_rift_ctx = CpiContext::new(
            ctx.accounts.token_program.to_account_info(),
            Transfer {
                from: ctx.accounts.user_rift_tokens.to_account_info(),
                to: ctx.accounts.pool_rift_tokens.to_account_info(),
                authority: ctx.accounts.user.to_account_info(),
            },
        );
        token::transfer(transfer_rift_ctx, rift_amount)?;
        
        // Mint LP tokens to user
        let rift_key = rift.key();
        let lp_mint_auth_seeds = &[
            b"lp_mint_auth",
            rift_key.as_ref(),
            &[ctx.bumps.lp_mint_authority],
        ];
        let signer_seeds = &[&lp_mint_auth_seeds[..]];
        
        let mint_ctx = CpiContext::new_with_signer(
            ctx.accounts.token_program.to_account_info(),
            MintTo {
                mint: ctx.accounts.lp_mint.to_account_info(),
                to: ctx.accounts.user_lp_tokens.to_account_info(),
                authority: ctx.accounts.lp_mint_authority.to_account_info(),
            },
            signer_seeds,
        );
        token::mint_to(mint_ctx, lp_tokens_to_mint)?;
        
        // Update pool state
        rift.total_liquidity_underlying = rift.total_liquidity_underlying
            .checked_add(underlying_amount)
            .ok_or(ErrorCode::MathOverflow)?;
        rift.total_liquidity_rift = rift.total_liquidity_rift
            .checked_add(rift_amount)
            .ok_or(ErrorCode::MathOverflow)?;
        rift.lp_token_supply = rift.lp_token_supply
            .checked_add(lp_tokens_to_mint)
            .ok_or(ErrorCode::MathOverflow)?;
        
        rift.reentrancy_guard = false;
        
        emit!(LiquidityAdded {
            rift: rift.key(),
            user: ctx.accounts.user.key(),
            underlying_amount,
            rift_amount,
            lp_tokens_minted: lp_tokens_to_mint,
        });
        
        Ok(())
    }

    /// Swap underlying tokens for rift tokens via LP pool
    pub fn swap_underlying_for_rift_pool(
        ctx: Context<SwapUnderlyingForRiftPool>,
        underlying_amount: u64,
        min_rift_out: u64,
    ) -> Result<()> {
        let rift = &mut ctx.accounts.rift;
        
        require!(!rift.is_paused, ErrorCode::RiftPaused);
        require!(!rift.reentrancy_guard, ErrorCode::ReentrancyDetected);
        rift.reentrancy_guard = true;
        
        require!(underlying_amount > 0, ErrorCode::InvalidAmount);
        require!(rift.lp_token_supply > 0, ErrorCode::NoLiquidityPool);
        
        // Calculate rift amount out using constant product formula (x * y = k)
        let trading_fee = underlying_amount * rift.pool_trading_fee_bps as u64 / 10000;
        let underlying_after_fee = underlying_amount - trading_fee;
        
        let new_underlying_reserve = rift.total_liquidity_underlying + underlying_after_fee;
        let new_rift_reserve = (rift.total_liquidity_underlying as u128 * rift.total_liquidity_rift as u128 / new_underlying_reserve as u128) as u64;
        let rift_amount_out = rift.total_liquidity_rift - new_rift_reserve;
        
        require!(rift_amount_out >= min_rift_out, ErrorCode::SlippageTooHigh);
        
        // Transfer underlying from user to pool
        let transfer_ctx = CpiContext::new(
            ctx.accounts.token_program.to_account_info(),
            Transfer {
                from: ctx.accounts.user_underlying.to_account_info(),
                to: ctx.accounts.pool_underlying.to_account_info(),
                authority: ctx.accounts.user.to_account_info(),
            },
        );
        token::transfer(transfer_ctx, underlying_amount)?;
        
        // Transfer rift from pool to user
        let rift_key = rift.key();
        let pool_auth_seeds = &[
            b"pool_auth",
            rift_key.as_ref(),
            &[ctx.bumps.pool_authority],
        ];
        let signer_seeds = &[&pool_auth_seeds[..]];
        
        let transfer_rift_ctx = CpiContext::new_with_signer(
            ctx.accounts.token_program.to_account_info(),
            Transfer {
                from: ctx.accounts.pool_rift_tokens.to_account_info(),
                to: ctx.accounts.user_rift_tokens.to_account_info(),
                authority: ctx.accounts.pool_authority.to_account_info(),
            },
            signer_seeds,
        );
        token::transfer(transfer_rift_ctx, rift_amount_out)?;
        
        // Update pool reserves
        rift.total_liquidity_underlying = new_underlying_reserve;
        rift.total_liquidity_rift = new_rift_reserve;
        
        // Update metrics
        rift.total_volume_24h = rift.total_volume_24h
            .checked_add(underlying_amount)
            .unwrap_or(rift.total_volume_24h);
        rift.total_fees_collected = rift.total_fees_collected
            .checked_add(trading_fee)
            .unwrap_or(rift.total_fees_collected);
        
        rift.reentrancy_guard = false;
        
        emit!(SwapExecuted {
            rift: rift.key(),
            user: ctx.accounts.user.key(),
            amount_in: underlying_amount,
            amount_out: rift_amount_out,
            fee_amount: trading_fee,
            is_underlying_to_rift: true,
        });
        
        Ok(())
    }

    // TODO: Add remaining functions from original 2435-line version
    // This is a shortened version showing the hybrid approach
}

// Account structures

#[derive(Accounts)]
pub struct CreateRiftWithVanityMint<'info> {
    #[account(mut)]
    pub creator: Signer<'info>,
    
    #[account(
        init,
        payer = creator,
        space = 8 + Rift::INIT_SPACE,
        seeds = [b"rift", underlying_mint.key().as_ref(), creator.key().as_ref()],
        bump
    )]
    pub rift: Account<'info, Rift>,
    
    pub underlying_mint: Account<'info, Mint>,
    
    /// The vanity mint account (pre-generated with address ending in 'rift')
    #[account(
        init,
        payer = creator,
        mint::decimals = underlying_mint.decimals,
        mint::authority = rift_mint_authority,
        signer
    )]
    pub rift_mint: Account<'info, Mint>,
    
    /// CHECK: PDA for mint authority
    #[account(
        seeds = [b"rift_mint_auth", rift.key().as_ref()],
        bump
    )]
    pub rift_mint_authority: UncheckedAccount<'info>,
    
    /// Vault token account - KEEP ORIGINAL VAULT SYSTEM
    #[account(
        init,
        payer = creator,
        token::mint = underlying_mint,
        token::authority = vault_authority,
        seeds = [b"vault", rift.key().as_ref()],
        bump
    )]
    pub vault: Account<'info, TokenAccount>,
    
    /// CHECK: PDA for vault authority
    #[account(
        seeds = [b"vault_auth", rift.key().as_ref()],
        bump
    )]
    pub vault_authority: UncheckedAccount<'info>,
    
    pub token_program: Program<'info, Token>,
    pub system_program: Program<'info, System>,
    pub rent: Sysvar<'info, Rent>,
}

#[derive(Accounts)]
pub struct CreateRift<'info> {
    #[account(mut)]
    pub creator: Signer<'info>,
    
    #[account(
        init,
        payer = creator,
        space = 8 + Rift::INIT_SPACE,
        seeds = [b"rift", underlying_mint.key().as_ref(), creator.key().as_ref()],
        bump
    )]
    pub rift: Account<'info, Rift>,
    
    pub underlying_mint: Account<'info, Mint>,
    
    #[account(
        init,
        payer = creator,
        mint::decimals = underlying_mint.decimals,
        mint::authority = rift_mint_authority,
        seeds = [b"rift_mint", rift.key().as_ref()],
        bump
    )]
    pub rift_mint: Account<'info, Mint>,
    
    /// CHECK: PDA for mint authority
    #[account(
        seeds = [b"rift_mint_auth", rift.key().as_ref()],
        bump
    )]
    pub rift_mint_authority: UncheckedAccount<'info>,
    
    /// Vault token account - KEEP ORIGINAL VAULT SYSTEM
    #[account(
        init,
        payer = creator,
        token::mint = underlying_mint,
        token::authority = vault_authority,
        seeds = [b"vault", rift.key().as_ref()],
        bump
    )]
    pub vault: Account<'info, TokenAccount>,
    
    /// CHECK: PDA for vault authority
    #[account(
        seeds = [b"vault_auth", rift.key().as_ref()],
        bump
    )]
    pub vault_authority: UncheckedAccount<'info>,
    
    pub token_program: Program<'info, Token>,
    pub system_program: Program<'info, System>,
    pub rent: Sysvar<'info, Rent>,
}

#[derive(Accounts)]
pub struct WrapTokens<'info> {
    #[account(mut)]
    pub user: Signer<'info>,
    
    #[account(mut)]
    pub rift: Account<'info, Rift>,
    
    #[account(mut)]
    pub user_underlying: Account<'info, TokenAccount>,
    
    #[account(mut)]
    pub user_rift_tokens: Account<'info, TokenAccount>,
    
    /// Vault token account - KEEP ORIGINAL VAULT SYSTEM
    #[account(
        mut,
        seeds = [b"vault", rift.key().as_ref()],
        bump
    )]
    pub vault: Account<'info, TokenAccount>,
    
    #[account(mut)]
    pub rift_mint: Account<'info, Mint>,
    
    /// CHECK: PDA
    #[account(
        seeds = [b"rift_mint_auth", rift.key().as_ref()],
        bump
    )]
    pub rift_mint_authority: UncheckedAccount<'info>,
    
    pub token_program: Program<'info, Token>,
}

#[derive(Accounts)]
pub struct UnwrapTokens<'info> {
    #[account(mut)]
    pub user: Signer<'info>,
    
    #[account(mut)]
    pub rift: Account<'info, Rift>,
    
    #[account(mut)]
    pub user_underlying: Account<'info, TokenAccount>,
    
    #[account(mut)]
    pub user_rift_tokens: Account<'info, TokenAccount>,
    
    /// Vault token account - KEEP ORIGINAL VAULT SYSTEM
    #[account(
        mut,
        seeds = [b"vault", rift.key().as_ref()],
        bump
    )]
    pub vault: Account<'info, TokenAccount>,
    
    #[account(mut)]
    pub rift_mint: Account<'info, Mint>,
    
    /// CHECK: PDA for vault authority
    #[account(
        seeds = [b"vault_auth", rift.key().as_ref()],
        bump
    )]
    pub vault_authority: UncheckedAccount<'info>,
    
    pub token_program: Program<'info, Token>,
}

// NEW LP POOL ACCOUNT STRUCTURES

#[derive(Accounts)]
pub struct CreateLPPool<'info> {
    #[account(mut)]
    pub creator: Signer<'info>,
    
    #[account(mut)]
    pub rift: Account<'info, Rift>,
    
    /// LP token mint
    #[account(
        init,
        payer = creator,
        mint::decimals = 6,
        mint::authority = lp_mint_authority,
        seeds = [b"lp_mint", rift.key().as_ref()],
        bump
    )]
    pub lp_mint: Account<'info, Mint>,
    
    /// CHECK: PDA for LP mint authority
    #[account(
        seeds = [b"lp_mint_auth", rift.key().as_ref()],
        bump
    )]
    pub lp_mint_authority: UncheckedAccount<'info>,
    
    /// Pool underlying token account
    #[account(
        init,
        payer = creator,
        token::mint = rift.underlying_mint,
        token::authority = pool_authority,
        seeds = [b"pool_underlying", rift.key().as_ref()],
        bump
    )]
    pub pool_underlying: Account<'info, TokenAccount>,
    
    /// Pool rift token account
    #[account(
        init,
        payer = creator,
        token::mint = rift.rift_mint,
        token::authority = pool_authority,
        seeds = [b"pool_rift", rift.key().as_ref()],
        bump
    )]
    pub pool_rift_tokens: Account<'info, TokenAccount>,
    
    /// CHECK: PDA for pool authority
    #[account(
        seeds = [b"pool_auth", rift.key().as_ref()],
        bump
    )]
    pub pool_authority: UncheckedAccount<'info>,
    
    pub token_program: Program<'info, Token>,
    pub system_program: Program<'info, System>,
    pub rent: Sysvar<'info, Rent>,
}

#[derive(Accounts)]
pub struct AddLiquidityToPool<'info> {
    #[account(mut)]
    pub user: Signer<'info>,
    
    #[account(mut)]
    pub rift: Account<'info, Rift>,
    
    #[account(mut)]
    pub user_underlying: Account<'info, TokenAccount>,
    
    #[account(mut)]
    pub user_rift_tokens: Account<'info, TokenAccount>,
    
    #[account(mut)]
    pub user_lp_tokens: Account<'info, TokenAccount>,
    
    #[account(mut)]
    pub pool_underlying: Account<'info, TokenAccount>,
    
    #[account(mut)]
    pub pool_rift_tokens: Account<'info, TokenAccount>,
    
    #[account(mut)]
    pub lp_mint: Account<'info, Mint>,
    
    /// CHECK: PDA for LP mint authority
    #[account(
        seeds = [b"lp_mint_auth", rift.key().as_ref()],
        bump
    )]
    pub lp_mint_authority: UncheckedAccount<'info>,
    
    pub token_program: Program<'info, Token>,
}

#[derive(Accounts)]
pub struct SwapUnderlyingForRiftPool<'info> {
    #[account(mut)]
    pub user: Signer<'info>,
    
    #[account(mut)]
    pub rift: Account<'info, Rift>,
    
    #[account(mut)]
    pub user_underlying: Account<'info, TokenAccount>,
    
    #[account(mut)]
    pub user_rift_tokens: Account<'info, TokenAccount>,
    
    #[account(mut)]
    pub pool_underlying: Account<'info, TokenAccount>,
    
    #[account(mut)]
    pub pool_rift_tokens: Account<'info, TokenAccount>,
    
    /// CHECK: PDA for pool authority
    #[account(
        seeds = [b"pool_auth", rift.key().as_ref()],
        bump
    )]
    pub pool_authority: UncheckedAccount<'info>,
    
    pub token_program: Program<'info, Token>,
}

// HYBRID DATA STRUCTURE - Original Rift + LP Pool fields

#[account]
#[derive(Default)]
pub struct Rift {
    // ORIGINAL FIELDS - KEEP ALL
    pub creator: Pubkey,
    pub underlying_mint: Pubkey,
    pub rift_mint: Pubkey,
    pub vault: Pubkey, // KEEP ORIGINAL VAULT
    pub name: String,
    
    // Original fee structure
    pub burn_fee_bps: u16,
    pub partner_fee_bps: u16,
    pub partner_wallet: Option<Pubkey>,
    
    // Original 1:1 wrap/unwrap state
    pub total_wrapped: u64,
    pub total_burned: u64,
    pub backing_ratio: u64, // KEEP 1:1 ratio
    
    // NEW LP POOL FIELDS - ADDED
    pub liquidity_pool: Option<Pubkey>, // Optional LP pool
    pub lp_token_supply: u64,
    pub pool_trading_fee_bps: u16, // Separate fee for LP trading
    pub total_liquidity_underlying: u64,
    pub total_liquidity_rift: u64,
    
    // Timestamps
    pub created_at: i64,
    pub last_rebalance: i64,
    
    // Oracle system (keep original)
    pub oracle_prices: [PriceData; 10],
    pub price_index: u8,
    pub oracle_update_interval: u64,
    pub max_rebalance_interval: u64,
    pub arbitrage_threshold_bps: u16,
    pub last_oracle_update: i64,
    
    // Analytics (keep original)
    pub total_volume_24h: u64,
    pub price_deviation: i64,
    pub arbitrage_opportunity_bps: u16,
    pub rebalance_count: u64,
    
    // Fee tracking (keep original)
    pub total_fees_collected: u64,
    pub rifts_tokens_distributed: u64,
    pub rifts_tokens_burned: u64,
    
    // LP staking (keep original)
    pub total_lp_staked: u64,
    pub pending_rewards: u64,
    pub last_reward_distribution: i64,
    
    // Security (keep original)
    pub reentrancy_guard: bool,
    pub is_paused: bool,
    pub pause_timestamp: i64,
    
    // External integration (keep original)
    pub pending_fee_distribution: u64,
    pub last_governance_update: i64,
}

impl Rift {
    pub const INIT_SPACE: usize = 32 + 32 + 32 + 32 + 36 + 
        2 + 2 + 33 + 8 + 8 + 8 + 
        33 + 8 + 2 + 8 + 8 + // LP pool fields
        320 + 1 + 8 + 8 + 2 + 8 + 
        8 + 8 + 2 + 8 + 8 + 8 + 8 +
        8 + 8 + 8 + 1 + 1 + 8 + 8 + 8 + 64;

    pub fn process_fee_immediately(&mut self, _fee_amount: u64) -> Result<()> {
        // Fee processing logic (simplified)
        Ok(())
    }
}

#[derive(AnchorSerialize, AnchorDeserialize, Clone, Copy, Default)]
pub struct PriceData {
    pub price: u64,
    pub confidence: u64,
    pub timestamp: i64,
    pub oracle_source: u8,
}

// Events

#[event]
pub struct RiftCreated {
    pub rift: Pubkey,
    pub creator: Pubkey,
    pub underlying_mint: Pubkey,
    pub burn_fee_bps: u16,
    pub partner_fee_bps: u16,
}

#[event]
pub struct WrapExecuted {
    pub rift: Pubkey,
    pub user: Pubkey,
    pub amount: u64,
    pub fee_amount: u64,
    pub tokens_minted: u64,
}

#[event]
pub struct UnwrapExecuted {
    pub rift: Pubkey,
    pub user: Pubkey,
    pub rift_token_amount: u64,
    pub fee_amount: u64,
    pub underlying_returned: u64,
}

#[event]
pub struct LPPoolCreated {
    pub rift: Pubkey,
    pub creator: Pubkey,
    pub initial_underlying_amount: u64,
    pub initial_rift_amount: u64,
    pub trading_fee_bps: u16,
}

#[event]
pub struct LiquidityAdded {
    pub rift: Pubkey,
    pub user: Pubkey,
    pub underlying_amount: u64,
    pub rift_amount: u64,
    pub lp_tokens_minted: u64,
}

#[event]
pub struct SwapExecuted {
    pub rift: Pubkey,
    pub user: Pubkey,
    pub amount_in: u64,
    pub amount_out: u64,
    pub fee_amount: u64,
    pub is_underlying_to_rift: bool,
}

// Error codes

#[error_code]
pub enum ErrorCode {
    #[msg("Invalid amount provided")]
    InvalidAmount,
    #[msg("Invalid burn fee")]
    InvalidBurnFee,
    #[msg("Invalid partner fee")]
    InvalidPartnerFee,
    #[msg("Vanity address must end with 'rift'")]
    InvalidVanityAddress,
    #[msg("Name too long")]
    NameTooLong,
    #[msg("Invalid rift name format")]
    InvalidRiftName,
    #[msg("Rift is paused")]
    RiftPaused,
    #[msg("Reentrancy detected")]
    ReentrancyDetected,
    #[msg("Math overflow")]
    MathOverflow,
    #[msg("Amount too large")]
    AmountTooLarge,
    #[msg("Amount too small")]
    AmountTooSmall,
    #[msg("Invalid backing ratio")]
    InvalidBackingRatio,
    #[msg("Backing ratio too large")]
    BackingRatioTooLarge,
    #[msg("Fee too small")]
    FeeTooSmall,
    #[msg("Invalid program ID")]
    InvalidProgramId,
    #[msg("Mint amount too small")]
    MintAmountTooSmall,
    #[msg("Mint amount too large")]
    MintAmountTooLarge,
    #[msg("Trading fee too high")]
    InvalidTradingFee,
    #[msg("Slippage tolerance exceeded")]
    SlippageTooHigh,
    #[msg("No liquidity pool exists")]
    NoLiquidityPool,
    #[msg("Pool already exists")]
    PoolAlreadyExists,
}