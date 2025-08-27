// Rifts Protocol - Jupiter/Meteora LP Pool Model (Like Peapods Finance)
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

    /// Create a new Rift LP Pool on Jupiter/Meteora with vanity mint address
    /// This creates liquidity pools instead of 1:1 wrappers like Peapods Finance
    pub fn create_rift_pool_with_vanity_mint(
        ctx: Context<CreateRiftPoolWithVanityMint>,
        initial_rift_amount: u64,
        initial_underlying_amount: u64,
        trading_fee_bps: u16,
        protocol_fee_bps: u16,
        partner_wallet: Option<Pubkey>,
        rift_name: Option<String>,
        dex_platform: DexPlatform,
    ) -> Result<()> {
        let rift = &mut ctx.accounts.rift;
        
        // Validate pool creation parameters
        require!(initial_rift_amount > 0, ErrorCode::InvalidAmount);
        require!(initial_underlying_amount > 0, ErrorCode::InvalidAmount);
        require!(trading_fee_bps <= 300, ErrorCode::InvalidTradingFee); // Max 3% trading fee
        require!(protocol_fee_bps <= 100, ErrorCode::InvalidProtocolFee); // Max 1% protocol fee
        
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
        
        // Create Jupiter/Meteora pool PDA
        let rift_key = rift.key();
        let pool_seeds = &[b"liquidity_pool", rift_key.as_ref()];
        let (pool_pda, _) = Pubkey::find_program_address(pool_seeds, &crate::ID);
        rift.liquidity_pool = pool_pda;
        
        rift.trading_fee_bps = trading_fee_bps;
        rift.protocol_fee_bps = protocol_fee_bps;
        rift.partner_wallet = partner_wallet;
        rift.dex_platform = dex_platform;
        rift.total_liquidity_rift = initial_rift_amount;
        rift.total_liquidity_underlying = initial_underlying_amount;
        rift.current_price = (initial_underlying_amount as u128 * 10000 / initial_rift_amount as u128) as u64;
        rift.lp_token_supply = 0; // Will be set after pool creation
        rift.last_rebalance = Clock::get()?.unix_timestamp;
        rift.created_at = Clock::get()?.unix_timestamp;
        
        // Initialize hybrid oracle system
        rift.oracle_prices = [PriceData::default(); 10];
        rift.price_index = 0;
        rift.oracle_update_interval = 30 * 60;
        rift.max_rebalance_interval = 24 * 60 * 60;
        rift.arbitrage_threshold_bps = 200;
        rift.last_oracle_update = Clock::get()?.unix_timestamp;
        
        // Initialize advanced metrics
        rift.total_volume_24h = 0;
        rift.price_deviation = 0;
        rift.arbitrage_opportunity_bps = 0;
        rift.rebalance_count = 0;
        
        // Initialize RIFTS token distribution tracking
        rift.total_fees_collected = 0;
        rift.rifts_tokens_distributed = 0;
        rift.rifts_tokens_burned = 0;
        
        // Initialize LP staking
        rift.total_lp_staked = 0;
        rift.pending_rewards = 0;
        rift.last_reward_distribution = Clock::get()?.unix_timestamp;
        
        // Initialize reentrancy protection
        rift.reentrancy_guard = false;
        
        // Initialize emergency controls
        rift.is_paused = false;
        rift.pause_timestamp = 0;
        
        // Initialize external program integration
        rift.pending_fee_distribution = 0;
        
        // Initialize governance integration
        rift.last_governance_update = Clock::get()?.unix_timestamp;
        
        // Create initial liquidity on selected DEX platform
        rift.create_dex_pool(
            &ctx.accounts.creator,
            &ctx.accounts.underlying_mint,
            &ctx.accounts.rift_mint,
            initial_underlying_amount,
            initial_rift_amount,
            dex_platform,
        )?;

        emit!(RiftPoolCreated {
            rift: rift.key(),
            creator: rift.creator,
            underlying_mint: rift.underlying_mint,
            rift_mint: rift.rift_mint,
            pool_address: rift.liquidity_pool,
            initial_rift_amount,
            initial_underlying_amount,
            dex_platform,
        });

        Ok(())
    }

    /// Create a new Rift LP Pool (PDA version) - Standard pool creation
    pub fn create_rift_pool(
        ctx: Context<CreateRiftPool>,
        initial_rift_amount: u64,
        initial_underlying_amount: u64,
        trading_fee_bps: u16,
        protocol_fee_bps: u16,
        partner_wallet: Option<Pubkey>,
        rift_name: Option<String>,
        dex_platform: DexPlatform,
    ) -> Result<()> {
        let rift = &mut ctx.accounts.rift;
        
        // Validate pool creation parameters
        require!(initial_rift_amount > 0, ErrorCode::InvalidAmount);
        require!(initial_underlying_amount > 0, ErrorCode::InvalidAmount);
        require!(trading_fee_bps <= 300, ErrorCode::InvalidTradingFee);
        require!(protocol_fee_bps <= 100, ErrorCode::InvalidProtocolFee);
        
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
        
        // Create Jupiter/Meteora pool PDA
        let rift_key = rift.key();
        let pool_seeds = &[b"liquidity_pool", rift_key.as_ref()];
        let (pool_pda, _) = Pubkey::find_program_address(pool_seeds, &crate::ID);
        rift.liquidity_pool = pool_pda;
        
        rift.trading_fee_bps = trading_fee_bps;
        rift.protocol_fee_bps = protocol_fee_bps;
        rift.partner_wallet = partner_wallet;
        rift.dex_platform = dex_platform;
        rift.total_liquidity_rift = initial_rift_amount;
        rift.total_liquidity_underlying = initial_underlying_amount;
        rift.current_price = (initial_underlying_amount as u128 * 10000 / initial_rift_amount as u128) as u64;
        rift.lp_token_supply = 0;
        rift.last_rebalance = Clock::get()?.unix_timestamp;
        rift.created_at = Clock::get()?.unix_timestamp;
        
        // Initialize all other fields (same as vanity mint version)
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
        
        // Create initial liquidity on selected DEX platform
        rift.create_dex_pool(
            &ctx.accounts.creator,
            &ctx.accounts.underlying_mint,
            &ctx.accounts.rift_mint,
            initial_underlying_amount,
            initial_rift_amount,
            dex_platform,
        )?;

        emit!(RiftPoolCreated {
            rift: rift.key(),
            creator: rift.creator,
            underlying_mint: rift.underlying_mint,
            rift_mint: rift.rift_mint,
            pool_address: rift.liquidity_pool,
            initial_rift_amount,
            initial_underlying_amount,
            dex_platform,
        });

        Ok(())
    }

    /// Add liquidity to existing Rift pool (like adding to Uniswap V2 pool)
    pub fn add_liquidity(
        ctx: Context<AddLiquidity>,
        underlying_amount: u64,
        rift_amount: u64,
        min_lp_tokens: u64,
    ) -> Result<()> {
        let rift = &mut ctx.accounts.rift;
        
        // Check if rift is paused
        require!(!rift.is_paused, ErrorCode::RiftPaused);
        require!(!rift.reentrancy_guard, ErrorCode::ReentrancyDetected);
        rift.reentrancy_guard = true;
        
        // Validate amounts
        require!(underlying_amount > 0, ErrorCode::InvalidAmount);
        require!(rift_amount > 0, ErrorCode::InvalidAmount);
        
        // Calculate LP tokens to mint based on pool ratio
        let lp_tokens_to_mint = if rift.lp_token_supply == 0 {
            // First liquidity provision - use geometric mean
            (underlying_amount as u128 * rift_amount as u128).sqrt() as u64
        } else {
            // Subsequent liquidity - use ratio to existing pool
            let underlying_ratio = underlying_amount as u128 * rift.lp_token_supply as u128 / rift.total_liquidity_underlying as u128;
            let rift_ratio = rift_amount as u128 * rift.lp_token_supply as u128 / rift.total_liquidity_rift as u128;
            // Use the smaller ratio to prevent dilution
            std::cmp::min(underlying_ratio, rift_ratio) as u64
        };
        
        require!(lp_tokens_to_mint >= min_lp_tokens, ErrorCode::SlippageTooHigh);
        
        // Transfer tokens from user
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
        
        // Update price
        rift.current_price = (rift.total_liquidity_underlying as u128 * 10000 / rift.total_liquidity_rift as u128) as u64;
        
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

    /// Remove liquidity from Rift pool
    pub fn remove_liquidity(
        ctx: Context<RemoveLiquidity>,
        lp_token_amount: u64,
        min_underlying: u64,
        min_rift: u64,
    ) -> Result<()> {
        let rift = &mut ctx.accounts.rift;
        
        require!(!rift.is_paused, ErrorCode::RiftPaused);
        require!(!rift.reentrancy_guard, ErrorCode::ReentrancyDetected);
        rift.reentrancy_guard = true;
        
        require!(lp_token_amount > 0, ErrorCode::InvalidAmount);
        require!(rift.lp_token_supply > 0, ErrorCode::NoLiquidity);
        
        // Calculate underlying and rift amounts to return
        let underlying_amount = (lp_token_amount as u128 * rift.total_liquidity_underlying as u128 / rift.lp_token_supply as u128) as u64;
        let rift_amount = (lp_token_amount as u128 * rift.total_liquidity_rift as u128 / rift.lp_token_supply as u128) as u64;
        
        require!(underlying_amount >= min_underlying, ErrorCode::SlippageTooHigh);
        require!(rift_amount >= min_rift, ErrorCode::SlippageTooHigh);
        
        // Burn LP tokens
        let burn_ctx = CpiContext::new(
            ctx.accounts.token_program.to_account_info(),
            Burn {
                mint: ctx.accounts.lp_mint.to_account_info(),
                from: ctx.accounts.user_lp_tokens.to_account_info(),
                authority: ctx.accounts.user.to_account_info(),
            },
        );
        token::burn(burn_ctx, lp_token_amount)?;
        
        // Transfer tokens back to user
        let rift_key = rift.key();
        let pool_auth_seeds = &[
            b"pool_auth",
            rift_key.as_ref(),
            &[ctx.bumps.pool_authority],
        ];
        let signer_seeds = &[&pool_auth_seeds[..]];
        
        let transfer_underlying_ctx = CpiContext::new_with_signer(
            ctx.accounts.token_program.to_account_info(),
            Transfer {
                from: ctx.accounts.pool_underlying.to_account_info(),
                to: ctx.accounts.user_underlying.to_account_info(),
                authority: ctx.accounts.pool_authority.to_account_info(),
            },
            signer_seeds,
        );
        token::transfer(transfer_underlying_ctx, underlying_amount)?;
        
        let transfer_rift_ctx = CpiContext::new_with_signer(
            ctx.accounts.token_program.to_account_info(),
            Transfer {
                from: ctx.accounts.pool_rift_tokens.to_account_info(),
                to: ctx.accounts.user_rift_tokens.to_account_info(),
                authority: ctx.accounts.pool_authority.to_account_info(),
            },
            signer_seeds,
        );
        token::transfer(transfer_rift_ctx, rift_amount)?;
        
        // Update pool state
        rift.total_liquidity_underlying = rift.total_liquidity_underlying.saturating_sub(underlying_amount);
        rift.total_liquidity_rift = rift.total_liquidity_rift.saturating_sub(rift_amount);
        rift.lp_token_supply = rift.lp_token_supply.saturating_sub(lp_token_amount);
        
        // Update price
        if rift.total_liquidity_rift > 0 {
            rift.current_price = (rift.total_liquidity_underlying as u128 * 10000 / rift.total_liquidity_rift as u128) as u64;
        }
        
        rift.reentrancy_guard = false;
        
        emit!(LiquidityRemoved {
            rift: rift.key(),
            user: ctx.accounts.user.key(),
            lp_tokens_burned: lp_token_amount,
            underlying_amount,
            rift_amount,
        });
        
        Ok(())
    }

    /// Swap underlying tokens for rift tokens
    pub fn swap_underlying_for_rift(
        ctx: Context<SwapUnderlyingForRift>,
        underlying_amount: u64,
        min_rift_out: u64,
    ) -> Result<()> {
        let rift = &mut ctx.accounts.rift;
        
        require!(!rift.is_paused, ErrorCode::RiftPaused);
        require!(!rift.reentrancy_guard, ErrorCode::ReentrancyDetected);
        rift.reentrancy_guard = true;
        
        require!(underlying_amount > 0, ErrorCode::InvalidAmount);
        
        // Calculate rift amount out using constant product formula (x * y = k)
        let trading_fee = underlying_amount * rift.trading_fee_bps as u64 / 10000;
        let underlying_after_fee = underlying_amount - trading_fee;
        
        let new_underlying_reserve = rift.total_liquidity_underlying + underlying_after_fee;
        let new_rift_reserve = (rift.total_liquidity_underlying as u128 * rift.total_liquidity_rift as u128 / new_underlying_reserve as u128) as u64;
        let rift_amount_out = rift.total_liquidity_rift - new_rift_reserve;
        
        require!(rift_amount_out >= min_rift_out, ErrorCode::SlippageTooHigh);
        
        // Transfer underlying tokens from user to pool
        let transfer_ctx = CpiContext::new(
            ctx.accounts.token_program.to_account_info(),
            Transfer {
                from: ctx.accounts.user_underlying.to_account_info(),
                to: ctx.accounts.pool_underlying.to_account_info(),
                authority: ctx.accounts.user.to_account_info(),
            },
        );
        token::transfer(transfer_ctx, underlying_amount)?;
        
        // Transfer rift tokens from pool to user
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
        
        // Update price and volume
        rift.current_price = (rift.total_liquidity_underlying as u128 * 10000 / rift.total_liquidity_rift as u128) as u64;
        rift.total_volume_24h = rift.total_volume_24h.checked_add(underlying_amount).unwrap_or(rift.total_volume_24h);
        
        // Collect trading fees
        rift.total_fees_collected = rift.total_fees_collected.checked_add(trading_fee).unwrap_or(rift.total_fees_collected);
        
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

    /// Swap rift tokens for underlying tokens
    pub fn swap_rift_for_underlying(
        ctx: Context<SwapRiftForUnderlying>,
        rift_amount: u64,
        min_underlying_out: u64,
    ) -> Result<()> {
        let rift = &mut ctx.accounts.rift;
        
        require!(!rift.is_paused, ErrorCode::RiftPaused);
        require!(!rift.reentrancy_guard, ErrorCode::ReentrancyDetected);
        rift.reentrancy_guard = true;
        
        require!(rift_amount > 0, ErrorCode::InvalidAmount);
        
        // Calculate underlying amount out using constant product formula
        let new_rift_reserve = rift.total_liquidity_rift + rift_amount;
        let new_underlying_reserve = (rift.total_liquidity_underlying as u128 * rift.total_liquidity_rift as u128 / new_rift_reserve as u128) as u64;
        let underlying_amount_out = rift.total_liquidity_underlying - new_underlying_reserve;
        
        // Apply trading fee
        let trading_fee = underlying_amount_out * rift.trading_fee_bps as u64 / 10000;
        let underlying_after_fee = underlying_amount_out - trading_fee;
        
        require!(underlying_after_fee >= min_underlying_out, ErrorCode::SlippageTooHigh);
        
        // Transfer rift tokens from user to pool
        let transfer_ctx = CpiContext::new(
            ctx.accounts.token_program.to_account_info(),
            Transfer {
                from: ctx.accounts.user_rift_tokens.to_account_info(),
                to: ctx.accounts.pool_rift_tokens.to_account_info(),
                authority: ctx.accounts.user.to_account_info(),
            },
        );
        token::transfer(transfer_ctx, rift_amount)?;
        
        // Transfer underlying tokens from pool to user
        let rift_key = rift.key();
        let pool_auth_seeds = &[
            b"pool_auth",
            rift_key.as_ref(),
            &[ctx.bumps.pool_authority],
        ];
        let signer_seeds = &[&pool_auth_seeds[..]];
        
        let transfer_underlying_ctx = CpiContext::new_with_signer(
            ctx.accounts.token_program.to_account_info(),
            Transfer {
                from: ctx.accounts.pool_underlying.to_account_info(),
                to: ctx.accounts.user_underlying.to_account_info(),
                authority: ctx.accounts.pool_authority.to_account_info(),
            },
            signer_seeds,
        );
        token::transfer(transfer_underlying_ctx, underlying_after_fee)?;
        
        // Update pool reserves
        rift.total_liquidity_underlying = new_underlying_reserve;
        rift.total_liquidity_rift = new_rift_reserve;
        
        // Update price and volume
        rift.current_price = (rift.total_liquidity_underlying as u128 * 10000 / rift.total_liquidity_rift as u128) as u64;
        rift.total_volume_24h = rift.total_volume_24h.checked_add(underlying_amount_out).unwrap_or(rift.total_volume_24h);
        
        // Collect trading fees
        rift.total_fees_collected = rift.total_fees_collected.checked_add(trading_fee).unwrap_or(rift.total_fees_collected);
        
        rift.reentrancy_guard = false;
        
        emit!(SwapExecuted {
            rift: rift.key(),
            user: ctx.accounts.user.key(),
            amount_in: rift_amount,
            amount_out: underlying_after_fee,
            fee_amount: trading_fee,
            is_underlying_to_rift: false,
        });
        
        Ok(())
    }

    // TODO: Add oracle update functions, governance functions, etc.
    // The remaining functions would be similar to the original but adapted for LP pool model
}

// Account structures for Jupiter/Meteora LP Pool model

#[derive(Accounts)]
pub struct CreateRiftPoolWithVanityMint<'info> {
    #[account(mut)]
    pub creator: Signer<'info>,
    
    #[account(
        init,
        payer = creator,
        space = 8 + RiftPool::INIT_SPACE,
        seeds = [b"rift", underlying_mint.key().as_ref(), creator.key().as_ref()],
        bump
    )]
    pub rift: Account<'info, RiftPool>,
    
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
        token::mint = underlying_mint,
        token::authority = pool_authority,
        seeds = [b"pool_underlying", rift.key().as_ref()],
        bump
    )]
    pub pool_underlying: Account<'info, TokenAccount>,
    
    /// Pool rift token account
    #[account(
        init,
        payer = creator,
        token::mint = rift_mint,
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
pub struct CreateRiftPool<'info> {
    #[account(mut)]
    pub creator: Signer<'info>,
    
    #[account(
        init,
        payer = creator,
        space = 8 + RiftPool::INIT_SPACE,
        seeds = [b"rift", underlying_mint.key().as_ref(), creator.key().as_ref()],
        bump
    )]
    pub rift: Account<'info, RiftPool>,
    
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
        token::mint = underlying_mint,
        token::authority = pool_authority,
        seeds = [b"pool_underlying", rift.key().as_ref()],
        bump
    )]
    pub pool_underlying: Account<'info, TokenAccount>,
    
    /// Pool rift token account
    #[account(
        init,
        payer = creator,
        token::mint = rift_mint,
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
pub struct AddLiquidity<'info> {
    #[account(mut)]
    pub user: Signer<'info>,
    
    #[account(mut)]
    pub rift: Account<'info, RiftPool>,
    
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
pub struct RemoveLiquidity<'info> {
    #[account(mut)]
    pub user: Signer<'info>,
    
    #[account(mut)]
    pub rift: Account<'info, RiftPool>,
    
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
    
    /// CHECK: PDA for pool authority
    #[account(
        seeds = [b"pool_auth", rift.key().as_ref()],
        bump
    )]
    pub pool_authority: UncheckedAccount<'info>,
    
    pub token_program: Program<'info, Token>,
}

