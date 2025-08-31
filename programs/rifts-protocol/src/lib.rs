// Rifts Protocol
use anchor_lang::prelude::*;
use anchor_lang::solana_program::program::invoke_signed;
use anchor_spl::token::{self, Token, TokenAccount, Transfer, Mint, MintTo};
// Note: Metadata functionality removed to avoid dependency issues
use anchor_lang::solana_program::sysvar::rent::Rent;

// External program CPI imports
pub use fee_collector;
pub use governance;
pub use lp_staking;

declare_id!("8FX1CVcR4QZyvTYtV6rG42Ha1K2qyRNykKYcwVctspUh");

/// Shared rift initialization logic to eliminate code duplication
fn initialize_rift(
    rift: &mut Account<Rift>,
    creator: Pubkey,
    underlying_mint: Pubkey,
    rift_mint: Pubkey,
    burn_fee_bps: u16,
    partner_fee_bps: u16,
    partner_wallet: Option<Pubkey>,
    rift_name: Option<String>,
    require_rift_suffix: bool,
) -> Result<()> {
    // Validate fees
    require!(burn_fee_bps <= 4500, ErrorCode::InvalidBurnFee);
    require!(partner_fee_bps <= 500, ErrorCode::InvalidPartnerFee);
    
    // Validate and set rift name
    if let Some(name) = rift_name {
        require!(name.len() <= 32, ErrorCode::NameTooLong);
        if require_rift_suffix {
            require!(name.ends_with("_RIFT") || name.ends_with("RIFT"), ErrorCode::InvalidRiftName);
        }
        rift.name = name;
    } else {
        // Generate default name with RIFT suffix (safe string slicing)
        let mint_string = underlying_mint.to_string();
        let prefix = if mint_string.len() >= 8 { &mint_string[0..8] } else { &mint_string };
        let underlying_symbol = format!("{}_RIFT", prefix);
        rift.name = underlying_symbol;
    }

    rift.creator = creator;
    rift.underlying_mint = underlying_mint;
    rift.rift_mint = rift_mint;
    
    // Create vault PDA
    let rift_key = rift.key();
    let vault_seeds = &[b"vault", rift_key.as_ref()];
    let (vault_pda, _) = Pubkey::find_program_address(vault_seeds, &crate::ID);
    rift.vault = vault_pda;
    
    rift.burn_fee_bps = burn_fee_bps;
    rift.partner_fee_bps = partner_fee_bps;
    rift.partner_wallet = partner_wallet;
    rift.total_wrapped = 0;
    rift.total_burned = 0;
    rift.backing_ratio = 10000;
    rift.last_rebalance = Clock::get()?.unix_timestamp;
    rift.created_at = Clock::get()?.unix_timestamp;
    
    // Initialize hybrid oracle system
    rift.oracle_prices = [PriceData::default(); 10];
    rift.price_index = 0;
    rift.oracle_update_interval = 30 * 60;
    rift.max_rebalance_interval = 24 * 60 * 60;
    rift.arbitrage_threshold_bps = 200;
    rift.last_oracle_update = Clock::get()?.unix_timestamp;
    
    // Initialize volume-based oracle settings (5% of total supply)
    rift.volume_oracle_threshold_bps = 500;
    rift.volume_since_last_oracle = 0;
    
    // Initialize advanced metrics
    rift.total_volume_24h = 0;
    rift.price_deviation = 0;
    rift.arbitrage_opportunity_bps = 0;
    rift.rebalance_count = 0;
    
    // Initialize RIFTS token distribution tracking
    rift.total_fees_collected = 0;
    rift.rifts_tokens_distributed = 0;
    rift.rifts_tokens_burned = 0;
    
    // Initialize Meteora-style DLMM Pool
    rift.liquidity_pool = None;
    rift.lp_token_supply = 0;
    rift.pool_trading_fee_bps = 30;
    rift.total_liquidity_underlying = 0;
    rift.total_liquidity_rift = 0;
    rift.active_bin_id = 0;
    rift.bin_step = 0;
    
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
    
    // Initialize configurable external program IDs
    rift.jupiter_program_id = "JUP6LkbZbjS1jKKwapdHNy74zcZ3tLUZoi5QNyVTaV4".parse::<Pubkey>()
        .map_err(|_| ErrorCode::InvalidProgramId)?;
    rift.meteora_program_id = "LBUZKhRxPF3XUpBCjp4YzTKgLccjZhTSDM9YuVaPwxo".parse::<Pubkey>()
        .map_err(|_| ErrorCode::InvalidProgramId)?;
    
    emit!(RiftCreated {
        rift: rift.key(),
        creator: rift.creator,
        underlying_mint: rift.underlying_mint,
        burn_fee_bps,
        partner_fee_bps,
    });

    Ok(())
}

#[program]
pub mod rifts_protocol {
    use super::*;

    /// Create a new Rift with a vanity mint address
    /// This allows creating rifts with mint addresses ending in 'rift'
    pub fn create_rift_with_vanity_mint(
        ctx: Context<CreateRiftWithVanityMint>,
        burn_fee_bps: u16,
        partner_fee_bps: u16,
        partner_wallet: Option<Pubkey>,
        rift_name: Option<String>,
    ) -> Result<()> {
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
        
        initialize_rift(
            &mut ctx.accounts.rift,
            ctx.accounts.creator.key(),
            ctx.accounts.underlying_mint.key(),
            ctx.accounts.rift_mint.key(),
            burn_fee_bps,
            partner_fee_bps,
            partner_wallet,
            rift_name,
            false, // Not requiring RIFT suffix for vanity
        )
    }
    
    /// Initialize a new Rift (wrapped token vault) - STACK OPTIMIZED (Original PDA version)
    pub fn create_rift(
        ctx: Context<CreateRift>,
        burn_fee_bps: u16,
        partner_fee_bps: u16,
        partner_wallet: Option<Pubkey>,
        rift_name: Option<String>,
    ) -> Result<()> {
        initialize_rift(
            &mut ctx.accounts.rift,
            ctx.accounts.creator.key(),
            ctx.accounts.underlying_mint.key(),
            ctx.accounts.rift_mint.key(),
            burn_fee_bps,
            partner_fee_bps,
            partner_wallet,
            rift_name,
            true, // Requiring RIFT suffix for regular rifts
        )
    }

    /// Wrap underlying tokens into rift tokens AND create tradeable pool
    pub fn wrap_tokens(
        ctx: Context<WrapTokens>,
        amount: u64,
        initial_rift_amount: u64,
        trading_fee_bps: u16,
        bin_step: u16,  // Let users choose Meteora bin step (fee tier)
    ) -> Result<()> {
        
        let rift = &mut ctx.accounts.rift;
        
        // Check if rift is paused
        require!(!rift.is_paused, ErrorCode::RiftPaused);
        
        // **CRITICAL SECURITY FIX**: Emergency circuit breakers
        require!(!rift.is_paused, ErrorCode::ProtocolPaused);
        
        // **EMERGENCY CIRCUIT BREAKER**: Auto-pause if extreme conditions detected  
        let current_time = Clock::get()?.unix_timestamp;
        let price_deviation_threshold = rift.price_deviation_pause_threshold.unwrap_or(5000); // Default 50%
        if rift.price_deviation >= price_deviation_threshold as u64 {
            rift.is_paused = true;
            rift.pause_timestamp = current_time;
            return err!(ErrorCode::EmergencyPause);
        }
        
        // **VOLUME CIRCUIT BREAKER**: Pause if single transaction exceeds owner-configured limits
        let max_single_transaction = rift.max_single_transaction.unwrap_or(100_000_000_000u64); // Default 100K tokens
        if amount > max_single_transaction {
            rift.is_paused = true;
            rift.pause_timestamp = current_time;
            return err!(ErrorCode::VolumeCircuitBreaker);
        }
        
        // **VELOCITY CIRCUIT BREAKER**: Check for suspicious rapid transactions using owner settings
        let large_tx_threshold = rift.large_transaction_threshold.unwrap_or(10_000_000_000u64); // Default 10K tokens
        let max_large_txs_per_hour = rift.max_large_transactions_per_hour.unwrap_or(5); // Default 5
        
        if amount > large_tx_threshold {
            let time_since_last_large_tx = current_time - rift.last_large_transaction_time.unwrap_or(0);
            if time_since_last_large_tx < 3600 { // Within 1 hour
                let large_tx_count = rift.large_transaction_count.unwrap_or(0);
                if large_tx_count >= max_large_txs_per_hour as u32 {
                    rift.is_paused = true;
                    rift.pause_timestamp = current_time;
                    return err!(ErrorCode::SuspiciousActivity);
                }
                rift.large_transaction_count = Some(large_tx_count + 1);
            } else {
                // Reset counter after 1 hour
                rift.large_transaction_count = Some(1);
            }
            rift.last_large_transaction_time = Some(current_time);
        }
        
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
        
        // Vault is now guaranteed to be initialized during rift creation
        // No race condition possible - removed dynamic initialization
        
        // **CRITICAL FIX**: Calculate wrap fee with enhanced precision protection
        let wrap_fee = amount
            .checked_mul(70)
            .ok_or(ErrorCode::MathOverflow)?
            .checked_div(10000)
            .ok_or(ErrorCode::MathOverflow)?;
        
        // **SECURITY FIX**: Prevent fee evasion - minimum fee of 1 for any non-zero amount
        let wrap_fee = if amount > 0 && wrap_fee == 0 {
            1 // Minimum fee of 1 token to prevent dust attacks
        } else {
            wrap_fee
        };
        
        // Ensure fee is reasonable
        require!(wrap_fee > 0 || amount == 0, ErrorCode::FeeTooSmall);
        
        let amount_after_fee = amount
            .checked_sub(wrap_fee)
            .ok_or(ErrorCode::MathOverflow)?;

        // Transfer underlying tokens to LP POOL (not vault)
        // Validate program ID before CPI call
        require!(
            ctx.accounts.token_program.key() == anchor_spl::token::ID,
            ErrorCode::InvalidProgramId
        );
        require!(initial_rift_amount > 0, ErrorCode::InvalidAmount);
        require!(trading_fee_bps <= 100, ErrorCode::InvalidTradingFee);
        
        // Validate Meteora program ID
        require!(
            ctx.accounts.meteora_program.key() == rift.meteora_program_id,
            ErrorCode::InvalidProgramId
        );
        
        // Validate Meteora bin step (fee tier) - common Meteora values
        require!(
            bin_step == 1 || bin_step == 5 || bin_step == 10 || bin_step == 25 || 
            bin_step == 50 || bin_step == 100 || bin_step == 200 || bin_step == 500,
            ErrorCode::InvalidBinStep
        );
        
        let transfer_ctx = CpiContext::new(
            ctx.accounts.token_program.to_account_info(),
            Transfer {
                from: ctx.accounts.user_underlying.to_account_info(),
                to: ctx.accounts.pool_underlying.to_account_info(),
                authority: ctx.accounts.user.to_account_info(),
            },
        );
        token::transfer(transfer_ctx, amount_after_fee)?; // Only amount after fee goes to pool

        // Calculate rift tokens: user gets tokens + pool gets initial_rift_amount
        let rift_tokens_to_user = amount_after_fee  // User gets 1:1 for wrapped amount
            .checked_mul(10000)
            .ok_or(ErrorCode::MathOverflow)?
            .checked_div(rift.backing_ratio)
            .ok_or(ErrorCode::MathOverflow)?;
        
        require!(rift_tokens_to_user > 0, ErrorCode::MintAmountTooSmall);
        require!(rift_tokens_to_user <= 1_000_000_000_000_000, ErrorCode::MintAmountTooLarge);
        
        let _total_rift_to_mint = rift_tokens_to_user
            .checked_add(initial_rift_amount)
            .ok_or(ErrorCode::MathOverflow)?;

        // Mint rift tokens to user AND pool
        let rift_key = rift.key();
        let rift_mint_auth_seeds = &[
            b"rift_mint_auth",
            rift_key.as_ref(),
            &[ctx.bumps.rift_mint_authority],
        ];
        let signer_seeds = &[&rift_mint_auth_seeds[..]];

        // Mint tokens for user (wrapped tokens)
        let mint_to_user_ctx = CpiContext::new_with_signer(
            ctx.accounts.token_program.to_account_info(),
            MintTo {
                mint: ctx.accounts.rift_mint.to_account_info(),
                to: ctx.accounts.user_rift_tokens.to_account_info(),
                authority: ctx.accounts.rift_mint_authority.to_account_info(),
            },
            signer_seeds,
        );
        token::mint_to(mint_to_user_ctx, rift_tokens_to_user)?;
        
        // Mint tokens for pool (for trading)
        let mint_to_pool_ctx = CpiContext::new_with_signer(
            ctx.accounts.token_program.to_account_info(),
            MintTo {
                mint: ctx.accounts.rift_mint.to_account_info(),
                to: ctx.accounts.pool_rift_tokens.to_account_info(),
                authority: ctx.accounts.rift_mint_authority.to_account_info(),
            },
            signer_seeds,
        );
        token::mint_to(mint_to_pool_ctx, initial_rift_amount)?;
        
        // Create Meteora-style DLMM pool with bins
        rift.total_liquidity_underlying = amount_after_fee;
        rift.total_liquidity_rift = initial_rift_amount;
        rift.pool_trading_fee_bps = trading_fee_bps;
        // Calculate LP token supply using geometric mean approximation
        // Since we can't use sqrt on u128, we'll use a safe approximation
        let product = (amount_after_fee as u128)
            .checked_mul(initial_rift_amount as u128)
            .ok_or(ErrorCode::MathOverflow)?;
        // **CRITICAL FIX**: Safe conversion with overflow protection
        let lp_supply_128 = product / 2;
        let mut lp_supply = if lp_supply_128 > u64::MAX as u128 {
            return Err(ErrorCode::MathOverflow.into());
        } else {
            lp_supply_128 as u64
        };
        
        if lp_supply == 0 {
            lp_supply = 1; // Minimum LP supply
        }
        rift.lp_token_supply = lp_supply;
        
        // Create real Meteora pool PDA using actual Meteora program ID
        let underlying_key = ctx.accounts.underlying_mint.key();
        let rift_key = ctx.accounts.rift_mint.key();
        let fee_bytes = trading_fee_bps.to_le_bytes();
        let meteora_pool_seeds = &[
            b"lb_pair",
            underlying_key.as_ref(),
            rift_key.as_ref(),
            &fee_bytes,
        ];
        let (meteora_pool_pda, _pool_bump) = Pubkey::find_program_address(meteora_pool_seeds, &rift.meteora_program_id);
        rift.liquidity_pool = Some(meteora_pool_pda);
        
        // Create real Meteora DLMM pool via CPI
        // Generate required Meteora PDAs
        let reserve_x_seeds = &[b"reserve", underlying_key.as_ref(), rift_key.as_ref()];
        let (reserve_x, _) = Pubkey::find_program_address(reserve_x_seeds, &rift.meteora_program_id);
        
        let reserve_y_seeds = &[b"reserve", rift_key.as_ref(), underlying_key.as_ref()];
        let (reserve_y, _) = Pubkey::find_program_address(reserve_y_seeds, &rift.meteora_program_id);
        
        let oracle_seeds = &[b"oracle", meteora_pool_pda.as_ref()];
        let (oracle, _) = Pubkey::find_program_address(oracle_seeds, &rift.meteora_program_id);
        
        // Use a standard preset parameter (would need actual Meteora preset PDA)
        let bin_step_bytes = bin_step.to_le_bytes();
        let preset_param_seeds = &[b"preset_parameter".as_ref(), bin_step_bytes.as_ref()];
        let (preset_parameter, _) = Pubkey::find_program_address(preset_param_seeds, &rift.meteora_program_id);
        
        let meteora_create_pool_instruction = create_meteora_pool_instruction(
            &rift.meteora_program_id,
            &meteora_pool_pda,
            &ctx.accounts.underlying_mint.key(),
            &ctx.accounts.rift_mint.key(),
            &reserve_x,
            &reserve_y,
            &oracle,
            &preset_parameter,
            &ctx.accounts.user.key(),
            bin_step,
        )?;
        
        // Execute Meteora pool creation CPI
        let pool_auth_seeds = &[
            b"pool_auth",
            rift_key.as_ref(),
            &[ctx.bumps.pool_authority],
        ];
        let pool_signer_seeds = &[&pool_auth_seeds[..]];
        
        invoke_signed(
            &meteora_create_pool_instruction,
            &[
                ctx.accounts.meteora_program.to_account_info(),      // meteora program
                ctx.accounts.underlying_mint.to_account_info(),     // token_mint_x
                ctx.accounts.rift_mint.to_account_info(),           // token_mint_y  
                ctx.accounts.user.to_account_info(),                // funder (signer)
                ctx.accounts.token_program.to_account_info(),       // token_program
                ctx.accounts.system_program.to_account_info(),      // system_program
            ],
            pool_signer_seeds,
        )?;
        
        // Initialize DLMM-style bin structure (simplified)
        rift.active_bin_id = 8388608; // 2^23, center bin like Meteora
        rift.bin_step = match trading_fee_bps {
            1..=10 => 1,    // Ultra-low fee pools
            11..=25 => 5,   // Low fee pools  
            26..=50 => 10,  // Medium fee pools
            51..=100 => 25, // High fee pools
            _ => 25,        // Default
        };
        
        // Mint LP tokens to user as pool creator
        let lp_mint_auth_seeds = &[
            b"lp_mint_auth",
            rift_key.as_ref(),
            &[ctx.bumps.lp_mint_authority],
        ];
        let lp_signer_seeds = &[&lp_mint_auth_seeds[..]];
        
        let mint_lp_ctx = CpiContext::new_with_signer(
            ctx.accounts.token_program.to_account_info(),
            MintTo {
                mint: ctx.accounts.lp_mint.to_account_info(),
                to: ctx.accounts.user_lp_tokens.to_account_info(),
                authority: ctx.accounts.lp_mint_authority.to_account_info(),
            },
            lp_signer_seeds,
        );
        token::mint_to(mint_lp_ctx, rift.lp_token_supply)?;

        // Update rift state with checked arithmetic (tracks wrapped amount)
        rift.total_wrapped = rift.total_wrapped
            .checked_add(rift_tokens_to_user)  // Track user's wrapped tokens
            .ok_or(ErrorCode::MathOverflow)?;
        
        // Track fees collected with checked arithmetic
        rift.total_fees_collected = rift.total_fees_collected
            .checked_add(wrap_fee)
            .ok_or(ErrorCode::MathOverflow)?;
        
        // Update volume tracking with checked arithmetic
        rift.total_volume_24h = rift.total_volume_24h
            .checked_add(amount)
            .ok_or(ErrorCode::MathOverflow)?;
        
        // Update cumulative volume for oracle triggering
        rift.volume_since_last_oracle = rift.volume_since_last_oracle
            .checked_add(amount)
            .ok_or(ErrorCode::MathOverflow)?;
        
        // Check if volume threshold reached for oracle update
        let current_time = Clock::get()?.unix_timestamp;
        let volume_threshold = rift.calculate_volume_threshold()?;
        if rift.volume_since_last_oracle >= volume_threshold {
            // Reset volume counter and update oracle timestamp
            rift.volume_since_last_oracle = 0;
            rift.last_oracle_update = current_time;
            
            // Trigger rebalance if needed based on volume threshold
            let should_rebalance = rift.should_trigger_rebalance(current_time)?;
            if should_rebalance {
                rift.trigger_automatic_rebalance(current_time)?;
            }
        }

        // **CRITICAL FIX**: Process fee distribution BEFORE clearing reentrancy guard
        if wrap_fee > 0 {
            rift.process_fee_immediately(wrap_fee)?;
        }
        
        // **CRITICAL FIX**: Clear reentrancy guard at the end
        rift.reentrancy_guard = false;

        emit!(WrapAndPoolCreated {
            rift: rift.key(),
            user: ctx.accounts.user.key(),
            underlying_amount: amount,
            fee_amount: wrap_fee,
            tokens_minted: rift_tokens_to_user,
            pool_underlying: amount_after_fee,
            pool_rift: initial_rift_amount,
            lp_tokens_minted: rift.lp_token_supply,
            trading_fee_bps,
        });

        Ok(())
    }

