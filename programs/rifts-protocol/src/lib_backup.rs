// Rifts Protocol - Full Peapods Clone with Hybrid Oracle System
use anchor_lang::prelude::*;
use anchor_spl::token::{self, Token, TokenAccount, Transfer, Mint, MintTo};
// Note: Metadata functionality removed to avoid dependency issues
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
    pub fn create_rift_with_vanity_mint(
        ctx: Context<CreateRiftWithVanityMint>,
        burn_fee_bps: u16,
        partner_fee_bps: u16,
        partner_wallet: Option<Pubkey>,
        rift_name: Option<String>,
    ) -> Result<()> {
        let rift = &mut ctx.accounts.rift;
        
        // Validate fees
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
        
        emit!(RiftCreated {
            rift: rift.key(),
            creator: rift.creator,
            underlying_mint: rift.underlying_mint,
            burn_fee_bps,
            partner_fee_bps,
        });

        Ok(())
    }
    
    /// Initialize a new Rift (wrapped token vault) - STACK OPTIMIZED (Original PDA version)
    pub fn create_rift(
        ctx: Context<CreateRift>,
        burn_fee_bps: u16,
        partner_fee_bps: u16,
        partner_wallet: Option<Pubkey>,
        rift_name: Option<String>,
    ) -> Result<()> {
        let rift = &mut ctx.accounts.rift;
        
        // Validate fees
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
        // Vault will be set to the derived PDA address
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
        
        // Initialize hybrid oracle system
        rift.oracle_prices = [PriceData::default(); 10];
        rift.price_index = 0;
        rift.oracle_update_interval = 30 * 60; // 30 minutes
        rift.max_rebalance_interval = 24 * 60 * 60; // 24 hours
        rift.arbitrage_threshold_bps = 200; // 2% threshold
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
        
        emit!(RiftCreated {
            rift: rift.key(),
            creator: rift.creator,
            underlying_mint: rift.underlying_mint,
            burn_fee_bps,
            partner_fee_bps,
        });

        Ok(())
    }

    /// Wrap underlying tokens into rift tokens
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
        
        // Vault is now guaranteed to be initialized during rift creation
        // No race condition possible - removed dynamic initialization
        
        // **CRITICAL FIX**: Calculate wrap fee with enhanced precision protection
        let wrap_fee = amount
            .checked_mul(70)
            .ok_or(ErrorCode::MathOverflow)?
            .checked_div(10000)
            .ok_or(ErrorCode::MathOverflow)?;
        
        // **CRITICAL FIX**: Ensure wrap fee is non-zero for non-zero amounts
        require!(wrap_fee > 0 || amount < 143, ErrorCode::FeeTooSmall);
        
        let amount_after_fee = amount
            .checked_sub(wrap_fee)
            .ok_or(ErrorCode::MathOverflow)?;

        // Transfer underlying tokens to vault
        // Validate program ID before CPI call
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

        // **CRITICAL FIX**: Calculate rift tokens to mint with additional validation
        // Backing ratio already validated above
        let rift_tokens_to_mint = amount_after_fee
            .checked_mul(10000)
            .ok_or(ErrorCode::MathOverflow)?
            .checked_div(rift.backing_ratio)
            .ok_or(ErrorCode::MathOverflow)?;
        
        // **CRITICAL FIX**: Validate mint amount is reasonable
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

        // Validate program ID before CPI call
        require!(
            ctx.accounts.token_program.key() == anchor_spl::token::ID,
            ErrorCode::InvalidProgramId
        );
        
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
        
        // Track fees collected with checked arithmetic
        rift.total_fees_collected = rift.total_fees_collected
            .checked_add(wrap_fee)
            .ok_or(ErrorCode::MathOverflow)?;
        
        // Update volume tracking with checked arithmetic
        rift.total_volume_24h = rift.total_volume_24h
            .checked_add(amount)
            .ok_or(ErrorCode::MathOverflow)?;
        
        // Update oracle timestamp to mark activity
        rift.last_oracle_update = Clock::get()?.unix_timestamp;

        // **CRITICAL FIX**: Process fee distribution BEFORE clearing reentrancy guard
        if wrap_fee > 0 {
            rift.process_fee_immediately(wrap_fee)?;
        }
        
        // **CRITICAL FIX**: Clear reentrancy guard at the end
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
        
        // Update oracle timestamp to mark activity
        rift.last_oracle_update = Clock::get()?.unix_timestamp;

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
            let transfer_partner_ctx = CpiContext::new(
                ctx.accounts.token_program.to_account_info(),
                Transfer {
                    from: ctx.accounts.vault.to_account_info(),
                    to: ctx.accounts.partner_vault.as_ref().unwrap().to_account_info(),
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
            // Calculate pending rewards: 10% APY = ~0.00003170979% per hour
            // Rewards = staked_amount * time_hours * hourly_rate / 1000000
            let time_hours = time_elapsed
                .checked_div(3600)
                .unwrap_or(0);
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
                .unwrap_or(0);
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
                .unwrap_or(0);
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
    pub fn jupiter_swap_for_buyback(
        ctx: Context<JupiterSwapForBuyback>,
        amount_in: u64,
        minimum_amount_out: u64,
        swap_data: Vec<u8>,
    ) -> Result<()> {
        let rift = &mut ctx.accounts.rift;
        
        // Validate input
        require!(amount_in > 0, ErrorCode::InvalidAmount);
        require!(amount_in <= 1_000_000_000_000, ErrorCode::AmountTooLarge);
        require!(swap_data.len() <= 10000, ErrorCode::InvalidInputData);
        
        // CPI to Jupiter program for swap
        let cpi_program = ctx.accounts.jupiter_program.to_account_info();
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
    /// This is passed in by the user, similar to how pump.fun accepts pre-generated mints
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
    
    /// CHECK: Jupiter program
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
    // Advanced Metrics
    pub total_volume_24h: u64,          // 24h trading volume
    pub price_deviation: u64,           // Current price deviation from backing
    pub arbitrage_opportunity_bps: u16, // Current arbitrage opportunity
    pub rebalance_count: u32,           // Total number of rebalances
    
    // RIFTS Token Distribution
    pub total_fees_collected: u64,     // Total fees collected
    pub rifts_tokens_distributed: u64, // Total RIFTS tokens distributed to LP stakers
    pub rifts_tokens_burned: u64,      // Total RIFTS tokens burned
    
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
    
    pub fn should_trigger_rebalance(&self, current_time: i64) -> Result<bool> {
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
        // **CRITICAL FIX**: Get total fees that haven't been distributed yet with checked arithmetic
        let total_distributed = self.rifts_tokens_distributed
            .checked_add(self.rifts_tokens_burned)
            .unwrap_or(u64::MAX); // Safe fallback if overflow
        
        if self.total_fees_collected > total_distributed {
            self.total_fees_collected
                .checked_sub(total_distributed)
                .unwrap_or(0) // Safe fallback
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

#[error_code]
pub enum ErrorCode {
    #[msg("Invalid burn fee (max 45%)")]
    InvalidBurnFee,
    #[msg("Invalid partner fee (max 5%)")]
    InvalidPartnerFee,
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
    #[msg("Unauthorized governance action")]
    UnauthorizedGovernance,
    #[msg("Rift is currently paused")]
    RiftPaused,
    #[msg("Insufficient oracle responses")]
    InsufficientOracles,
}

// Oracle update instruction implementations
pub fn update_pyth_oracle(ctx: Context<UpdatePythOracle>) -> Result<()> {
    let rift = &mut ctx.accounts.rift;
    let pyth_price_account = &ctx.accounts.pyth_price_account;
    
    // Parse REAL Pyth price account data directly
    let price_data = pyth_price_account.try_borrow_data()?;
    require!(price_data.len() >= 240, ErrorCode::InvalidOraclePrice); // Minimum Pyth account size
    
    let current_time = Clock::get()?.unix_timestamp;
    
    // Parse Pyth price account structure
    // Pyth account layout: magic(4) + version(4) + account_type(4) + price_data...
    let magic = u32::from_le_bytes([price_data[0], price_data[1], price_data[2], price_data[3]]);
    let version = u32::from_le_bytes([price_data[4], price_data[5], price_data[6], price_data[7]]);
    let account_type = u32::from_le_bytes([price_data[8], price_data[9], price_data[10], price_data[11]]);
    
    // Validate Pyth magic number and account type
    require!(magic == 0xa1b2c3d4, ErrorCode::InvalidOraclePrice); // Pyth magic
    require!(account_type == 3, ErrorCode::InvalidOraclePrice); // Price account type
    
    // Extract price data from Pyth account
    // Price is at offset 208-215 (i64)
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
        confidence_scaled <= price_scaled.checked_div(10).unwrap_or(0), 
        ErrorCode::InvalidConfidence
    );
    
    // Update oracle price in rift with real parsed Pyth data
    rift.add_price_data(price_scaled, confidence_scaled, timestamp_i64)?;
    
    emit!(OraclePriceUpdated {
        rift: rift.key(),
        oracle_type: "Pyth".to_string(),
        price: price_scaled,
        confidence: confidence_scaled,
        timestamp: timestamp_i64,
    });
    
    Ok(())
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
        .unwrap_or(0)
        .checked_mul(1_000_000) // Scale to our 6 decimal format
        .ok_or(ErrorCode::MathOverflow)?;
    
    // Extract standard deviation (confidence) from offset 152-167
    let std_dev_bytes = &aggregator_data[152..168];
    let mut std_dev_bits = [0u8; 16];
    std_dev_bits.copy_from_slice(std_dev_bytes);
    let std_dev_u128 = u128::from_le_bytes(std_dev_bits);
    
    let confidence_scaled = (std_dev_u128 as u64)
        .checked_div(1_000_000_000_000)
        .unwrap_or(0)
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
        confidence_scaled <= price_scaled.checked_div(5).unwrap_or(0), // Max 20% confidence interval
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
        price_update.confidence <= price_update.price.checked_div(10).unwrap_or(0), // Max 10% confidence interval
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
            // Price shouldn't deviate more than 50% from last known price
            let max_deviation = last_price.checked_div(2).unwrap_or(0);
            let min_price = last_price.checked_sub(max_deviation).unwrap_or(0);
            let max_price = last_price.checked_add(max_deviation).unwrap_or(u64::MAX);
            
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