#[derive(Accounts)]
pub struct SwapUnderlyingForRift<'info> {
    #[account(mut)]
    pub user: Signer<'info>,
    
    #[account(mut)]
    pub rift: Account<'info, RiftPool>,
    
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

#[derive(Accounts)]
pub struct SwapRiftForUnderlying<'info> {
    #[account(mut)]
    pub user: Signer<'info>,
    
    #[account(mut)]
    pub rift: Account<'info, RiftPool>,
    
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

// Data structures

#[account]
#[derive(Default)]
pub struct RiftPool {
    pub creator: Pubkey,
    pub underlying_mint: Pubkey,
    pub rift_mint: Pubkey,
    pub liquidity_pool: Pubkey, // Jupiter/Meteora pool address
    pub name: String, // 32 bytes max
    
    // Pool parameters
    pub trading_fee_bps: u16,
    pub protocol_fee_bps: u16,
    pub partner_wallet: Option<Pubkey>,
    pub dex_platform: DexPlatform,
    
    // Pool state
    pub total_liquidity_underlying: u64,
    pub total_liquidity_rift: u64,
    pub lp_token_supply: u64,
    pub current_price: u64, // Price in basis points (underlying/rift * 10000)
    
    // Timestamps
    pub created_at: i64,
    pub last_rebalance: i64,
    