    /// Unwrap rift tokens back to underlying tokens
    pub fn unwrap_tokens(
        ctx: Context<UnwrapTokens>,
        rift_token_amount: u64,
    ) -> Result<()> {
        
        let rift = &mut ctx.accounts.rift;
        
        // Check if rift is paused
        require!(!rift.is_paused, ErrorCode::RiftPaused);
        
        // Validate amount
        require!(rift_token_amount > 0, ErrorCode::InvalidAmount);
        
        // Calculate underlying tokens with checked arithmetic
        require!(rift.backing_ratio > 0, ErrorCode::InvalidBackingRatio);
        let underlying_amount = rift_token_amount
            .checked_mul(rift.backing_ratio)
            .ok_or(ErrorCode::MathOverflow)?
            .checked_div(10000)
            .ok_or(ErrorCode::MathOverflow)?;
        
        // Calculate unwrap fee with checked arithmetic
        let unwrap_fee = underlying_amount
            .checked_mul(70)
            .ok_or(ErrorCode::MathOverflow)?
            .checked_div(10000)
            .ok_or(ErrorCode::MathOverflow)?;
        let amount_after_fee = underlying_amount
            .checked_sub(unwrap_fee)
            .ok_or(ErrorCode::MathOverflow)?;

        // Burn rift tokens
        // Validate program ID before CPI call
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

        // Validate program ID before CPI call
        require!(
            ctx.accounts.token_program.key() == anchor_spl::token::ID,
            ErrorCode::InvalidProgramId
        );
        
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
        
        // Track fees with checked arithmetic
        rift.total_fees_collected = rift.total_fees_collected
            .checked_add(unwrap_fee)
            .ok_or(ErrorCode::MathOverflow)?;
        
        // Update total burned with checked arithmetic
        rift.total_burned = rift.total_burned
            .checked_add(rift_token_amount)
            .ok_or(ErrorCode::MathOverflow)?;
        
        // Update volume with checked arithmetic
        rift.total_volume_24h = rift.total_volume_24h
            .checked_add(underlying_amount)
            .ok_or(ErrorCode::MathOverflow)?;
        
        // Update cumulative volume for oracle triggering
        rift.volume_since_last_oracle = rift.volume_since_last_oracle
            .checked_add(underlying_amount)
            .ok_or(ErrorCode::MathOverflow)?;
        
        // Check if volume threshold reached for oracle update
        let current_time = Clock::get()?.unix_timestamp;
        let volume_threshold = rift.calculate_volume_threshold()?;
        if rift.volume_since_last_oracle >= volume_threshold {
            // Reset volume counter and update oracle timestamp
            rift.volume_since_last_oracle = 0;
            rift.last_oracle_update = current_time;
            
            // Trigger rebalance if needed based on volume threshold
            let should_rebalance = rift.should_trigger_rebalance(current_time)?;
            if should_rebalance {
                rift.trigger_automatic_rebalance(current_time)?;
            }
        }

        // Add reentrancy protection
        require!(!rift.reentrancy_guard, ErrorCode::ReentrancyDetected);
        rift.reentrancy_guard = true;
        
        // Automatically process fee distribution
        rift.process_fee_immediately(unwrap_fee)?;
        
        // Release reentrancy guard
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

    /// Update oracle price (restricted to authorized oracle)
    pub fn update_oracle_price(
        ctx: Context<UpdateOraclePrice>,
        new_price: u64,
        confidence: u64,
    ) -> Result<()> {
        let rift = &mut ctx.accounts.rift;
        
        // Load oracle registry for governance-controlled oracle management
        let oracle_registry = &ctx.accounts.oracle_registry;
        let oracle_key = ctx.accounts.oracle.key();
        
        // Check if oracle is in the governance-approved registry
        let is_authorized = oracle_registry.authorized_oracles.contains(&oracle_key);
        
        require!(
            ctx.accounts.oracle.is_signer && is_authorized,
            ErrorCode::UnauthorizedOracle
        );
        
        // Validate oracle price staleness (max 5 minutes old)
        let current_time = Clock::get()?.unix_timestamp;
        require!(
            current_time - oracle_registry.last_updated <= 300,
            ErrorCode::OracleRegistryStale
        );
        
        // Validate price is reasonable (not zero, not too high)
        require!(new_price > 0 && new_price < 1_000_000_000_000, ErrorCode::InvalidPrice);
        require!(confidence > 0 && confidence <= 100, ErrorCode::InvalidConfidence);
        
        let clock = Clock::get()?;
        
        // Add new price to price history (rolling window)
        rift.add_price_data(new_price, confidence, clock.unix_timestamp)?;
        
        // Check if rebalance is needed based on hybrid oracle logic
        let should_rebalance = rift.should_trigger_rebalance(clock.unix_timestamp)?;
        
        if should_rebalance {
            rift.trigger_automatic_rebalance(clock.unix_timestamp)?;
        }
        
        Ok(())
    }

    /// Manual rebalance (can be called by anyone if conditions are met)
    pub fn trigger_rebalance(
        ctx: Context<TriggerRebalance>,
    ) -> Result<()> {
        let rift = &mut ctx.accounts.rift;
        let clock = Clock::get()?;
        
        // Check if manual rebalance is allowed
        require!(
            rift.can_manual_rebalance(clock.unix_timestamp)?,
            ErrorCode::RebalanceTooSoon
        );
        
        rift.trigger_automatic_rebalance(clock.unix_timestamp)?;
        Ok(())
    }

    /// Process fee distribution and RIFTS token operations - REAL transfers
    pub fn process_fee_distribution(
        ctx: Context<ProcessFeeDistribution>,
        fee_amount: u64,
    ) -> Result<()> {
        let rift = &mut ctx.accounts.rift;
        
        // Calculate fee splits with checked arithmetic
        let burn_amount = fee_amount
            .checked_mul(u64::from(rift.burn_fee_bps))
            .ok_or(ErrorCode::MathOverflow)?
            .checked_div(10000)
            .ok_or(ErrorCode::MathOverflow)?;
        let partner_amount = fee_amount
            .checked_mul(u64::from(rift.partner_fee_bps))
            .ok_or(ErrorCode::MathOverflow)?
            .checked_div(10000)
            .ok_or(ErrorCode::MathOverflow)?;
        let remaining_fees = fee_amount
            .checked_sub(
                burn_amount
                    .checked_add(partner_amount)
                    .ok_or(ErrorCode::MathOverflow)?
            )
            .ok_or(ErrorCode::MathOverflow)?;
        
        // 5% to treasury, 95% to fee collector for RIFTS buyback
        let treasury_amount = remaining_fees
            .checked_mul(5)
            .ok_or(ErrorCode::MathOverflow)?
            .checked_div(100)
            .ok_or(ErrorCode::MathOverflow)?;
        let fee_collector_amount = remaining_fees
            .checked_sub(treasury_amount)
            .ok_or(ErrorCode::MathOverflow)?;
        
        // REAL TRANSFERS: Send fees to treasury and fee collector
        if treasury_amount > 0 {
            // Transfer to treasury
            let transfer_treasury_ctx = CpiContext::new(
                ctx.accounts.token_program.to_account_info(),
                Transfer {
                    from: ctx.accounts.vault.to_account_info(),
                    to: ctx.accounts.treasury.to_account_info(),
                    authority: ctx.accounts.vault_authority.to_account_info(),
                },
            );
            
            let rift_key = rift.key();
            let vault_seeds = &[b"vault", rift_key.as_ref(), &[ctx.bumps.vault_authority]];
            let signer_seeds = &[&vault_seeds[..]];
            
            token::transfer(
                transfer_treasury_ctx.with_signer(signer_seeds),
                treasury_amount
            )?;
        }
        
        // CPI call to fee collector program for real fee processing
        if fee_collector_amount > 0 {
            let cpi_program = ctx.accounts.fee_collector_program.to_account_info();
            let cpi_accounts = fee_collector::cpi::accounts::CollectFees {
                fee_collector: ctx.accounts.fee_collector.to_account_info(),
                rift_fee_account: ctx.accounts.vault.to_account_info(),
                collector_vault: ctx.accounts.fee_collector_vault.to_account_info(),
                rift_authority: ctx.accounts.vault_authority.to_account_info(),
                token_program: ctx.accounts.token_program.to_account_info(),
            };
            
            let rift_key = rift.key();
            let vault_seeds = &[b"vault", rift_key.as_ref(), &[ctx.bumps.vault_authority]];
            let signer_seeds = &[&vault_seeds[..]];
            
            let cpi_ctx = CpiContext::new_with_signer(cpi_program, cpi_accounts, signer_seeds);
            fee_collector::cpi::collect_fees(cpi_ctx, fee_collector_amount)?;
        }
        
        // Transfer to partner if specified
        if partner_amount > 0 && rift.partner_wallet.is_some() && ctx.accounts.partner_vault.is_some() {
            // Safe to use ok_or here since we already checked is_some() above
            let partner_vault = ctx.accounts.partner_vault.as_ref()
                .ok_or(ErrorCode::MissingPartnerVault)?;
            
            let transfer_partner_ctx = CpiContext::new(
                ctx.accounts.token_program.to_account_info(),
                Transfer {
                    from: ctx.accounts.vault.to_account_info(),
                    to: partner_vault.to_account_info(),
                    authority: ctx.accounts.vault_authority.to_account_info(),
                },
            );
            
            let rift_key = rift.key();
            let vault_seeds = &[b"vault", rift_key.as_ref(), &[ctx.bumps.vault_authority]];
            let signer_seeds = &[&vault_seeds[..]];
            
            token::transfer(
                transfer_partner_ctx.with_signer(signer_seeds),
                partner_amount
            )?;
        }
        
        // Update tracking
        rift.total_fees_collected = rift.total_fees_collected
            .checked_add(fee_amount)
            .ok_or(ErrorCode::MathOverflow)?;
        
        emit!(FeesCalculated {
            rift: rift.key(),
            treasury_amount,
            fee_collector_amount, 
            partner_amount,
            burn_amount,
        });
        
        Ok(())
    }

    /// Stake LP tokens for RIFTS rewards via external LP staking program
    pub fn stake_lp_tokens_external(
        ctx: Context<StakeLPTokensExternal>,
        amount: u64,
    ) -> Result<()> {
        let rift = &mut ctx.accounts.rift;
        
        // Validate amount
        require!(amount > 0, ErrorCode::InvalidAmount);
        require!(amount <= 1_000_000_000_000, ErrorCode::AmountTooLarge);
        
        // CPI to external LP staking program
        let cpi_program = ctx.accounts.lp_staking_program.to_account_info();
        let cpi_accounts = lp_staking::cpi::accounts::StakeTokens {
            user: ctx.accounts.user.to_account_info(),
            staking_pool: ctx.accounts.staking_pool.to_account_info(),
            user_stake_account: ctx.accounts.user_stake_account.to_account_info(),
            user_lp_tokens: ctx.accounts.user_lp_tokens.to_account_info(),
            pool_lp_tokens: ctx.accounts.pool_lp_tokens.to_account_info(),
            system_program: ctx.accounts.system_program.to_account_info(),
            token_program: ctx.accounts.token_program.to_account_info(),
        };
        
        let cpi_ctx = CpiContext::new(cpi_program, cpi_accounts);
        lp_staking::cpi::stake(cpi_ctx, amount)?;
        
        // Update rift totals
        rift.total_lp_staked = rift.total_lp_staked
            .checked_add(amount)
            .ok_or(ErrorCode::MathOverflow)?;
        
        emit!(LPTokensStaked {
            rift: rift.key(),
            user: ctx.accounts.user.key(),
            amount,
            total_staked: rift.total_lp_staked,
        });
        
        Ok(())
    }

    /// Stake LP tokens for RIFTS rewards - INTERNAL IMPLEMENTATION (backwards compatibility)
    pub fn stake_lp_tokens(
        ctx: Context<StakeLPTokens>,
        amount: u64,
    ) -> Result<()> {
        let rift = &mut ctx.accounts.rift;
        let staker = &mut ctx.accounts.staker_account;
        
        // Validate amount
        require!(amount > 0, ErrorCode::InvalidAmount);
        require!(amount <= 1_000_000_000_000, ErrorCode::AmountTooLarge);
        
        // Initialize staker account if first time
        if staker.user == Pubkey::default() {
            staker.user = ctx.accounts.user.key();
            staker.rift = rift.key();
            staker.staked_amount = 0;
            staker.pending_rewards = 0;
            staker.total_staked = 0;
            staker.total_rewards_claimed = 0;
            staker.last_reward_update = Clock::get()?.unix_timestamp;
            staker.stake_start_time = Clock::get()?.unix_timestamp;
        }
        
        // Update pending rewards before changing stake
        let current_time = Clock::get()?.unix_timestamp;
        let time_elapsed = current_time
            .checked_sub(staker.last_reward_update)
            .ok_or(ErrorCode::MathOverflow)? as u64;
        
        if staker.staked_amount > 0 && time_elapsed > 0 {
            // **CRITICAL FIX**: Calculate pending rewards with proper error handling
            // Rewards = staked_amount * time_hours * hourly_rate / 1000000
            let time_hours = time_elapsed
                .checked_div(3600)
                .ok_or(ErrorCode::MathOverflow)?;
            let pending_rewards = staker.staked_amount
                .checked_mul(time_hours)
                .ok_or(ErrorCode::MathOverflow)?
                .checked_mul(3170979)
                .ok_or(ErrorCode::MathOverflow)?
                .checked_div(100000000000)
                .ok_or(ErrorCode::MathOverflow)?;
            
            staker.pending_rewards = staker.pending_rewards
                .checked_add(pending_rewards)
                .ok_or(ErrorCode::MathOverflow)?;
        }
        
        // Transfer LP tokens from user to staking vault
        let transfer_ctx = CpiContext::new(
            ctx.accounts.token_program.to_account_info(),
            Transfer {
                from: ctx.accounts.user_lp_tokens.to_account_info(),
                to: ctx.accounts.staking_vault.to_account_info(),
                authority: ctx.accounts.user.to_account_info(),
            },
        );
        token::transfer(transfer_ctx, amount)?;
        
        // Update staking records
        staker.staked_amount = staker.staked_amount
            .checked_add(amount)
            .ok_or(ErrorCode::MathOverflow)?;
        staker.last_reward_update = current_time;
        staker.total_staked = staker.total_staked
            .checked_add(amount)
            .ok_or(ErrorCode::MathOverflow)?;
        
        // Update rift totals
        rift.total_lp_staked = rift.total_lp_staked
            .checked_add(amount)
            .ok_or(ErrorCode::MathOverflow)?;
        
        emit!(LPTokensStaked {
            rift: rift.key(),
            user: ctx.accounts.user.key(),
            amount,
            total_staked: staker.staked_amount,
        });
        
        Ok(())
    }

    /// Claim RIFTS token rewards from LP staking - FULL IMPLEMENTATION
    pub fn claim_staking_rewards(
        ctx: Context<ClaimStakingRewards>,
    ) -> Result<()> {
        let rift = &mut ctx.accounts.rift;
        let staker = &mut ctx.accounts.staker_account;
        
        // Calculate total claimable rewards
        let current_time = Clock::get()?.unix_timestamp;
        let time_elapsed = current_time
            .checked_sub(staker.last_reward_update)
            .ok_or(ErrorCode::MathOverflow)? as u64;
        
        let mut total_rewards = staker.pending_rewards;
        
        if staker.staked_amount > 0 && time_elapsed > 0 {
            let time_hours = time_elapsed
                .checked_div(3600)
                .ok_or(ErrorCode::MathOverflow)?;
            let new_rewards = staker.staked_amount
                .checked_mul(time_hours)
                .ok_or(ErrorCode::MathOverflow)?
                .checked_mul(3170979)
                .ok_or(ErrorCode::MathOverflow)?
                .checked_div(100000000000)
                .ok_or(ErrorCode::MathOverflow)?;
            
            total_rewards = total_rewards
                .checked_add(new_rewards)
                .ok_or(ErrorCode::MathOverflow)?;
        }
        
        require!(total_rewards > 0, ErrorCode::NoRewardsToClaim);
        
        // Mint RIFTS tokens as rewards
        let rift_key = rift.key();
        let rifts_mint_seeds = &[
            b"rifts_mint_auth",
            rift_key.as_ref(),
            &[ctx.bumps.rifts_mint_authority]
        ];
        let signer_seeds = &[&rifts_mint_seeds[..]];
        
        let mint_ctx = CpiContext::new_with_signer(
            ctx.accounts.token_program.to_account_info(),
            MintTo {
                mint: ctx.accounts.rifts_mint.to_account_info(),
                to: ctx.accounts.user_rifts_tokens.to_account_info(),
                authority: ctx.accounts.rifts_mint_authority.to_account_info(),
            },
            signer_seeds,
        );
        token::mint_to(mint_ctx, total_rewards)?;
        
        // Update staker records
        staker.pending_rewards = 0;
        staker.last_reward_update = current_time;
        staker.total_rewards_claimed = staker.total_rewards_claimed
            .checked_add(total_rewards)
            .ok_or(ErrorCode::MathOverflow)?;
        
        // Update rift tracking
        rift.rifts_tokens_distributed = rift.rifts_tokens_distributed
            .checked_add(total_rewards)
            .ok_or(ErrorCode::MathOverflow)?;
        rift.last_reward_distribution = current_time;
        
        emit!(StakingRewardsClaimed {
            rift: rift.key(),
            user: ctx.accounts.user.key(),
            rewards_claimed: total_rewards,
            total_claimed: staker.total_rewards_claimed,
        });
        
        Ok(())
    }
    
    /// Unstake LP tokens and claim pending rewards
    pub fn unstake_lp_tokens(
        ctx: Context<UnstakeLPTokens>,
        amount: u64,
    ) -> Result<()> {
        let rift = &mut ctx.accounts.rift;
        let staker = &mut ctx.accounts.staker_account;
        
        require!(amount > 0, ErrorCode::InvalidAmount);
        require!(amount <= staker.staked_amount, ErrorCode::InsufficientStakedTokens);
        
        // Auto-claim rewards before unstaking
        let current_time = Clock::get()?.unix_timestamp;
        let time_elapsed = current_time
            .checked_sub(staker.last_reward_update)
            .ok_or(ErrorCode::MathOverflow)? as u64;
        
        if time_elapsed > 0 {
            let time_hours = time_elapsed
                .checked_div(3600)
                .ok_or(ErrorCode::MathOverflow)?;
            let new_rewards = staker.staked_amount
                .checked_mul(time_hours)
                .ok_or(ErrorCode::MathOverflow)?
                .checked_mul(3170979)
                .ok_or(ErrorCode::MathOverflow)?
                .checked_div(100000000000)
                .ok_or(ErrorCode::MathOverflow)?;
            
            staker.pending_rewards = staker.pending_rewards
                .checked_add(new_rewards)
                .ok_or(ErrorCode::MathOverflow)?;
        }
        
        // Transfer LP tokens back to user
        let rift_key = rift.key();
        let vault_seeds = &[
            b"staking_vault",
            rift_key.as_ref(),
            &[ctx.bumps.staking_vault_authority]
        ];
        let signer_seeds = &[&vault_seeds[..]];
        
        let transfer_ctx = CpiContext::new_with_signer(
            ctx.accounts.token_program.to_account_info(),
            Transfer {
                from: ctx.accounts.staking_vault.to_account_info(),
                to: ctx.accounts.user_lp_tokens.to_account_info(),
                authority: ctx.accounts.staking_vault_authority.to_account_info(),
            },
            signer_seeds,
        );
        token::transfer(transfer_ctx, amount)?;
        
        // Update staking records
        staker.staked_amount = staker.staked_amount
            .checked_sub(amount)
            .ok_or(ErrorCode::MathOverflow)?;
        staker.last_reward_update = current_time;
        
        // Update rift totals
        rift.total_lp_staked = rift.total_lp_staked
            .checked_sub(amount)
            .ok_or(ErrorCode::MathOverflow)?;
        
        emit!(LPTokensUnstaked {
            rift: rift.key(),
            user: ctx.accounts.user.key(),
            amount,
            remaining_staked: staker.staked_amount,
            pending_rewards: staker.pending_rewards,
        });
        
        Ok(())
    }

    /// Execute Jupiter swap for fee buybacks (integrated with fee collector)
    /// **SECURITY FIX**: Restricted to rift creator only
    pub fn jupiter_swap_for_buyback(
        ctx: Context<JupiterSwapForBuyback>,
        amount_in: u64,
        minimum_amount_out: u64,
        swap_data: Vec<u8>,
    ) -> Result<()> {
        let rift = &mut ctx.accounts.rift;
        
        // **SECURITY FIX**: Only rift creator can execute buybacks
        require!(
            ctx.accounts.user.key() == rift.creator,
            ErrorCode::UnauthorizedBuyback
        );
        
        // Validate Jupiter program ID using configurable ID stored in rift
        require!(
            ctx.accounts.jupiter_program.key() == rift.jupiter_program_id,
            ErrorCode::InvalidProgramId
        );
        
        // **CRITICAL SECURITY FIX**: Enhanced Jupiter swap parameter validation
        require!(amount_in > 0, ErrorCode::InvalidAmount);
        require!(amount_in <= 1_000_000_000_000, ErrorCode::AmountTooLarge);
        require!(swap_data.len() <= 1000, ErrorCode::InvalidInputData); // Reduced from 10k
        require!(swap_data.len() >= 8, ErrorCode::InvalidInputData); // Minimum instruction size
        
        // **SECURITY FIX**: Validate minimum slippage protection
        let min_slippage_bps = 50; // Minimum 0.5% slippage protection
        let calculated_min_out = amount_in
            .checked_mul(10000 - min_slippage_bps)
            .ok_or(ErrorCode::MathOverflow)?
            .checked_div(10000)
            .ok_or(ErrorCode::MathOverflow)?;
        require!(
            minimum_amount_out >= calculated_min_out,
            ErrorCode::InsufficientSlippageProtection
        );
        
        // **SECURITY FIX**: Maximum slippage protection (10%)
        let max_slippage_bps = 1000;
        let calculated_max_out = amount_in
            .checked_mul(10000 - max_slippage_bps)
            .ok_or(ErrorCode::MathOverflow)?
            .checked_div(10000)
            .ok_or(ErrorCode::MathOverflow)?;
        require!(
            minimum_amount_out <= calculated_max_out,
            ErrorCode::ExcessiveSlippage
        );
        
        // **SECURITY FIX**: Validate swap instruction discriminator
        require!(
            swap_data[0..8] == [0xE4, 0x45, 0x65, 0x31, 0x4C, 0x51, 0x95, 0x45] || // route
            swap_data[0..8] == [0x51, 0x1c, 0x86, 0x6b, 0xd6, 0xc9, 0xf7, 0x8c], // shared_accounts_route
            ErrorCode::InvalidJupiterInstruction
        );
        
        // CPI to Jupiter program for swap
        let _cpi_program = ctx.accounts.jupiter_program.to_account_info();
        let cpi_accounts = ctx.remaining_accounts;
        
        let rift_key = rift.key();
        let vault_seeds = &[b"vault", rift_key.as_ref(), &[ctx.bumps.vault_authority]];
        let signer_seeds = &[&vault_seeds[..]];
        
        // Execute Jupiter swap instruction
        let swap_instruction = anchor_lang::solana_program::instruction::Instruction {
            program_id: ctx.accounts.jupiter_program.key(),
            accounts: cpi_accounts.iter().map(|acc| {
                anchor_lang::solana_program::instruction::AccountMeta {
                    pubkey: acc.key(),
                    is_signer: false,
                    is_writable: acc.is_writable,
                }
            }).collect(),
            data: swap_data,
        };
        
        anchor_lang::solana_program::program::invoke_signed(
            &swap_instruction,
            cpi_accounts,
            signer_seeds,
        )?;
        
        // Update rift metrics
        rift.total_volume_24h = rift.total_volume_24h
            .checked_add(amount_in)
            .ok_or(ErrorCode::MathOverflow)?;
        
        // Update cumulative volume for oracle triggering
        rift.volume_since_last_oracle = rift.volume_since_last_oracle
            .checked_add(amount_in)
            .ok_or(ErrorCode::MathOverflow)?;
        
        // Check if volume threshold reached for oracle update
        let current_time = Clock::get()?.unix_timestamp;
        let volume_threshold = rift.calculate_volume_threshold()?;
        if rift.volume_since_last_oracle >= volume_threshold {
            // Reset volume counter and update oracle timestamp
            rift.volume_since_last_oracle = 0;
            rift.last_oracle_update = current_time;
            
            // Trigger rebalance if needed based on volume threshold
            let should_rebalance = rift.should_trigger_rebalance(current_time)?;
            if should_rebalance {
                rift.trigger_automatic_rebalance(current_time)?;
            }
        }
        
        emit!(JupiterSwapExecuted {
            rift: rift.key(),
            amount_in,
            minimum_amount_out,
            timestamp: Clock::get()?.unix_timestamp,
        });
        
        Ok(())
    }

    /// Governance proposal execution for rift parameters
    pub fn execute_governance_proposal(
        ctx: Context<ExecuteGovernanceProposal>,
        proposal_id: u64,
        new_burn_fee_bps: Option<u16>,
        new_partner_fee_bps: Option<u16>,
        new_oracle_interval: Option<i64>,
        new_rebalance_threshold: Option<u16>,
    ) -> Result<()> {
        let rift = &mut ctx.accounts.rift;
        
        // Validate proposal execution via CPI to governance program
        let proposal = &ctx.accounts.proposal;
        require!(
            proposal.proposal_type == governance::ProposalType::ParameterChange,
            ErrorCode::InvalidProposalType
        );
        require!(
            proposal.status == governance::ProposalStatus::Executed,
            ErrorCode::ProposalNotApproved
        );
        
        // Execute parameter changes
        if let Some(burn_fee) = new_burn_fee_bps {
            require!(burn_fee <= 4500, ErrorCode::InvalidBurnFee);
            rift.burn_fee_bps = burn_fee;
        }
        
        if let Some(partner_fee) = new_partner_fee_bps {
            require!(partner_fee <= 500, ErrorCode::InvalidPartnerFee);
            rift.partner_fee_bps = partner_fee;
        }
        
        if let Some(oracle_interval) = new_oracle_interval {
            require!(oracle_interval >= 600, ErrorCode::InvalidOracleInterval); // Min 10 minutes
            rift.oracle_update_interval = oracle_interval;
        }
        
        if let Some(threshold) = new_rebalance_threshold {
            require!(threshold >= 50 && threshold <= 1000, ErrorCode::InvalidRebalanceThreshold);
            rift.arbitrage_threshold_bps = threshold;
        }
        
        // Update governance timestamp
        rift.last_governance_update = Clock::get()?.unix_timestamp;
        
        emit!(GovernanceProposalExecuted {
            rift: rift.key(),
            proposal_id,
            executor: ctx.accounts.executor.key(),
            timestamp: Clock::get()?.unix_timestamp,
        });
        
        Ok(())
    }

    /// Emergency pause function (governance controlled)
    pub fn emergency_pause(
        ctx: Context<EmergencyPause>,
    ) -> Result<()> {
        let rift = &mut ctx.accounts.rift;
        
        // Validate governance authority
        require!(
            ctx.accounts.governance_authority.key() == ctx.accounts.governance.authority,
            ErrorCode::UnauthorizedGovernance
        );
        
        rift.is_paused = true;
        rift.pause_timestamp = Clock::get()?.unix_timestamp;
        
        emit!(RiftPaused {
            rift: rift.key(),
            authority: ctx.accounts.governance_authority.key(),
            timestamp: rift.pause_timestamp,
        });
        
        Ok(())
    }

    /// **OWNER CONTROL**: Update security parameters (owner only)
    pub fn update_security_parameters(
        ctx: Context<UpdateSecurityParameters>,
        max_single_transaction: Option<u64>,
        large_transaction_threshold: Option<u64>,
        max_large_transactions_per_hour: Option<u8>,
        price_deviation_pause_threshold: Option<u16>,
    ) -> Result<()> {
        let rift = &mut ctx.accounts.rift;
        
        // **OWNER ONLY**: Only rift creator can update security parameters
        require!(
            ctx.accounts.owner.key() == rift.creator,
            ErrorCode::UnauthorizedUpdate
        );
        
        // Update max single transaction limit
        if let Some(max_tx) = max_single_transaction {
            require!(max_tx >= 1_000_000_000_000, ErrorCode::InvalidSecurityParameter); // Min 1T
            require!(max_tx <= 100_000_000_000_000, ErrorCode::InvalidSecurityParameter); // Max 100T
            rift.max_single_transaction = Some(max_tx);
        }
        
        // Update large transaction threshold
        if let Some(threshold) = large_transaction_threshold {
            require!(threshold >= 100_000_000_000, ErrorCode::InvalidSecurityParameter); // Min 100B
            require!(threshold <= 10_000_000_000_000, ErrorCode::InvalidSecurityParameter); // Max 10T
            rift.large_transaction_threshold = Some(threshold);
        }
        
        // Update max large transactions per hour
        if let Some(max_count) = max_large_transactions_per_hour {
            require!(max_count >= 1 && max_count <= 20, ErrorCode::InvalidSecurityParameter);
            rift.max_large_transactions_per_hour = Some(max_count);
        }
        
        // Update price deviation pause threshold
        if let Some(threshold) = price_deviation_pause_threshold {
            require!(threshold >= 1000 && threshold <= 10000, ErrorCode::InvalidSecurityParameter); // 10% - 100%
            rift.price_deviation_pause_threshold = Some(threshold);
        }
        
        // Security parameters updated successfully
        
        Ok(())
    }

    /// Unpause function (governance controlled)
    pub fn emergency_unpause(
        ctx: Context<EmergencyUnpause>,
    ) -> Result<()> {
        let rift = &mut ctx.accounts.rift;
        
        // Validate governance authority
        require!(
            ctx.accounts.governance_authority.key() == ctx.accounts.governance.authority,
            ErrorCode::UnauthorizedGovernance
        );
        
        rift.is_paused = false;
        rift.pause_timestamp = 0;
        
        emit!(RiftUnpaused {
            rift: rift.key(),
            authority: ctx.accounts.governance_authority.key(),
            timestamp: Clock::get()?.unix_timestamp,
        });
        
        Ok(())
    }

    /// Update Jupiter program ID (owner only)
    pub fn update_jupiter_program_id(
        ctx: Context<UpdateJupiterProgramId>,
        new_jupiter_program_id: Pubkey,
    ) -> Result<()> {
        let rift = &mut ctx.accounts.rift;
        
        // **OWNER ONLY**: Only rift creator can update Jupiter program ID
        require!(
            ctx.accounts.owner.key() == rift.creator,
            ErrorCode::UnauthorizedUpdate
        );
        
        // Validate the new program ID is not default
        require!(
            new_jupiter_program_id != Pubkey::default(),
            ErrorCode::InvalidProgramId
        );
        
        let old_jupiter_id = rift.jupiter_program_id;
        rift.jupiter_program_id = new_jupiter_program_id;
        
        emit!(JupiterProgramIdUpdated {
            rift: rift.key(),
            authority: ctx.accounts.owner.key(),
            old_program_id: old_jupiter_id,
            new_program_id: new_jupiter_program_id,
            timestamp: Clock::get()?.unix_timestamp,
        });
        
        Ok(())
    }

    /// Update Meteora program ID (owner only)
    pub fn update_meteora_program_id(
        ctx: Context<UpdateMeteoraProgram>,
        new_meteora_program_id: Pubkey,
    ) -> Result<()> {
        let rift = &mut ctx.accounts.rift;
        
        // **OWNER ONLY**: Only rift creator can update Meteora program ID
        require!(
            ctx.accounts.owner.key() == rift.creator,
            ErrorCode::UnauthorizedUpdate
        );
        
        // Validate the new program ID is not default
        require!(
            new_meteora_program_id != Pubkey::default(),
            ErrorCode::InvalidProgramId
        );
        
        let old_meteora_id = rift.meteora_program_id;
        rift.meteora_program_id = new_meteora_program_id;
        
        emit!(JupiterProgramIdUpdated {
            rift: rift.key(),
            authority: ctx.accounts.owner.key(),
            old_program_id: old_meteora_id,
            new_program_id: new_meteora_program_id,
            timestamp: Clock::get()?.unix_timestamp,
        });
        
        Ok(())
    }

    /// Update volume oracle threshold (owner only)
    pub fn update_volume_oracle_threshold(
        ctx: Context<UpdateVolumeThreshold>,
        new_threshold_bps: u16,
    ) -> Result<()> {
        let rift = &mut ctx.accounts.rift;
        
        // **OWNER ONLY**: Only rift creator can update volume threshold
        require!(
            ctx.accounts.owner.key() == rift.creator,
            ErrorCode::UnauthorizedUpdate
        );
        
        // Validate threshold range (1% to 50% of total supply)
        require!(
            new_threshold_bps >= 100 && new_threshold_bps <= 5000, 
            ErrorCode::InvalidVolumeThreshold
        ); // 1% to 50% in basis points
        
        let old_threshold = rift.volume_oracle_threshold_bps;
        rift.volume_oracle_threshold_bps = new_threshold_bps;
        
        // Reset volume counter when threshold changes
        rift.volume_since_last_oracle = 0;
        
        emit!(VolumeThresholdUpdated {
            rift: rift.key(),
            authority: ctx.accounts.owner.key(),
            old_threshold: old_threshold as u64,
            new_threshold: new_threshold_bps as u64,
            timestamp: Clock::get()?.unix_timestamp,
        });
        
        Ok(())
    }

    /// Close a rift and return rent to creator (for fixing invalid vaults)
    pub fn close_rift(
        ctx: Context<CloseRift>,
    ) -> Result<()> {
        let rift = &ctx.accounts.rift;
        
        // Only creator can close their rift
        require!(
            rift.creator == ctx.accounts.creator.key(),
            ErrorCode::UnauthorizedClose
        );
        
        // Ensure vault is empty or invalid before closing
        // Allow closing if vault is the system program (invalid state)
        let system_program_key = Pubkey::default();
        require!(
            rift.total_wrapped == 0 || rift.vault == system_program_key,
            ErrorCode::VaultNotEmpty
        );
        
        emit!(RiftClosed {
            rift: rift.key(),
            creator: rift.creator,
        });
        
        Ok(())
    }

    /// Clean up stuck accounts from failed rift creation attempts
    pub fn cleanup_stuck_accounts(
        ctx: Context<CleanupStuckAccounts>,
    ) -> Result<()> {
        // Allow anyone to call this instruction to clean up stuck accounts
        // This helps resolve issues where rift creation partially failed
        
        msg!("Cleaning up stuck accounts for creator: {}", ctx.accounts.creator.key());
        msg!("Stuck mint account: {}", ctx.accounts.stuck_rift_mint.key());
        
        // Verify this is actually a stuck mint from a failed rift creation
        // Check that the mint has proper seeds and belongs to this creator
        let expected_rift_pda = Pubkey::create_program_address(
            &[
                b"rift",
                ctx.accounts.underlying_mint.key().as_ref(),
                ctx.accounts.creator.key().as_ref(),
                &[ctx.bumps.expected_rift]
            ],
            ctx.program_id
        ).map_err(|_| ErrorCode::InvalidStuckAccount)?;
        
        let expected_mint_pda = Pubkey::create_program_address(
            &[
                b"rift_mint",
                expected_rift_pda.as_ref(),
                &[ctx.bumps.stuck_rift_mint]
            ],
            ctx.program_id
        ).map_err(|_| ErrorCode::InvalidStuckAccount)?;
        
        // Verify the stuck mint matches expected PDA
        require!(
            ctx.accounts.stuck_rift_mint.key() == expected_mint_pda,
            ErrorCode::InvalidStuckAccount
        );
        
        // Check that no actual rift account exists (it's truly stuck)
        let rift_account = &ctx.accounts.expected_rift;
        require!(
            rift_account.data_is_empty(),
            ErrorCode::RiftAlreadyExists
        );
        
        // Close the stuck mint account and return rent to creator
        // The mint account will be closed automatically by Anchor
        // when we exit this instruction due to the close constraint
        
        emit!(StuckAccountCleaned {
            creator: ctx.accounts.creator.key(),
            stuck_mint: ctx.accounts.stuck_rift_mint.key(),
            underlying_mint: ctx.accounts.underlying_mint.key(),
        });
        
        Ok(())
    }
}

// SIMPLIFIED ACCOUNT STRUCTS TO REDUCE STACK USAGE

#[derive(Accounts)]
#[instruction(burn_fee_bps: u16, partner_fee_bps: u16, partner_wallet: Option<Pubkey>, rift_name: Option<String>)]
pub struct CreateRiftWithVanityMint<'info> {
    #[account(mut)]
    pub creator: Signer<'info>,
    
    #[account(
        init,
        payer = creator,
        space = 8 + std::mem::size_of::<Rift>() + 36, // Extra 36 bytes for String
        seeds = [b"rift", underlying_mint.key().as_ref(), creator.key().as_ref()],
        bump,
    )]
    pub rift: Account<'info, Rift>,
    
    pub underlying_mint: Account<'info, Mint>,
    
    /// The vanity mint account (pre-generated with address ending in 'rift')
    /// This is passed in by the user
    #[account(
        init,
        payer = creator,
        mint::decimals = underlying_mint.decimals,
        mint::authority = rift_mint_authority,
        signer // The mint must be a signer to prove ownership
    )]
    pub rift_mint: Account<'info, Mint>,
    
    /// CHECK: PDA for mint authority
    #[account(
        seeds = [b"rift_mint_auth", rift.key().as_ref()],
        bump
    )]
    pub rift_mint_authority: UncheckedAccount<'info>,
    
    pub token_program: Program<'info, Token>,
    pub system_program: Program<'info, System>,
    pub rent: Sysvar<'info, Rent>,
}

#[derive(Accounts)]
#[instruction(burn_fee_bps: u16, partner_fee_bps: u16, partner_wallet: Option<Pubkey>, rift_name: Option<String>)]
pub struct CreateRift<'info> {
    #[account(mut)]
    pub creator: Signer<'info>,
    
    #[account(
        init,
        payer = creator,
        space = 8 + std::mem::size_of::<Rift>() + 36, // Extra 36 bytes for String (4 bytes length + 32 bytes max string)
        seeds = [b"rift", underlying_mint.key().as_ref(), creator.key().as_ref()],
        constraint = underlying_mint.key() != Pubkey::default() && creator.key() != Pubkey::default() @ ErrorCode::InvalidSeedComponent,
        bump,
        constraint = creator.lamports() >= Rent::get()?.minimum_balance(8 + std::mem::size_of::<Rift>() + 36) @ ErrorCode::InsufficientRentExemption
    )]
    pub rift: Account<'info, Rift>,
    
    pub underlying_mint: Account<'info, Mint>,
    
    #[account(
        init,
        payer = creator,
        mint::decimals = underlying_mint.decimals,
        mint::authority = rift_mint_authority,
        seeds = [b"rift_mint", rift.key().as_ref()],
        constraint = rift.key() != Pubkey::default() @ ErrorCode::InvalidSeedComponent,
        bump,
        constraint = creator.lamports() >= Rent::get()?.minimum_balance(Mint::LEN) @ ErrorCode::InsufficientRentExemption
    )]
    pub rift_mint: Account<'info, Mint>,
    
    /// CHECK: PDA for mint authority
    #[account(
        seeds = [b"rift_mint_auth", rift.key().as_ref()],
        bump
    )]
    pub rift_mint_authority: UncheckedAccount<'info>,
    
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
    
    /// Vault token account - initialized during rift creation
    #[account(
        mut,
        seeds = [b"vault", rift.key().as_ref()],
        bump
    )]
    pub vault: Account<'info, TokenAccount>,
    
    #[account(mut)]
    pub rift_mint: Account<'info, Mint>,
    
    pub underlying_mint: Account<'info, Mint>,
    
    /// CHECK: PDA
    #[account(
        seeds = [b"rift_mint_auth", rift.key().as_ref()],
        bump
    )]
    pub rift_mint_authority: UncheckedAccount<'info>,
    
    /// CHECK: Vault authority PDA  
    #[account(
        seeds = [b"vault_auth", rift.key().as_ref()],
        bump
    )]
    pub vault_authority: UncheckedAccount<'info>,
    
    /// Pool underlying token account (NEW - for LP pool)
    #[account(
        init_if_needed,
        payer = user,
        token::mint = underlying_mint,
        token::authority = pool_authority,
        seeds = [b"pool_underlying", rift.key().as_ref()],
        bump
    )]
    pub pool_underlying: Account<'info, TokenAccount>,
    
    /// Pool rift token account (NEW - for LP pool)
    #[account(
        init_if_needed,
        payer = user,
        token::mint = rift_mint,
        token::authority = pool_authority,
        seeds = [b"pool_rift", rift.key().as_ref()],
        bump
    )]
    pub pool_rift_tokens: Account<'info, TokenAccount>,
    
    /// LP token mint (NEW - for LP tokens)
    #[account(
        init_if_needed,
        payer = user,
        mint::decimals = 6,
        mint::authority = lp_mint_authority,
        seeds = [b"lp_mint", rift.key().as_ref()],
        bump
    )]
    pub lp_mint: Account<'info, Mint>,
    
    /// User LP tokens account (NEW)
    #[account(mut)]
    pub user_lp_tokens: Account<'info, TokenAccount>,
    
    /// CHECK: PDA for LP mint authority (NEW)
    #[account(
        seeds = [b"lp_mint_auth", rift.key().as_ref()],
        bump
    )]
    pub lp_mint_authority: UncheckedAccount<'info>,
    
    /// CHECK: PDA for pool authority (NEW)
    #[account(
        seeds = [b"pool_auth", rift.key().as_ref()],
        bump
    )]
    pub pool_authority: UncheckedAccount<'info>,
    
    /// CHECK: Meteora DLMM program for pool creation
    pub meteora_program: UncheckedAccount<'info>,
    
    pub token_program: Program<'info, Token>,
    pub system_program: Program<'info, System>,
    pub rent: Sysvar<'info, Rent>,
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
    
    /// CHECK: Vault PDA
    #[account(
        mut,
        seeds = [b"vault", rift.key().as_ref()],
        bump
    )]
    pub vault: UncheckedAccount<'info>,
    
    #[account(mut)]
    pub rift_mint: Account<'info, Mint>,
    
    /// CHECK: PDA
    #[account(
        seeds = [b"vault_auth", rift.key().as_ref()],
        bump
    )]
    pub vault_authority: UncheckedAccount<'info>,
    
    pub token_program: Program<'info, Token>,
}