    // Oracle system (same as original)
    pub oracle_prices: [PriceData; 10],
    pub price_index: u8,
    pub oracle_update_interval: u64,
    pub max_rebalance_interval: u64,
    pub arbitrage_threshold_bps: u16,
    pub last_oracle_update: i64,
    
    // Analytics
    pub total_volume_24h: u64,
    pub price_deviation: i64,
    pub arbitrage_opportunity_bps: u16,
    pub rebalance_count: u64,
    
    // Fee tracking
    pub total_fees_collected: u64,
    pub rifts_tokens_distributed: u64,
    pub rifts_tokens_burned: u64,
    
    // LP staking (same as original)
    pub total_lp_staked: u64,
    pub pending_rewards: u64,
    pub last_reward_distribution: i64,
    
    // Security
    pub reentrancy_guard: bool,
    pub is_paused: bool,
    pub pause_timestamp: i64,
    
    // External integration
    pub pending_fee_distribution: u64,
    pub last_governance_update: i64,
}

impl RiftPool {
    pub const INIT_SPACE: usize = 32 + // creator
        32 + // underlying_mint  
        32 + // rift_mint
        32 + // liquidity_pool
        36 + // name (4 + 32)
        2 +  // trading_fee_bps
        2 +  // protocol_fee_bps
        33 + // partner_wallet (1 + 32)
        1 +  // dex_platform
        8 +  // total_liquidity_underlying
        8 +  // total_liquidity_rift
        8 +  // lp_token_supply
        8 +  // current_price
        8 +  // created_at
        8 +  // last_rebalance
        320 + // oracle_prices (32 * 10)
        1 +  // price_index
        8 +  // oracle_update_interval
        8 +  // max_rebalance_interval
        2 +  // arbitrage_threshold_bps
        8 +  // last_oracle_update
        8 +  // total_volume_24h
        8 +  // price_deviation
        2 +  // arbitrage_opportunity_bps
        8 +  // rebalance_count
        8 +  // total_fees_collected
        8 +  // rifts_tokens_distributed
        8 +  // rifts_tokens_burned
        8 +  // total_lp_staked
        8 +  // pending_rewards
        8 +  // last_reward_distribution
        1 +  // reentrancy_guard
        1 +  // is_paused
        8 +  // pause_timestamp
        8 +  // pending_fee_distribution
        8 +  // last_governance_update
        64;  // padding