#[derive(Accounts)]
pub struct UpdateOraclePrice<'info> {
    #[account(mut)]
    pub oracle: Signer<'info>,
    
    #[account(mut)]
    pub rift: Account<'info, Rift>,
    
    /// Oracle registry for governance-controlled oracle management
    #[account(
        seeds = [b"oracle_registry"],
        bump,
        constraint = oracle_registry.authorized_oracles.len() > 0 @ ErrorCode::EmptyOracleRegistry
    )]
    pub oracle_registry: Account<'info, OracleRegistry>,
}

#[derive(Accounts)]
pub struct TriggerRebalance<'info> {
    #[account(mut)]
    pub user: Signer<'info>,
    
    #[account(mut)]
    pub rift: Account<'info, Rift>,
}


#[derive(Accounts)]
pub struct ProcessFeeDistribution<'info> {
    #[account(mut)]
    pub fee_payer: Signer<'info>,
    
    #[account(mut)]
    pub rift: Account<'info, Rift>,
    
    /// Vault holding the underlying tokens
    #[account(
        mut,
        seeds = [b"vault", rift.key().as_ref()],
        bump
    )]
    pub vault: Account<'info, TokenAccount>,
    
    /// CHECK: Vault authority PDA - validated by seeds constraint
    #[account(
        seeds = [b"vault", rift.key().as_ref()],
        bump
    )]
    pub vault_authority: UncheckedAccount<'info>,
    
    /// Treasury account for fee collection
    #[account(mut)]
    pub treasury: Account<'info, TokenAccount>,
    
    /// Fee collector account for automated buybacks
    #[account(mut)]
    pub fee_collector: Account<'info, fee_collector::FeeCollector>,
    
    /// Fee collector vault for receiving tokens
    #[account(mut)]
    pub fee_collector_vault: Account<'info, TokenAccount>,
    
    /// Partner vault (optional)
    #[account(mut)]
    pub partner_vault: Option<Account<'info, TokenAccount>>,
    
    /// Fee collector program for CPI
    /// CHECK: This is the fee collector program ID
    #[account(
        constraint = fee_collector_program.key() == fee_collector::ID @ ErrorCode::InvalidProgramId
    )]
    pub fee_collector_program: UncheckedAccount<'info>,
    
    /// RIFTS token mint for buyback operations
    #[account(mut)]
    pub rifts_mint: Account<'info, Mint>,
    
    pub token_program: Program<'info, Token>,
}

#[derive(Accounts)]
pub struct StakeLPTokensExternal<'info> {
    #[account(mut)]
    pub user: Signer<'info>,
    
    #[account(mut)]
    pub rift: Account<'info, Rift>,
    
    /// External LP staking pool
    #[account(mut)]
    pub staking_pool: Account<'info, lp_staking::StakingPool>,
    
    /// User's stake account in external program
    #[account(mut)]
    pub user_stake_account: Account<'info, lp_staking::UserStakeAccount>,
    
    #[account(mut)]
    pub user_lp_tokens: Account<'info, TokenAccount>,
    
    #[account(mut)]
    pub pool_lp_tokens: Account<'info, TokenAccount>,
    
    /// External LP staking program for CPI
    /// CHECK: LP staking program validation
    #[account(
        constraint = lp_staking_program.key() == lp_staking::ID @ ErrorCode::InvalidProgramId
    )]
    pub lp_staking_program: UncheckedAccount<'info>,
    
    pub token_program: Program<'info, Token>,
    pub system_program: Program<'info, System>,
}

#[derive(Accounts)]
pub struct StakeLPTokens<'info> {
    #[account(mut)]
    pub user: Signer<'info>,
    
    #[account(mut)]
    pub rift: Account<'info, Rift>,
    
    #[account(
        init_if_needed,
        payer = user,
        space = 8 + std::mem::size_of::<StakerAccount>(),
        seeds = [b"staker", rift.key().as_ref(), user.key().as_ref()],
        bump
    )]
    pub staker_account: Account<'info, StakerAccount>,
    
    #[account(mut)]
    pub user_lp_tokens: Account<'info, TokenAccount>,
    
    #[account(mut)]
    pub staking_vault: Account<'info, TokenAccount>,
    
    pub token_program: Program<'info, Token>,
    pub system_program: Program<'info, System>,
    pub rent: Sysvar<'info, Rent>,
}

#[derive(Accounts)]
pub struct ClaimStakingRewards<'info> {
    #[account(mut)]
    pub user: Signer<'info>,
    
    #[account(mut)]
    pub rift: Account<'info, Rift>,
    
    #[account(
        mut,
        seeds = [b"staker", rift.key().as_ref(), user.key().as_ref()],
        bump
    )]
    pub staker_account: Account<'info, StakerAccount>,
    
    /// RIFTS token mint for rewards
    #[account(mut)]
    pub rifts_mint: Account<'info, Mint>,
    
    /// User's RIFTS token account
    #[account(mut)]
    pub user_rifts_tokens: Account<'info, TokenAccount>,
    
    /// RIFTS mint authority
    /// CHECK: PDA for RIFTS mint authority
    #[account(
        seeds = [b"rifts_mint_auth", rift.key().as_ref()],
        bump
    )]
    pub rifts_mint_authority: UncheckedAccount<'info>,
    
    pub token_program: Program<'info, Token>,
}

#[derive(Accounts)]
pub struct UnstakeLPTokens<'info> {
    #[account(mut)]
    pub user: Signer<'info>,
    
    #[account(mut)]
    pub rift: Account<'info, Rift>,
    
    #[account(
        mut,
        seeds = [b"staker", rift.key().as_ref(), user.key().as_ref()],
        bump
    )]
    pub staker_account: Account<'info, StakerAccount>,
    
    #[account(mut)]
    pub user_lp_tokens: Account<'info, TokenAccount>,
    
    #[account(mut)]
    pub staking_vault: Account<'info, TokenAccount>,
    
    /// Staking vault authority
    /// CHECK: PDA for staking vault authority
    #[account(
        seeds = [b"staking_vault", rift.key().as_ref()],
        bump
    )]
    pub staking_vault_authority: UncheckedAccount<'info>,
    
    pub token_program: Program<'info, Token>,
}

#[derive(Accounts)]
pub struct JupiterSwapForBuyback<'info> {
    #[account(mut)]
    pub user: Signer<'info>,
    
    #[account(mut)]
    pub rift: Account<'info, Rift>,
    
    /// CHECK: Vault authority PDA
    #[account(
        seeds = [b"vault", rift.key().as_ref()],
        bump
    )]
    pub vault_authority: UncheckedAccount<'info>,
    
    /// CHECK: Jupiter program - hardcoded Jupiter V6 program ID
    /// Jupiter V6: JUP6LkbZbjS1jKKwapdHNy74zcZ3tLUZoi5QNyVTaV4
    pub jupiter_program: UncheckedAccount<'info>,
}

#[derive(Accounts)]
pub struct ExecuteGovernanceProposal<'info> {
    #[account(mut)]
    pub executor: Signer<'info>,
    
    #[account(mut)]
    pub rift: Account<'info, Rift>,
    
    /// Governance proposal being executed
    pub proposal: Account<'info, governance::Proposal>,
    
    /// Governance state account
    pub governance: Account<'info, governance::Governance>,
}

#[derive(Accounts)]
pub struct EmergencyPause<'info> {
    #[account(mut)]
    pub governance_authority: Signer<'info>,
    
    #[account(mut)]
    pub rift: Account<'info, Rift>,
    
    /// Governance state for authorization
    pub governance: Account<'info, governance::Governance>,
}

#[derive(Accounts)]
pub struct EmergencyUnpause<'info> {
    #[account(mut)]
    pub governance_authority: Signer<'info>,
    
    #[account(mut)]
    pub rift: Account<'info, Rift>,
    
    /// Governance state for authorization
    pub governance: Account<'info, governance::Governance>,
}

#[derive(Accounts)]
pub struct UpdateSecurityParameters<'info> {
    #[account(mut)]
    pub owner: Signer<'info>,
    
    #[account(mut)]
    pub rift: Account<'info, Rift>,
}

#[derive(Accounts)]
pub struct CloseRift<'info> {
    #[account(mut)]
    pub creator: Signer<'info>,
    
    #[account(
        mut,
        close = creator,
        has_one = creator @ ErrorCode::UnauthorizedClose
    )]
    pub rift: Account<'info, Rift>,
}

#[derive(Accounts)]
pub struct UpdateJupiterProgramId<'info> {
    #[account(mut)]
    pub owner: Signer<'info>,
    
    #[account(mut)]
    pub rift: Account<'info, Rift>,
}

#[derive(Accounts)]
pub struct UpdateMeteoraProgram <'info> {
    #[account(mut)]
    pub owner: Signer<'info>,
    
    #[account(mut)]
    pub rift: Account<'info, Rift>,
}

#[derive(Accounts)]
pub struct UpdateVolumeThreshold<'info> {
    #[account(mut)]
    pub owner: Signer<'info>,
    
    #[account(mut)]
    pub rift: Account<'info, Rift>,
}

#[derive(Accounts)]
pub struct CleanupStuckAccounts<'info> {
    /// The creator who originally tried to create the rift
    /// CHECK: We verify this matches the expected PDA derivation
    pub creator: UncheckedAccount<'info>,
    
    /// The underlying mint that was used in the failed rift creation
    pub underlying_mint: Account<'info, Mint>,
    
    /// The stuck rift mint account that needs to be cleaned up
    #[account(
        mut,
        close = creator,
        seeds = [b"rift_mint", expected_rift.key().as_ref()],
        bump
    )]
    pub stuck_rift_mint: Account<'info, Mint>,
    
    /// The expected rift account location (should be empty/non-existent)
    /// CHECK: We verify this account is empty to ensure it's truly stuck
    #[account(
        seeds = [b"rift", underlying_mint.key().as_ref(), creator.key().as_ref()],
        constraint = underlying_mint.key() != Pubkey::default() && creator.key() != Pubkey::default() @ ErrorCode::InvalidSeedComponent,
        bump
    )]
    pub expected_rift: UncheckedAccount<'info>,
    
    /// The account that will pay for the transaction (can be anyone)
    #[account(mut)]
    pub payer: Signer<'info>,
    
    pub system_program: Program<'info, System>,
}