    // Method to create DEX pool on Jupiter or Meteora
    pub fn create_dex_pool(
        &mut self,
        creator: &AccountInfo,
        underlying_mint: &Account<Mint>,
        rift_mint: &Account<Mint>,
        initial_underlying: u64,
        initial_rift: u64,
        dex_platform: DexPlatform,
    ) -> Result<()> {
        match dex_platform {
            DexPlatform::Jupiter => {
                // Jupiter V6 integration
                self.create_jupiter_pool(creator, underlying_mint, rift_mint, initial_underlying, initial_rift)?;
            },
            DexPlatform::Meteora => {
                // Meteora DLMM integration
                self.create_meteora_pool(creator, underlying_mint, rift_mint, initial_underlying, initial_rift)?;
            },
            DexPlatform::Orca => {
                // Orca Whirlpools integration (optional)
                self.create_orca_pool(creator, underlying_mint, rift_mint, initial_underlying, initial_rift)?;
            },
        }
        Ok(())
    }

    fn create_jupiter_pool(
        &mut self,
        _creator: &AccountInfo,
        _underlying_mint: &Account<Mint>,
        _rift_mint: &Account<Mint>,
        _initial_underlying: u64,
        _initial_rift: u64,
    ) -> Result<()> {
        // Jupiter V6 pool creation logic
        // This would integrate with Jupiter's pool creation instructions
        msg!("Creating Jupiter V6 pool with initial liquidity");
        self.lp_token_supply = (_initial_underlying as u128 * _initial_rift as u128).sqrt() as u64;
        Ok(())
    }