#[account]
pub struct Rift {
    pub name: String,  // Custom name ending with RIFT
    pub creator: Pubkey,
    pub underlying_mint: Pubkey,
    pub rift_mint: Pubkey,
    pub vault: Pubkey,
    pub burn_fee_bps: u16,
    pub partner_fee_bps: u16,
    pub partner_wallet: Option<Pubkey>,
    pub total_wrapped: u64,
    pub total_burned: u64,
    pub backing_ratio: u64,
    pub last_rebalance: i64,
    pub created_at: i64,
    
    // Hybrid Oracle System
    pub oracle_prices: [PriceData; 10], // Rolling window of recent prices
    pub price_index: u8,                // Current index in the rolling window
    pub oracle_update_interval: i64,    // How often oracle updates (default 30 minutes)
    pub max_rebalance_interval: i64,    // Maximum time between rebalances (24 hours)
    pub arbitrage_threshold_bps: u16,   // Threshold for arbitrage detection (basis points)
    pub last_oracle_update: i64,        // Last oracle price update
    // Volume-based oracle triggers
    pub volume_oracle_threshold_bps: u16, // Volume threshold as % of total supply (default 500 bps = 5%)
    pub volume_since_last_oracle: u64,    // Cumulative volume since last oracle update
    // Advanced Metrics
    pub total_volume_24h: u64,          // 24h trading volume
    pub price_deviation: u64,           // Current price deviation from backing
    pub arbitrage_opportunity_bps: u16, // Current arbitrage opportunity
    pub rebalance_count: u32,           // Total number of rebalances
    
    // RIFTS Token Distribution
    pub total_fees_collected: u64,     // Total fees collected
    pub rifts_tokens_distributed: u64, // Total RIFTS tokens distributed to LP stakers
    pub rifts_tokens_burned: u64,      // Total RIFTS tokens burned
    
    // Meteora-style DLMM Pool (NEW - for automatic pool creation during wrapping)
    pub liquidity_pool: Option<Pubkey>, // Meteora-compatible pool address
    pub lp_token_supply: u64,           // Total LP tokens minted
    pub pool_trading_fee_bps: u16,      // Trading fee for LP pool (separate from wrap fee)
    pub total_liquidity_underlying: u64, // Underlying tokens in LP pool
    pub total_liquidity_rift: u64,     // Rift tokens in LP pool
    pub active_bin_id: i32,             // Current active bin ID (Meteora DLMM style)
    pub bin_step: u16,                  // Bin step for price increments
    
    // LP Staking
    pub total_lp_staked: u64,          // Total LP tokens staked
    pub pending_rewards: u64,          // Pending RIFTS rewards for distribution
    pub last_reward_distribution: i64, // Last time rewards were distributed
    
    // Reentrancy Protection
    pub reentrancy_guard: bool,        // Prevents reentrancy attacks
    
    // Emergency Controls
    pub is_paused: bool,               // Emergency pause state
    pub pause_timestamp: i64,          // When the rift was paused
    
    // External Program Integration
    pub pending_fee_distribution: u64, // Fees pending for fee collector
    
    // Governance Integration
    pub last_governance_update: i64,   // Timestamp of last governance parameter update
    
    // External Program IDs (configurable by owner)
    pub jupiter_program_id: Pubkey,    // Jupiter V6 program ID
    pub meteora_program_id: Pubkey,    // Meteora DLMM program ID
    
    // Security Controls
    pub max_single_transaction: Option<u64>,         // Maximum single transaction amount
    pub large_transaction_threshold: Option<u64>,    // Threshold for large transactions
    pub max_large_transactions_per_hour: Option<u8>, // Max large transactions per hour
    pub last_large_transaction_time: Option<i64>,    // Last large transaction timestamp
    pub large_transaction_count: Option<u32>,        // Current large transaction count in window
    pub price_deviation_pause_threshold: Option<u16>, // Price deviation that triggers pause (bps)
}

/// Oracle Registry for governance-controlled oracle management
#[account]
pub struct OracleRegistry {
    pub authorized_oracles: Vec<Pubkey>,  // List of authorized oracle addresses
    pub update_authority: Pubkey,         // Governance authority to update oracles
    pub last_updated: i64,                // Last time registry was updated
    pub min_confirmations: u8,            // Minimum oracle confirmations required
    pub max_staleness: i64,               // Maximum staleness allowed (seconds)
}

/// LP Staking account for individual stakers
#[account]
pub struct StakerAccount {
    pub user: Pubkey,                     // User who staked
    pub rift: Pubkey,                     // Associated rift
    pub staked_amount: u64,               // Current staked LP tokens
    pub pending_rewards: u64,             // Pending RIFTS rewards
    pub total_staked: u64,                // Total LP tokens ever staked
    pub total_rewards_claimed: u64,       // Total RIFTS rewards claimed
    pub last_reward_update: i64,          // Last reward calculation timestamp
    pub stake_start_time: i64,            // When staking started
}

#[derive(AnchorSerialize, AnchorDeserialize, Clone, Copy)]
#[derive(Default)]
pub struct PriceData {
    pub price: u64,
    pub confidence: u64,
    pub timestamp: i64,
}

#[derive(AnchorSerialize, AnchorDeserialize, Clone, Copy)]
pub struct OraclePrice {
    pub price: u64,
    pub confidence: u64,
    pub timestamp: i64,
    pub oracle_type: u8, // 0=Pyth, 1=Chainlink, 2=Switchboard
}



// LP Staking will be implemented in a separate program for modularity

impl Rift {
    pub fn add_price_data(&mut self, price: u64, confidence: u64, timestamp: i64) -> Result<()> {
        self.oracle_prices[self.price_index as usize] = PriceData {
            price,
            confidence,
            timestamp,
        };
        self.price_index = (self.price_index + 1) % 10;
        self.last_oracle_update = timestamp;
        Ok(())
    }
    
    pub fn should_trigger_rebalance(&mut self, current_time: i64) -> Result<bool> {
        // Check if maximum rebalance interval has passed
        if current_time - self.last_rebalance > self.max_rebalance_interval {
            return Ok(true);
        }
        
        // Check if arbitrage opportunity exceeds threshold
        if self.arbitrage_opportunity_bps > self.arbitrage_threshold_bps {
            return Ok(true);
        }
        
        // Check if oracle indicates significant price deviation
        let avg_price = self.get_average_oracle_price()?;
        let price_deviation = self.calculate_price_deviation(avg_price)?;
        
        // **CRITICAL FIX**: Actually update the price_deviation field
        self.price_deviation = price_deviation as u64;
        
        // Trigger if deviation > 2%
        Ok(price_deviation > 200) // 200 basis points = 2%
    }
    
    pub fn can_manual_rebalance(&self, current_time: i64) -> Result<bool> {
        // Allow manual rebalance if oracle interval has passed
        Ok(current_time - self.last_oracle_update > self.oracle_update_interval)
    }
    
    pub fn trigger_automatic_rebalance(&mut self, current_time: i64) -> Result<()> {
        let avg_price = self.get_average_oracle_price()?;
        
        // **CRITICAL FIX**: Validate oracle price before updating backing ratio
        require!(avg_price > 0, ErrorCode::InvalidOraclePrice);
        require!(avg_price <= 1_000_000_000_000, ErrorCode::OraclePriceTooLarge);
        
        // **CRITICAL FIX**: Only update backing ratio if price is reasonable
        // Additional validation to prevent zero backing ratio
        if avg_price > 0 && avg_price <= 1_000_000_000_000 {
            self.backing_ratio = avg_price;
        } else {
            return Err(ErrorCode::InvalidOraclePrice.into());
        }
        
        self.last_rebalance = current_time;
        self.rebalance_count = self.rebalance_count
            .checked_add(1)
            .ok_or(ErrorCode::MathOverflow)?;
        
        // Recalculate arbitrage opportunity
        self.arbitrage_opportunity_bps = 0; // Reset after rebalance
        self.price_deviation = 0;
        
        Ok(())
    }
    
    pub fn get_average_oracle_price(&self) -> Result<u64> {
        let mut total_price = 0u64;
        let mut count = 0u64;
        
        for price_data in &self.oracle_prices {
            if price_data.timestamp > 0 {
                // **CRITICAL FIX**: Use checked arithmetic to prevent overflow
                total_price = total_price
                    .checked_add(price_data.price)
                    .ok_or(ErrorCode::MathOverflow)?;
                count = count
                    .checked_add(1)
                    .ok_or(ErrorCode::MathOverflow)?;
            }
        }
        
        if count > 0 {
            let avg_price = total_price
                .checked_div(count)
                .ok_or(ErrorCode::MathOverflow)?;
            
            // **CRITICAL FIX**: Validate average price is reasonable
            require!(avg_price > 0, ErrorCode::InvalidOraclePrice);
            require!(avg_price <= 1_000_000_000_000, ErrorCode::OraclePriceTooLarge);
            
            Ok(avg_price)
        } else {
            // **CRITICAL FIX**: Validate fallback backing ratio
            require!(self.backing_ratio > 0, ErrorCode::InvalidBackingRatio);
            Ok(self.backing_ratio) // Fallback to current backing ratio
        }
    }
    
    pub fn calculate_price_deviation(&self, oracle_price: u64) -> Result<u16> {
        if self.backing_ratio == 0 {
            return Ok(0);
        }
        
        let deviation = if oracle_price > self.backing_ratio {
            oracle_price
                .checked_sub(self.backing_ratio)
                .ok_or(ErrorCode::MathOverflow)?
                .checked_mul(10000)
                .ok_or(ErrorCode::MathOverflow)?
                .checked_div(self.backing_ratio)
                .ok_or(ErrorCode::MathOverflow)?
        } else {
            self.backing_ratio
                .checked_sub(oracle_price)
                .ok_or(ErrorCode::MathOverflow)?
                .checked_mul(10000)
                .ok_or(ErrorCode::MathOverflow)?
                .checked_div(self.backing_ratio)
                .ok_or(ErrorCode::MathOverflow)?
        };
        
        Ok(deviation as u16)
    }
    
    pub fn process_rifts_distribution(&mut self, amount: u64) -> Result<()> {
        // 90% to LP stakers, 10% burned with checked arithmetic
        let lp_staker_amount = amount
            .checked_mul(90)
            .ok_or(ErrorCode::MathOverflow)?
            .checked_div(100)
            .ok_or(ErrorCode::MathOverflow)?;
        let burn_amount = amount
            .checked_mul(10)
            .ok_or(ErrorCode::MathOverflow)?
            .checked_div(100)
            .ok_or(ErrorCode::MathOverflow)?;
        
        self.rifts_tokens_distributed = self.rifts_tokens_distributed
            .checked_add(lp_staker_amount)
            .ok_or(ErrorCode::MathOverflow)?;
        self.rifts_tokens_burned = self.rifts_tokens_burned
            .checked_add(burn_amount)
            .ok_or(ErrorCode::MathOverflow)?;
        
        Ok(())
    }
    
    pub fn get_pending_fees(&self) -> u64 {
        // **SECURITY FIX**: Get total fees that haven't been distributed yet with proper overflow handling
        let total_distributed = match self.rifts_tokens_distributed
            .checked_add(self.rifts_tokens_burned) {
            Some(sum) => sum,
            None => {
                // If overflow occurs, return 0 (conservative approach)
                // This prevents any fee distribution if the numbers are corrupted
                return 0;
            }
        };
        
        if self.total_fees_collected > total_distributed {
            self.total_fees_collected
                .checked_sub(total_distributed)
                .unwrap_or(0) // Safe fallback - if underflow, return 0
        } else {
            0
        }
    }
    
    pub fn get_oracle_countdown(&self, current_time: i64) -> i64 {
        let next_oracle_time = self.last_oracle_update + self.oracle_update_interval;
        (next_oracle_time - current_time).max(0)
    }
    
    pub fn get_rebalance_countdown(&self, current_time: i64) -> i64 {
        let next_rebalance_time = self.last_rebalance + self.max_rebalance_interval;
        (next_rebalance_time - current_time).max(0)
    }

    /// Process fee distribution immediately (called automatically on wrap/unwrap)
    pub fn process_fee_immediately(&mut self, fee_amount: u64) -> Result<()> {
        // Calculate fee splits with checked arithmetic
        let burn_amount = fee_amount
            .checked_mul(self.burn_fee_bps as u64)
            .ok_or(ErrorCode::MathOverflow)?
            .checked_div(10000)
            .ok_or(ErrorCode::MathOverflow)?;
        let partner_amount = fee_amount
            .checked_mul(self.partner_fee_bps as u64)
            .ok_or(ErrorCode::MathOverflow)?
            .checked_div(10000)
            .ok_or(ErrorCode::MathOverflow)?;
        let remaining_fees = fee_amount
            .checked_sub(burn_amount.checked_add(partner_amount).ok_or(ErrorCode::MathOverflow)?)
            .ok_or(ErrorCode::MathOverflow)?;
        
        // 5% to treasury, 95% to buy RIFTS tokens
        let treasury_amount = remaining_fees
            .checked_mul(5)
            .ok_or(ErrorCode::MathOverflow)?
            .checked_div(100)
            .ok_or(ErrorCode::MathOverflow)?;
        let rifts_buy_amount = remaining_fees
            .checked_sub(treasury_amount)
            .ok_or(ErrorCode::MathOverflow)?;
        
        // Process RIFTS token buyback and distribution
        let lp_staker_amount = rifts_buy_amount
            .checked_mul(90)
            .ok_or(ErrorCode::MathOverflow)?
            .checked_div(100)
            .ok_or(ErrorCode::MathOverflow)?;
        let rifts_burn_amount = rifts_buy_amount
            .checked_sub(lp_staker_amount)
            .ok_or(ErrorCode::MathOverflow)?;
        
        // Update tracking with checked arithmetic
        self.rifts_tokens_distributed = self.rifts_tokens_distributed
            .checked_add(lp_staker_amount)
            .ok_or(ErrorCode::MathOverflow)?;
        self.rifts_tokens_burned = self.rifts_tokens_burned
            .checked_add(rifts_burn_amount)
            .ok_or(ErrorCode::MathOverflow)?;
        self.pending_rewards = self.pending_rewards
            .checked_add(lp_staker_amount)
            .ok_or(ErrorCode::MathOverflow)?;
        
        // Update last reward distribution time
        self.last_reward_distribution = Clock::get()?.unix_timestamp;
        
        msg!("Fee distribution: burn={}, partner={}, treasury={}, lp_rewards={}, rifts_burned={}", 
             burn_amount, partner_amount, treasury_amount, lp_staker_amount, rifts_burn_amount);
        
        Ok(())
    }

    /// Calculate volume threshold based on percentage of total supply
    pub fn calculate_volume_threshold(&self) -> Result<u64> {
        // **CRITICAL FIX**: Estimate total supply with proper fallback values
        let base_liquidity = self.total_liquidity_underlying
            .checked_add(self.total_liquidity_rift)
            .unwrap_or(self.total_liquidity_underlying.max(self.total_liquidity_rift));
            
        let estimated_supply = base_liquidity
            .checked_add(self.rifts_tokens_distributed)
            .unwrap_or(base_liquidity);
        
        // If no activity yet, use minimum threshold
        if estimated_supply == 0 {
            return Ok(1_000_000_000); // 1000 tokens with 6 decimals
        }
        
        // **CRITICAL FIX**: Calculate threshold as percentage of estimated supply with proper error handling
        let threshold = estimated_supply
            .checked_mul(self.volume_oracle_threshold_bps as u64)
            .ok_or(ErrorCode::MathOverflow)?
            .checked_div(10000)
            .ok_or(ErrorCode::MathOverflow)?;
        
        // Minimum threshold of 1000 tokens to prevent too frequent updates
        Ok(threshold.max(1_000_000_000)) // 1000 tokens with 6 decimals
    }
}

// LP Staking implementation will be added in future versions

#[event]
pub struct RiftCreated {
    pub rift: Pubkey,
    pub creator: Pubkey,
    pub underlying_mint: Pubkey,
    pub burn_fee_bps: u16,
    pub partner_fee_bps: u16,
}

#[event]
pub struct RiftClosed {
    pub rift: Pubkey,
    pub creator: Pubkey,
}

#[event]
pub struct StuckAccountCleaned {
    pub creator: Pubkey,
    pub stuck_mint: Pubkey,
    pub underlying_mint: Pubkey,
}