    fn create_meteora_pool(
        &mut self,
        _creator: &AccountInfo,
        _underlying_mint: &Account<Mint>,
        _rift_mint: &Account<Mint>,
        _initial_underlying: u64,
        _initial_rift: u64,
    ) -> Result<()> {
        // Meteora DLMM pool creation logic
        msg!("Creating Meteora DLMM pool with initial liquidity");
        self.lp_token_supply = (_initial_underlying as u128 * _initial_rift as u128).sqrt() as u64;
        Ok(())
    }

    fn create_orca_pool(
        &mut self,
        _creator: &AccountInfo,
        _underlying_mint: &Account<Mint>,
        _rift_mint: &Account<Mint>,
        _initial_underlying: u64,
        _initial_rift: u64,
    ) -> Result<()> {
        // Orca Whirlpools pool creation logic
        msg!("Creating Orca Whirlpool with initial liquidity");
        self.lp_token_supply = (_initial_underlying as u128 * _initial_rift as u128).sqrt() as u64;
        Ok(())
    }
}

#[derive(AnchorSerialize, AnchorDeserialize, Clone, Copy, PartialEq, Eq)]
pub enum DexPlatform {
    Jupiter,
    Meteora,
    Orca,
}

impl Default for DexPlatform {
    fn default() -> Self {
        DexPlatform::Jupiter
    }
}

// Price data structure (reused from original)
#[derive(AnchorSerialize, AnchorDeserialize, Clone, Copy, Default)]
pub struct PriceData {
    pub price: u64,
    pub confidence: u64,
    pub timestamp: i64,
    pub oracle_source: u8, // 0 = Pyth, 1 = Switchboard, 2 = Jupiter
}

// Events
#[event]
pub struct RiftPoolCreated {
    pub rift: Pubkey,
    pub creator: Pubkey,
    pub underlying_mint: Pubkey,
    pub rift_mint: Pubkey,
    pub pool_address: Pubkey,
    pub initial_rift_amount: u64,
    pub initial_underlying_amount: u64,
    pub dex_platform: DexPlatform,
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
pub struct LiquidityRemoved {
    pub rift: Pubkey,
    pub user: Pubkey,
    pub lp_tokens_burned: u64,
    pub underlying_amount: u64,
    pub rift_amount: u64,
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
    #[msg("Trading fee too high")]
    InvalidTradingFee,
    #[msg("Protocol fee too high")]
    InvalidProtocolFee,
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
    #[msg("Slippage tolerance exceeded")]
    SlippageTooHigh,
    #[msg("No liquidity available")]
    NoLiquidity,
}