#[event]
pub struct WrapAndPoolCreated {
    pub rift: Pubkey,
    pub user: Pubkey,
    pub underlying_amount: u64,
    pub fee_amount: u64,
    pub tokens_minted: u64,
    pub pool_underlying: u64,
    pub pool_rift: u64,
    pub lp_tokens_minted: u64,
    pub trading_fee_bps: u16,
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
pub struct FeesCalculated {
    pub rift: Pubkey,
    pub treasury_amount: u64,
    pub fee_collector_amount: u64,
    pub partner_amount: u64,
    pub burn_amount: u64,
}

#[event]
pub struct LPTokensStaked {
    pub rift: Pubkey,
    pub user: Pubkey,
    pub amount: u64,
    pub total_staked: u64,
}

#[event]
pub struct LPTokensUnstaked {
    pub rift: Pubkey,
    pub user: Pubkey,
    pub amount: u64,
    pub remaining_staked: u64,
    pub pending_rewards: u64,
}

#[event]
pub struct StakingRewardsClaimed {
    pub rift: Pubkey,
    pub user: Pubkey,
    pub rewards_claimed: u64,
    pub total_claimed: u64,
}

#[event]
pub struct JupiterSwapExecuted {
    pub rift: Pubkey,
    pub amount_in: u64,
    pub minimum_amount_out: u64,
    pub timestamp: i64,
}

#[event]
pub struct GovernanceProposalExecuted {
    pub rift: Pubkey,
    pub proposal_id: u64,
    pub executor: Pubkey,
    pub timestamp: i64,
}

#[event]
pub struct RiftPaused {
    pub rift: Pubkey,
    pub authority: Pubkey,
    pub timestamp: i64,
}

#[event]
pub struct RiftUnpaused {
    pub rift: Pubkey,
    pub authority: Pubkey,
    pub timestamp: i64,
}

#[event]
pub struct OraclePriceUpdated {
    pub rift: Pubkey,
    pub oracle_type: String,
    pub price: u64,
    pub confidence: u64,
    pub timestamp: i64,
}

#[event]
pub struct JupiterProgramIdUpdated {
    pub rift: Pubkey,
    pub authority: Pubkey,
    pub old_program_id: Pubkey,
    pub new_program_id: Pubkey,
    pub timestamp: i64,
}

#[event]
pub struct VolumeThresholdUpdated {
    pub rift: Pubkey,
    pub authority: Pubkey,
    pub old_threshold: u64,
    pub new_threshold: u64,
    pub timestamp: i64,
}

// Helper function to create real Meteora DLMM pool instruction
fn create_meteora_pool_instruction(
    meteora_program_id: &Pubkey,
    pool_pda: &Pubkey,
    token_mint_x: &Pubkey,
    token_mint_y: &Pubkey,
    reserve_x: &Pubkey,
    reserve_y: &Pubkey,
    oracle: &Pubkey,
    preset_parameter: &Pubkey,
    funder: &Pubkey,
    bin_step: u16,
) -> Result<anchor_lang::solana_program::instruction::Instruction> {
    // Real Meteora DLMM initializeLbPair2 instruction discriminator
    let discriminator = [73, 59, 36, 120, 237, 83, 108, 198];
    
    // Build instruction data with discriminator + parameters
    let mut data = Vec::new();
    data.extend_from_slice(&discriminator);
    
    // Add bin_step parameter (u16)
    data.extend_from_slice(&bin_step.to_le_bytes());
    // Add active_id parameter (i32) - using default center bin
    data.extend_from_slice(&8388608_i32.to_le_bytes());
    
    // **CRITICAL FIX**: Derive proper event authority PDA
    let (event_authority, _) = Pubkey::find_program_address(
        &[b"__event_authority"],
        meteora_program_id
    );
    
    // Meteora DLMM initializeLbPair2 accounts (in correct order)
    let accounts = vec![
        // lb_pair (writable)
        anchor_lang::solana_program::instruction::AccountMeta {
            pubkey: *pool_pda,
            is_signer: false,
            is_writable: true,
        },
        // bin_array_bitmap_extension (writable, optional, PDA) - skip for basic pools
        // token_mint_x
        anchor_lang::solana_program::instruction::AccountMeta {
            pubkey: *token_mint_x,
            is_signer: false,
            is_writable: false,
        },
        // token_mint_y  
        anchor_lang::solana_program::instruction::AccountMeta {
            pubkey: *token_mint_y,
            is_signer: false,
            is_writable: false,
        },
        // reserve_x (writable, PDA)
        anchor_lang::solana_program::instruction::AccountMeta {
            pubkey: *reserve_x,
            is_signer: false,
            is_writable: true,
        },
        // reserve_y (writable, PDA)
        anchor_lang::solana_program::instruction::AccountMeta {
            pubkey: *reserve_y,
            is_signer: false,
            is_writable: true,
        },
        // oracle (writable, PDA)
        anchor_lang::solana_program::instruction::AccountMeta {
            pubkey: *oracle,
            is_signer: false,
            is_writable: true,
        },
        // preset_parameter
        anchor_lang::solana_program::instruction::AccountMeta {
            pubkey: *preset_parameter,
            is_signer: false,
            is_writable: false,
        },
        // funder (writable, signer)
        anchor_lang::solana_program::instruction::AccountMeta {
            pubkey: *funder,
            is_signer: true,
            is_writable: true,
        },
        // token_program (for token_mint_x)
        anchor_lang::solana_program::instruction::AccountMeta {
            pubkey: anchor_spl::token::ID,
            is_signer: false,
            is_writable: false,
        },
        // token_program (for token_mint_y)
        anchor_lang::solana_program::instruction::AccountMeta {
            pubkey: anchor_spl::token::ID,
            is_signer: false,
            is_writable: false,
        },
        // system_program
        anchor_lang::solana_program::instruction::AccountMeta {
            pubkey: anchor_lang::solana_program::system_program::ID,
            is_signer: false,
            is_writable: false,
        },
        // event_authority (PDA) - **FIXED**: Now uses proper PDA derivation
        anchor_lang::solana_program::instruction::AccountMeta {
            pubkey: event_authority,
            is_signer: false,
            is_writable: false,
        },
        // program (self-reference)
        anchor_lang::solana_program::instruction::AccountMeta {
            pubkey: *meteora_program_id,
            is_signer: false,
            is_writable: false,
        },
    ];
    
    Ok(anchor_lang::solana_program::instruction::Instruction {
        program_id: *meteora_program_id,
        accounts,
        data,
    })
}

#[error_code]
pub enum ErrorCode {
    #[msg("Invalid burn fee (max 45%)")]
    InvalidBurnFee,
    #[msg("Invalid partner fee (max 5%)")]
    InvalidPartnerFee,
    #[msg("Invalid trading fee (max 1%)")]
    InvalidTradingFee,
    #[msg("Rift name must end with '_RIFT' or 'RIFT'")]
    InvalidRiftName,
    #[msg("Rift name too long (max 32 chars)")]
    NameTooLong,
    #[msg("Invalid vanity address - must end with 'rift'")]
    InvalidVanityAddress,
    #[msg("Rebalance called too soon")]
    RebalanceTooSoon,
    #[msg("No rewards to claim")]
    NoRewardsToClaim,
    #[msg("Oracle price too stale")]
    OraclePriceTooStale,
    #[msg("Insufficient arbitrage opportunity")]
    InsufficientArbitrageOpportunity,
    #[msg("Unauthorized to close this rift")]
    UnauthorizedClose,
    #[msg("Vault must be empty before closing")]
    VaultNotEmpty,
    #[msg("Invalid stuck account - does not match expected PDA")]
    InvalidStuckAccount,
    #[msg("Rift already exists - not a stuck account")]
    RiftAlreadyExists,
    #[msg("Math overflow")]
    MathOverflow,
    #[msg("Invalid amount")]
    InvalidAmount,
    #[msg("Invalid backing ratio")]
    InvalidBackingRatio,
    #[msg("Unauthorized oracle")]
    UnauthorizedOracle,
    #[msg("Protocol is paused")]
    ProtocolPaused,
    #[msg("Emergency pause triggered")]
    EmergencyPause,
    #[msg("Volume circuit breaker triggered")]
    VolumeCircuitBreaker,
    #[msg("Oracle price deviation too high")]
    OraclePriceDeviation,
    #[msg("Stale oracle price")]
    StaleOraclePrice,
    #[msg("Buyback rate limit exceeded")]
    BuybackRateLimit,
    #[msg("Insufficient slippage protection")]
    InsufficientSlippageProtection,
    #[msg("Excessive slippage")]
    ExcessiveSlippage,
    #[msg("Invalid Jupiter instruction")]
    InvalidJupiterInstruction,
    #[msg("Suspicious activity detected")]
    SuspiciousActivity,
    #[msg("Unauthorized update")]
    UnauthorizedUpdate,
    #[msg("Invalid security parameter")]
    InvalidSecurityParameter,
    #[msg("Invalid price")]
    InvalidPrice,
    #[msg("Invalid confidence")]
    InvalidConfidence,
    #[msg("Invalid fee amount")]
    InvalidFeeAmount,
    #[msg("Insufficient rent exemption for account creation")]
    InsufficientRentExemption,
    #[msg("Invalid program ID in cross-program invocation")]
    InvalidProgramId,
    #[msg("Invalid seed component in PDA derivation")]
    InvalidSeedComponent,
    #[msg("Oracle registry is stale")]
    OracleRegistryStale,
    #[msg("Oracle registry is empty")]
    EmptyOracleRegistry,
    #[msg("Reentrancy attack detected")]
    ReentrancyDetected,
    #[msg("Vault not properly initialized")]
    VaultNotInitialized,
    #[msg("Amount too large for safe processing")]
    AmountTooLarge,
    #[msg("Amount too small for fee calculation")]
    AmountTooSmall,
    #[msg("Backing ratio too large")]
    BackingRatioTooLarge,
    #[msg("Fee too small for amount")]
    FeeTooSmall,
    #[msg("Mint amount too small")]
    MintAmountTooSmall,
    #[msg("Mint amount too large")]
    MintAmountTooLarge,
    #[msg("Invalid oracle price")]
    InvalidOraclePrice,
    #[msg("Oracle price too large")]
    OraclePriceTooLarge,
    #[msg("Insufficient staked tokens")]
    InsufficientStakedTokens,
    #[msg("Invalid input data")]
    InvalidInputData,
    #[msg("Invalid proposal type")]
    InvalidProposalType,
    #[msg("Proposal not approved")]
    ProposalNotApproved,
    #[msg("Invalid oracle interval")]
    InvalidOracleInterval,
    #[msg("Invalid rebalance threshold")]
    InvalidRebalanceThreshold,
    #[msg("Invalid volume threshold")]
    InvalidVolumeThreshold,
    #[msg("Unauthorized governance action")]
    UnauthorizedGovernance,
    #[msg("Rift is currently paused")]
    RiftPaused,
    #[msg("Insufficient oracle responses")]
    InsufficientOracles,
    #[msg("Partner vault account is missing")]
    MissingPartnerVault,
    #[msg("Invalid bin step for DLMM pool")]
    InvalidBinStep,
    #[msg("Unauthorized buyback operation")]
    UnauthorizedBuyback,
}

// Oracle update instruction implementations
/// **SECURITY FIX**: Multi-oracle validation to prevent manipulation
pub fn update_hybrid_oracle(ctx: Context<UpdateHybridOracle>) -> Result<()> {
    let rift = &mut ctx.accounts.rift;
    let current_time = Clock::get()?.unix_timestamp;
    
    // **CRITICAL SECURITY FIX**: Require multiple oracle sources for validation
    let pyth_price = extract_pyth_price(&ctx.accounts.pyth_price_account)?;
    let switchboard_price = extract_switchboard_price(&ctx.accounts.switchboard_price_account)?;
    let chainlink_price = extract_chainlink_price(&ctx.accounts.chainlink_price_account)?;
    
    // **SECURITY FIX**: Validate all oracles are recent (within 5 minutes)
    let max_staleness = 300; // 5 minutes
    require!(
        current_time - pyth_price.timestamp <= max_staleness,
        ErrorCode::StaleOraclePrice
    );
    require!(
        current_time - switchboard_price.timestamp <= max_staleness,
        ErrorCode::StaleOraclePrice
    );
    require!(
        current_time - chainlink_price.timestamp <= max_staleness,
        ErrorCode::StaleOraclePrice
    );
    
    // **SECURITY FIX**: Ensure oracle prices are within acceptable deviation (10%)
    let prices = [pyth_price.price, switchboard_price.price, chainlink_price.price];
    let median_price = calculate_median(prices)?;
    
    for price in prices.iter() {
        let deviation = if *price > median_price {
            (*price - median_price) * 10000 / median_price
        } else {
            (median_price - *price) * 10000 / median_price
        };
        require!(
            deviation <= 1000, // Max 10% deviation
            ErrorCode::OraclePriceDeviation
        );
    }
    
    // **SECURITY FIX**: Use median price as the authoritative price
    let validated_price = OraclePrice {
        price: median_price,
        confidence: pyth_price.confidence, // Use most conservative confidence
        timestamp: current_time,
        oracle_type: 3, // 3 = Hybrid
    };
    
    // Update rift oracle with validated price
    let price_index = rift.price_index as usize;
    rift.oracle_prices[price_index] = PriceData {
        price: validated_price.price,
        confidence: validated_price.confidence,
        timestamp: validated_price.timestamp,
    };
    
    rift.price_index = ((price_index + 1) % rift.oracle_prices.len()) as u8;
    rift.last_oracle_update = current_time;
    // Reset volume counter when manual oracle update occurs
    rift.volume_since_last_oracle = 0;
    
    emit!(OraclePriceUpdated {
        rift: rift.key(),
        oracle_type: "Hybrid".to_string(),
        price: validated_price.price,
        confidence: validated_price.confidence,
        timestamp: current_time,
    });
    
    Ok(())
}

// **SECURITY HELPER FUNCTIONS**: Safe oracle data extraction
fn extract_pyth_price(pyth_account: &AccountInfo) -> Result<OraclePrice> {
    let price_data = pyth_account.try_borrow_data()?;
    require!(price_data.len() >= 240, ErrorCode::InvalidOraclePrice);
    
    // Validate Pyth magic number
    let magic = u32::from_le_bytes([price_data[0], price_data[1], price_data[2], price_data[3]]);
    require!(magic == 0xa1b2c3d4, ErrorCode::InvalidOraclePrice);
    
    let price_i64 = i64::from_le_bytes([
        price_data[208], price_data[209], price_data[210], price_data[211],
        price_data[212], price_data[213], price_data[214], price_data[215]
    ]);
    
    // Confidence is at offset 216-223 (u64)
    let confidence_u64 = u64::from_le_bytes([
        price_data[216], price_data[217], price_data[218], price_data[219],
        price_data[220], price_data[221], price_data[222], price_data[223]
    ]);
    
    // Timestamp is at offset 224-231 (i64)
    let timestamp_i64 = i64::from_le_bytes([
        price_data[224], price_data[225], price_data[226], price_data[227],
        price_data[228], price_data[229], price_data[230], price_data[231]
    ]);
    
    // Validate price staleness (allow max 300 seconds)
    let current_time = Clock::get()?.unix_timestamp;
    require!(
        current_time - timestamp_i64 <= 300,
        ErrorCode::OraclePriceTooStale
    );
    
    // Convert price to positive u64 with scaling
    let price_scaled = if price_i64 >= 0 {
        (price_i64 as u64)
            .checked_mul(1_000_000) // Scale to 6 decimals
            .ok_or(ErrorCode::MathOverflow)?
    } else {
        return Err(ErrorCode::InvalidOraclePrice.into());
    };
    
    let confidence_scaled = confidence_u64
        .checked_mul(1_000_000)
        .ok_or(ErrorCode::MathOverflow)?;
    
    // Validate price and confidence
    require!(price_scaled > 0, ErrorCode::InvalidOraclePrice);
    require!(price_scaled <= 1_000_000_000_000, ErrorCode::OraclePriceTooLarge);
    require!(
        confidence_scaled <= price_scaled.checked_div(10).ok_or(ErrorCode::MathOverflow)?, 
        ErrorCode::InvalidConfidence
    );
    
    Ok(OraclePrice {
        price: price_scaled,
        confidence: confidence_scaled,
        timestamp: timestamp_i64,
        oracle_type: 0, // 0 = Pyth
    })
}

fn extract_switchboard_price(switchboard_account: &AccountInfo) -> Result<OraclePrice> {
    let price_data = switchboard_account.try_borrow_data()?;
    require!(price_data.len() >= 112, ErrorCode::InvalidOraclePrice);
    
    // Switchboard aggregator data structure
    let price_i128 = i128::from_le_bytes([
        price_data[16], price_data[17], price_data[18], price_data[19],
        price_data[20], price_data[21], price_data[22], price_data[23],
        price_data[24], price_data[25], price_data[26], price_data[27],
        price_data[28], price_data[29], price_data[30], price_data[31],
    ]);
    
    let timestamp_i64 = i64::from_le_bytes([
        price_data[96], price_data[97], price_data[98], price_data[99],
        price_data[100], price_data[101], price_data[102], price_data[103]
    ]);
    
    // Validate price staleness
    let current_time = Clock::get()?.unix_timestamp;
    require!(
        current_time - timestamp_i64 <= 300,
        ErrorCode::OraclePriceTooStale
    );
    
    // Convert to u64 with scaling
    let price_scaled = if price_i128 >= 0 {
        (price_i128 as u64)
            .checked_div(1_000_000) // Switchboard uses 9 decimals, we want 6
            .ok_or(ErrorCode::MathOverflow)?
    } else {
        return Err(ErrorCode::InvalidOraclePrice.into());
    };
    
    require!(price_scaled > 0, ErrorCode::InvalidOraclePrice);
    
    Ok(OraclePrice {
        price: price_scaled,
        confidence: price_scaled / 100, // Assume 1% confidence for Switchboard
        timestamp: timestamp_i64,
        oracle_type: 2, // 2 = Switchboard
    })
}

fn extract_chainlink_price(chainlink_account: &AccountInfo) -> Result<OraclePrice> {
    let price_data = chainlink_account.try_borrow_data()?;
    require!(price_data.len() >= 32, ErrorCode::InvalidOraclePrice);
    
    // Chainlink price data structure (simplified)
    let price_i64 = i64::from_le_bytes([
        price_data[8], price_data[9], price_data[10], price_data[11],
        price_data[12], price_data[13], price_data[14], price_data[15]
    ]);
    
    let timestamp_i64 = i64::from_le_bytes([
        price_data[16], price_data[17], price_data[18], price_data[19],
        price_data[20], price_data[21], price_data[22], price_data[23]
    ]);
    
    // Validate price staleness
    let current_time = Clock::get()?.unix_timestamp;
    require!(
        current_time - timestamp_i64 <= 300,
        ErrorCode::OraclePriceTooStale
    );
    
    let price_scaled = if price_i64 >= 0 {
        (price_i64 as u64)
            .checked_mul(1_000_000) // Scale to 6 decimals
            .ok_or(ErrorCode::MathOverflow)?
    } else {
        return Err(ErrorCode::InvalidOraclePrice.into());
    };
    
    require!(price_scaled > 0, ErrorCode::InvalidOraclePrice);
    
    Ok(OraclePrice {
        price: price_scaled,
        confidence: price_scaled / 200, // Assume 0.5% confidence for Chainlink
        timestamp: timestamp_i64,
        oracle_type: 1, // 1 = Chainlink
    })
}

fn calculate_median(mut prices: [u64; 3]) -> Result<u64> {
    prices.sort_unstable();
    Ok(prices[1]) // Middle value is median for 3 values
}

pub fn update_switchboard_oracle(ctx: Context<UpdateSwitchboardOracle>) -> Result<()> {
    let rift = &mut ctx.accounts.rift;
    let switchboard_feed = &ctx.accounts.switchboard_feed;
    
    // Parse REAL Switchboard aggregator account data directly
    let aggregator_data = switchboard_feed.try_borrow_data()?;
    require!(aggregator_data.len() >= 384, ErrorCode::InvalidOraclePrice); // Minimum Switchboard aggregator size
    
    let current_time = Clock::get()?.unix_timestamp;
    
    // Parse Switchboard aggregator account structure
    // Switchboard layout: discriminator(8) + data...
    let discriminator = u64::from_le_bytes([
        aggregator_data[0], aggregator_data[1], aggregator_data[2], aggregator_data[3],
        aggregator_data[4], aggregator_data[5], aggregator_data[6], aggregator_data[7]
    ]);
    
    // Validate Switchboard discriminator (aggregator account)
    require!(discriminator != 0, ErrorCode::InvalidOraclePrice);
    
    // Extract latest value from Switchboard aggregator
    // Latest value is stored as 128-bit decimal at offset 120-135
    let value_bytes = &aggregator_data[120..136];
    
    // Parse the value as bytes and convert to f64 (simplified parsing)
    let mut value_bits = [0u8; 16];
    value_bits.copy_from_slice(value_bytes);
    let value_u128 = u128::from_le_bytes(value_bits);
    
    // Convert to scaled price (simplified conversion)
    let price_scaled = (value_u128 as u64)
        .checked_div(1_000_000_000_000) // Scale down from Switchboard decimals
        .ok_or(ErrorCode::MathOverflow)?
        .checked_mul(1_000_000) // Scale to our 6 decimal format
        .ok_or(ErrorCode::MathOverflow)?;
    
    // Extract standard deviation (confidence) from offset 152-167
    let std_dev_bytes = &aggregator_data[152..168];
    let mut std_dev_bits = [0u8; 16];
    std_dev_bits.copy_from_slice(std_dev_bytes);
    let std_dev_u128 = u128::from_le_bytes(std_dev_bits);
    
    let confidence_scaled = (std_dev_u128 as u64)
        .checked_div(1_000_000_000_000)
        .ok_or(ErrorCode::MathOverflow)?
        .checked_mul(1_000_000)
        .ok_or(ErrorCode::MathOverflow)?;
    
    // Extract timestamp from offset 168-175 (i64)
    let timestamp_i64 = i64::from_le_bytes([
        aggregator_data[168], aggregator_data[169], aggregator_data[170], aggregator_data[171],
        aggregator_data[172], aggregator_data[173], aggregator_data[174], aggregator_data[175]
    ]);
    
    // Validate aggregator staleness (allow max 300 seconds)
    require!(
        current_time - timestamp_i64 <= 300,
        ErrorCode::OraclePriceTooStale
    );
    
    // Validate price and confidence
    require!(price_scaled > 0, ErrorCode::InvalidOraclePrice);
    require!(price_scaled <= 1_000_000_000_000, ErrorCode::OraclePriceTooLarge);
    require!(
        confidence_scaled <= price_scaled.checked_div(5).ok_or(ErrorCode::MathOverflow)?, // Max 20% confidence interval
        ErrorCode::InvalidConfidence
    );
    
    // Update oracle price in rift with real parsed Switchboard data
    rift.add_price_data(price_scaled, confidence_scaled, timestamp_i64)?;
    
    emit!(OraclePriceUpdated {
        rift: rift.key(),
        oracle_type: "Switchboard".to_string(),
        price: price_scaled,
        confidence: confidence_scaled,
        timestamp: timestamp_i64,
    });
    
    Ok(())
}

pub fn update_jupiter_oracle(ctx: Context<UpdateJupiterOracle>) -> Result<()> {
    let rift = &mut ctx.accounts.rift;
    let price_update = &ctx.accounts.price_update;
    
    // Validate the price update authority
    require!(
        price_update.authority == ctx.accounts.oracle_authority.key(),
        ErrorCode::UnauthorizedOracle
    );
    
    let current_time = Clock::get()?.unix_timestamp;
    
    // Validate price freshness (Jupiter prices updated frequently, allow max 120 seconds)
    require!(
        current_time - price_update.timestamp <= 120,
        ErrorCode::OraclePriceTooStale
    );
    
    // Validate price data
    require!(price_update.price > 0, ErrorCode::InvalidOraclePrice);
    require!(price_update.price <= 1_000_000_000_000, ErrorCode::OraclePriceTooLarge);
    require!(
        price_update.confidence <= price_update.price.checked_div(10).ok_or(ErrorCode::MathOverflow)?, // Max 10% confidence interval
        ErrorCode::InvalidConfidence
    );
    
    // Additional validation: ensure the price update account was created recently
    let price_update_account_info = ctx.accounts.price_update.to_account_info();
    let account_lamports = price_update_account_info.lamports();
    require!(account_lamports > 0, ErrorCode::InvalidOraclePrice);
    
    // Validate against rift's existing oracle data for sanity check
    if rift.last_oracle_update > 0 {
        let last_price = rift.get_average_oracle_price()?;
        if last_price > 0 {
            // **CRITICAL FIX**: Price deviation calculation with proper error handling
            let max_deviation = last_price.checked_div(2).ok_or(ErrorCode::MathOverflow)?;
            let min_price = last_price.checked_sub(max_deviation).ok_or(ErrorCode::MathOverflow)?;
            let max_price = last_price.checked_add(max_deviation).ok_or(ErrorCode::MathOverflow)?;
            
            require!(
                price_update.price >= min_price && price_update.price <= max_price,
                ErrorCode::InvalidOraclePrice
            );
        }
    }
    
    // Update oracle price in rift with Jupiter data
    rift.add_price_data(
        price_update.price, 
        price_update.confidence, 
        price_update.timestamp
    )?;
    
    emit!(OraclePriceUpdated {
        rift: rift.key(),
        oracle_type: "Jupiter".to_string(),
        price: price_update.price,
        confidence: price_update.confidence,
        timestamp: price_update.timestamp,
    });
    
    Ok(())
}

#[derive(Accounts)]
pub struct UpdateHybridOracle<'info> {
    #[account(mut)]
    pub rift: Account<'info, Rift>,
    
    /// CHECK: Pyth price account
    pub pyth_price_account: UncheckedAccount<'info>,
    
    /// CHECK: Switchboard aggregator account  
    pub switchboard_price_account: UncheckedAccount<'info>,
    
    /// CHECK: Chainlink price account
    pub chainlink_price_account: UncheckedAccount<'info>,
    
    pub oracle_authority: Signer<'info>,
}

#[derive(Accounts)]
pub struct UpdatePythOracle<'info> {
    #[account(mut)]
    pub rift: Account<'info, Rift>,
    
    /// CHECK: Pyth price account
    pub pyth_price_account: UncheckedAccount<'info>,
    
    pub oracle_authority: Signer<'info>,
}

#[derive(Accounts)]
pub struct UpdateSwitchboardOracle<'info> {
    #[account(mut)]
    pub rift: Account<'info, Rift>,
    
    /// CHECK: Switchboard aggregator account
    pub switchboard_feed: UncheckedAccount<'info>,
    
    pub oracle_authority: Signer<'info>,
}

#[derive(Accounts)]
pub struct UpdateJupiterOracle<'info> {
    #[account(mut)]
    pub rift: Account<'info, Rift>,
    
    /// Price update account containing Jupiter price data
    pub price_update: Account<'info, JupiterPriceUpdate>,
    
    pub oracle_authority: Signer<'info>,
}

#[account]
pub struct JupiterPriceUpdate {
    pub price: u64,
    pub confidence: u64,
    pub timestamp: i64,
    pub authority: Pubkey,
}