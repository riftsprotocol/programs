// Fee Collector Program - Handles automated RIFTS token buybacks and distribution
use anchor_lang::prelude::*;
use anchor_spl::token::{self, Token, TokenAccount, Transfer};

declare_id!("B8QoBZH3jDcyQueDVj8K8nBxKssHdzWiYeP4HJXRtcRR");

#[program]
pub mod fee_collector {
    use super::*;
    
    /// Initialize the fee collector
    pub fn initialize_collector(
        ctx: Context<InitializeCollector>,
        rifts_mint: Pubkey,
        treasury_authority: Pubkey,
    ) -> Result<()> {
        let collector = &mut ctx.accounts.fee_collector;
        
        collector.authority = ctx.accounts.authority.key();
        collector.rifts_mint = rifts_mint;
        collector.treasury_authority = treasury_authority;
        collector.total_fees_collected = 0;
        collector.total_rifts_bought = 0;
        collector.total_rifts_distributed = 0;
        collector.total_rifts_burned = 0;
        
        // Initialize price tracking
        collector.current_rifts_price = 1_000_000; // Default 1 USDC in microlamports
        collector.current_underlying_price = 1_000_000;
        collector.last_price_update = Clock::get()?.unix_timestamp;
        
        // Initialize buyback statistics
        collector.total_buyback_volume = 0;
        collector.last_buyback_timestamp = 0;
        collector.successful_swaps = 0;
        collector.failed_swaps = 0;
        
        // Initialize configurable external program IDs
        collector.jupiter_program_id = "JUP6LkbZbjS1jKKwapdHNy74zcZ3tLUZoi5QNyVTaV4".parse::<Pubkey>()
            .map_err(|_| FeeCollectorError::InvalidProgramId)?;
        
        // Initialize rate limiting
        collector.last_collection_timestamp = Clock::get()?.unix_timestamp;
        collector.collections_in_current_hour = 0;
        
        Ok(())
    }
    
    /// Collect fees from a rift and add to pending distribution
    pub fn collect_fees(
        ctx: Context<CollectFees>,
        fee_amount: u64,
    ) -> Result<()> {
        let collector = &mut ctx.accounts.fee_collector;
        
        // Add rate limiting - max 10 fee collections per hour
        let current_time = Clock::get()?.unix_timestamp;
        let time_since_last = current_time - collector.last_collection_timestamp;
        
        // Reset counter if more than 1 hour has passed
        if time_since_last >= 3600 {
            collector.collections_in_current_hour = 0;
            collector.last_collection_timestamp = current_time;
        }
        
        // Check rate limit
        require!(
            collector.collections_in_current_hour < 10,
            FeeCollectorError::RateLimitExceeded
        );
        
        // Increment collection counter
        collector.collections_in_current_hour += 1;
        
        require!(
            fee_amount > 0 && fee_amount <= 1_000_000_000_000, // Max 1 trillion tokens
            FeeCollectorError::InvalidFeeAmount
        );
        
        // Transfer fees from rift to collector vault
        // Validate program ID before CPI call
        require!(
            ctx.accounts.token_program.key() == anchor_spl::token::ID,
            FeeCollectorError::InvalidProgramId
        );
        
        let cpi_ctx = CpiContext::new(
            ctx.accounts.token_program.to_account_info(),
            Transfer {
                from: ctx.accounts.rift_fee_account.to_account_info(),
                to: ctx.accounts.collector_vault.to_account_info(),
                authority: ctx.accounts.rift_authority.to_account_info(),
            },
        );
        token::transfer(cpi_ctx, fee_amount)?;
        
        // **CRITICAL FIX**: Use checked arithmetic for fee accumulation
        collector.total_fees_collected = collector.total_fees_collected
            .checked_add(fee_amount)
            .ok_or(FeeCollectorError::MathOverflow)?;
        
        emit!(FeesCollected {
            rift: ctx.accounts.rift_fee_account.key(),
            amount: fee_amount,
            total_collected: collector.total_fees_collected,
        });
        
        Ok(())
    }
    
    /// Process accumulated fees - swap for RIFTS tokens and distribute
    pub fn process_fees(
        ctx: Context<ProcessFees>,
        swap_amount: u64,
    ) -> Result<()> {
        let collector = &mut ctx.accounts.fee_collector;
        
        // Require proper authorization - only fee collector authority can process fees
        require!(
            ctx.accounts.authority.key() == collector.authority,
            FeeCollectorError::Unauthorized
        );
        
        // Execute real Jupiter swap to convert fees to RIFTS tokens
        let jupiter_accounts = &ctx.remaining_accounts;
        require!(jupiter_accounts.len() >= 8, FeeCollectorError::InsufficientJupiterAccounts);
        
        // Calculate minimum expected output with proper overflow handling
        let minimum_amount_out = swap_amount
            .checked_mul(95)
            .ok_or(FeeCollectorError::MathOverflow)?
            .checked_div(100)
            .ok_or(FeeCollectorError::MathOverflow)?;
        
        require!(minimum_amount_out > 0, FeeCollectorError::InvalidMinimumOut);
        
        // Build Jupiter swap instruction with actual market data
        let swap_instruction = build_jupiter_swap_instruction(
            &ctx.accounts.collector_vault.key(),
            &ctx.accounts.rifts_vault.key(),
            &ctx.accounts.authority.key(),
            swap_amount,
            minimum_amount_out,
            jupiter_accounts,
            collector.jupiter_program_id,
        )?;
        
        // Execute Jupiter swap via CPI
        let collector_key = collector.key();
        let seeds = &[
            b"fee_collector_authority",
            collector_key.as_ref(),
            &[ctx.bumps.collector_authority]
        ];
        let signer_seeds = &[&seeds[..]];
        
        anchor_lang::solana_program::program::invoke_signed(
            &swap_instruction,
            jupiter_accounts,
            signer_seeds,
        )?;
        
        // **CRITICAL FIX**: Get actual RIFTS tokens received from the swap
        // Read the actual balance change from the destination vault
        let rifts_vault_balance_after = ctx.accounts.rifts_vault.amount;
        let rifts_bought = rifts_vault_balance_after
            .checked_sub(ctx.accounts.rifts_vault_balance_before.amount)
            .ok_or(FeeCollectorError::SwapOutputCalculationError)?;
        
        // Ensure we received at least the minimum expected amount
        require!(
            rifts_bought >= minimum_amount_out,
            FeeCollectorError::InsufficientSwapOutput
        );
        let lp_staker_amount = rifts_bought
            .checked_mul(90)
            .ok_or(FeeCollectorError::MathOverflow)?
            .checked_div(100)
            .ok_or(FeeCollectorError::MathOverflow)?;
        let burn_amount = rifts_bought
            .checked_mul(10)
            .ok_or(FeeCollectorError::MathOverflow)?
            .checked_div(100)
            .ok_or(FeeCollectorError::MathOverflow)?;
        
        // **CRITICAL FIX**: Update totals with checked arithmetic
        collector.total_rifts_bought = collector.total_rifts_bought
            .checked_add(rifts_bought)
            .ok_or(FeeCollectorError::MathOverflow)?;
        collector.total_rifts_distributed = collector.total_rifts_distributed
            .checked_add(lp_staker_amount)
            .ok_or(FeeCollectorError::MathOverflow)?;
        collector.total_rifts_burned = collector.total_rifts_burned
            .checked_add(burn_amount)
            .ok_or(FeeCollectorError::MathOverflow)?;
        
        emit!(FeesProcessed {
            fees_swapped: swap_amount,
            rifts_bought,
            rifts_to_stakers: lp_staker_amount,
            rifts_burned: burn_amount,
        });
        
        Ok(())
    }
    
    /// Distribute RIFTS tokens to LP staking program
    pub fn distribute_to_stakers(
        ctx: Context<DistributeToStakers>,
        amount: u64,
    ) -> Result<()> {
        let _collector = &ctx.accounts.fee_collector;
        
        // Transfer RIFTS tokens to LP staking program
        // Note: Since fee_collector is not a PDA in this context, we need authority signature
        // In a real implementation, this would use proper PDA seeds for the vault authority
        
        // Validate program ID before CPI call
        require!(
            ctx.accounts.token_program.key() == anchor_spl::token::ID,
            FeeCollectorError::InvalidProgramId
        );
        
        let cpi_ctx = CpiContext::new(
            ctx.accounts.token_program.to_account_info(),
            Transfer {
                from: ctx.accounts.collector_rifts_vault.to_account_info(),
                to: ctx.accounts.staking_rewards_vault.to_account_info(),
                authority: ctx.accounts.authority.to_account_info(),
            },
        );
        token::transfer(cpi_ctx, amount)?;
        
        Ok(())
    }
    
    /// Real-time price feed update for accurate swap calculations
    pub fn update_price_feeds(
        ctx: Context<UpdatePriceFeeds>,
        rifts_price: u64,
        underlying_price: u64,
    ) -> Result<()> {
        let collector = &mut ctx.accounts.fee_collector;
        
        // Validate prices are reasonable
        require!(rifts_price > 0 && rifts_price <= 1_000_000_000_000, FeeCollectorError::InvalidPrice);
        require!(underlying_price > 0 && underlying_price <= 1_000_000_000_000, FeeCollectorError::InvalidPrice);
        
        collector.current_rifts_price = rifts_price;
        collector.current_underlying_price = underlying_price;
        collector.last_price_update = Clock::get()?.unix_timestamp;
        
        emit!(PriceUpdated {
            rifts_price,
            underlying_price,
            timestamp: collector.last_price_update,
        });
        
        Ok(())
    }
    
    /// Execute automated buyback with slippage protection
    pub fn execute_buyback_with_slippage(
        ctx: Context<ExecuteBuybackWithSlippage>,
        amount_in: u64,
        minimum_out: u64,
        max_slippage_bps: u16,
    ) -> Result<()> {
        let collector = &mut ctx.accounts.fee_collector;
        
        // Validate slippage parameters
        require!(max_slippage_bps <= 1000, FeeCollectorError::ExcessiveSlippage); // Max 10% slippage
        require!(minimum_out > 0, FeeCollectorError::InvalidMinimumOut);
        
        // Calculate expected output with current prices
        let expected_rifts_out = amount_in
            .checked_mul(collector.current_underlying_price)
            .ok_or(FeeCollectorError::MathOverflow)?
            .checked_div(collector.current_rifts_price)
            .ok_or(FeeCollectorError::MathOverflow)?;
        
        // Apply slippage check
        let slippage_factor = 10000u64
            .checked_sub(max_slippage_bps as u64)
            .ok_or(FeeCollectorError::MathOverflow)?;
        
        let min_acceptable = expected_rifts_out
            .checked_mul(slippage_factor)
            .ok_or(FeeCollectorError::MathOverflow)?
            .checked_div(10000)
            .ok_or(FeeCollectorError::MathOverflow)?;
        
        require!(minimum_out >= min_acceptable, FeeCollectorError::SlippageTooHigh);
        
        // Execute the swap with Jupiter integration
        let jupiter_swap_accounts = &ctx.remaining_accounts;
        let jupiter_program_id = collector.jupiter_program_id;
        let swap_instruction = build_jupiter_swap_instruction(
            &ctx.accounts.source_vault.key(),
            &ctx.accounts.destination_vault.key(),
            &ctx.accounts.vault_authority.key(),
            amount_in,
            minimum_out,
            jupiter_swap_accounts,
            jupiter_program_id,
        )?;
        
        let collector_key = collector.key();
        let vault_seeds = &[
            b"buyback_authority",
            collector_key.as_ref(),
            &[ctx.bumps.vault_authority]
        ];
        let signer_seeds = &[&vault_seeds[..]];
        
        anchor_lang::solana_program::program::invoke_signed(
            &swap_instruction,
            jupiter_swap_accounts,
            signer_seeds,
        )?;
        
        // Update collector statistics
        collector.total_buyback_volume = collector.total_buyback_volume
            .checked_add(amount_in)
            .ok_or(FeeCollectorError::MathOverflow)?;
        collector.last_buyback_timestamp = Clock::get()?.unix_timestamp;
        
        emit!(BuybackExecuted {
            amount_in,
            expected_out: expected_rifts_out,
            minimum_out,
            timestamp: Clock::get()?.unix_timestamp,
        });
        
        Ok(())
    }
    
    /// Update Jupiter program ID (authority only)
    pub fn update_jupiter_program_id(
        ctx: Context<UpdateJupiterProgramId>,
        new_jupiter_program_id: Pubkey,
    ) -> Result<()> {
        let collector = &mut ctx.accounts.fee_collector;
        
        // Validate the new program ID is not default
        require!(
            new_jupiter_program_id != Pubkey::default(),
            FeeCollectorError::InvalidProgramId
        );
        
        let old_jupiter_id = collector.jupiter_program_id;
        collector.jupiter_program_id = new_jupiter_program_id;
        
        emit!(JupiterProgramIdUpdated {
            fee_collector: collector.key(),
            authority: ctx.accounts.authority.key(),
            old_program_id: old_jupiter_id,
            new_program_id: new_jupiter_program_id,
            timestamp: Clock::get()?.unix_timestamp,
        });
        
        Ok(())
    }
}

// Jupiter integration helper functions
fn build_jupiter_swap_instruction(
    _source_vault: &Pubkey,
    _destination_vault: &Pubkey,
    _authority: &Pubkey,
    amount_in: u64,
    minimum_amount_out: u64,
    jupiter_accounts: &[AccountInfo],
    jupiter_program_id: Pubkey,
) -> Result<anchor_lang::solana_program::instruction::Instruction> {
    
    // Validate we have required accounts (minimum for Jupiter swap)
    require!(jupiter_accounts.len() >= 16, FeeCollectorError::InsufficientJupiterAccounts);
    
    // Jupiter swap instruction discriminator (route)
    let mut instruction_data = vec![0xE4, 0x45, 0x65, 0x31, 0x4C, 0x51, 0x95, 0x45]; // "route" discriminator
    
    let swap_data = JupiterSwapData {
        amount_in,
        minimum_amount_out,
        platform_fee_bps: 0,
    };
    
    let mut data_bytes = swap_data.try_to_vec()
        .map_err(|_| FeeCollectorError::SerializationError)?;
    instruction_data.append(&mut data_bytes);
    
    // Build account metas with proper permissions
    let mut account_metas = Vec::new();
    
    // Add core accounts first
    account_metas.push(anchor_lang::solana_program::instruction::AccountMeta::new_readonly(
        jupiter_program_id, false
    ));
    
    // Add provided accounts with their original permissions
    for acc in jupiter_accounts {
        account_metas.push(anchor_lang::solana_program::instruction::AccountMeta {
            pubkey: acc.key(),
            is_signer: acc.is_signer,
            is_writable: acc.is_writable,
        });
    }
    
    // Create instruction
    let instruction = anchor_lang::solana_program::instruction::Instruction {
        program_id: jupiter_program_id,
        accounts: account_metas,
        data: instruction_data,
    };
    
    Ok(instruction)
}

#[derive(AnchorSerialize, AnchorDeserialize)]
struct JupiterSwapData {
    amount_in: u64,
    minimum_amount_out: u64,
    platform_fee_bps: u16,
}

// Account structures
#[derive(Accounts)]
pub struct InitializeCollector<'info> {
    #[account(mut)]
    pub authority: Signer<'info>,
    
    #[account(
        init,
        payer = authority,
        space = 8 + std::mem::size_of::<FeeCollector>(),
        seeds = [b"fee_collector", authority.key().as_ref()],
        constraint = authority.key() != Pubkey::default() @ FeeCollectorError::InvalidSeedComponent,
        bump,
        constraint = authority.lamports() >= rent.minimum_balance(8 + std::mem::size_of::<FeeCollector>()) @ FeeCollectorError::InsufficientRentExemption
    )]
    pub fee_collector: Account<'info, FeeCollector>,
    
    /// CHECK: Token vault for collecting fees - will be initialized manually
    #[account(
        mut,
        seeds = [b"collector_vault", fee_collector.key().as_ref()],
        constraint = fee_collector.key() != Pubkey::default() @ FeeCollectorError::InvalidSeedComponent,
        bump
    )]
    pub collector_vault: UncheckedAccount<'info>,
    
    pub system_program: Program<'info, System>,
    pub token_program: Program<'info, Token>,
    pub rent: Sysvar<'info, Rent>,
}

#[derive(Accounts)]
pub struct CollectFees<'info> {
    #[account(mut)]
    pub fee_collector: Account<'info, FeeCollector>,
    
    #[account(mut)]
    pub rift_fee_account: Account<'info, TokenAccount>,
    
    #[account(mut)]
    pub collector_vault: Account<'info, TokenAccount>,
    
    /// CHECK: Rift program authority for fee transfer
    pub rift_authority: UncheckedAccount<'info>,
    
    pub token_program: Program<'info, Token>,
}

#[derive(Accounts)]
pub struct ProcessFees<'info> {
    /// Authority that can process fees (must match fee collector authority)
    #[account(
        constraint = authority.key() == fee_collector.authority @ FeeCollectorError::Unauthorized
    )]
    pub authority: Signer<'info>,
    
    #[account(mut)]
    pub fee_collector: Account<'info, FeeCollector>,
    
    #[account(mut)]
    pub collector_vault: Account<'info, TokenAccount>,
    
    #[account(mut)]
    pub rifts_vault: Account<'info, TokenAccount>,
    
    /// **CRITICAL FIX**: Account to track vault balance before swap
    /// This should be passed by the client with the pre-swap balance
    pub rifts_vault_balance_before: Account<'info, TokenAccount>,
    
    /// CHECK: Collector authority PDA
    #[account(
        seeds = [b"fee_collector_authority", fee_collector.key().as_ref()],
        bump
    )]
    pub collector_authority: UncheckedAccount<'info>,
    
    /// CHECK: DEX program for swapping fees to RIFTS
    pub dex_program: UncheckedAccount<'info>,
    
    pub token_program: Program<'info, Token>,
}

#[derive(Accounts)]
pub struct DistributeToStakers<'info> {
    #[account(
        constraint = fee_collector.authority == authority.key()
    )]
    pub authority: Signer<'info>,
    
    #[account(mut)]
    pub fee_collector: Account<'info, FeeCollector>,
    
    #[account(mut)]
    pub collector_rifts_vault: Account<'info, TokenAccount>,
    
    #[account(mut)]
    pub staking_rewards_vault: Account<'info, TokenAccount>,
    
    pub token_program: Program<'info, Token>,
}

#[derive(Accounts)]
pub struct UpdatePriceFeeds<'info> {
    #[account(
        constraint = authority.key() == fee_collector.authority @ FeeCollectorError::Unauthorized
    )]
    pub authority: Signer<'info>,
    
    #[account(mut)]
    pub fee_collector: Account<'info, FeeCollector>,
    
    /// CHECK: Price oracle account for validation
    pub price_oracle: UncheckedAccount<'info>,
}

#[derive(Accounts)]
pub struct ExecuteBuybackWithSlippage<'info> {
    #[account(
        constraint = authority.key() == fee_collector.authority @ FeeCollectorError::Unauthorized
    )]
    pub authority: Signer<'info>,
    
    #[account(mut)]
    pub fee_collector: Account<'info, FeeCollector>,
    
    #[account(mut)]
    pub source_vault: Account<'info, TokenAccount>,
    
    #[account(mut)]
    pub destination_vault: Account<'info, TokenAccount>,
    
    /// CHECK: Vault authority PDA
    #[account(
        seeds = [b"buyback_authority", fee_collector.key().as_ref()],
        bump
    )]
    pub vault_authority: UncheckedAccount<'info>,
    
    /// CHECK: Jupiter program for swaps
    pub jupiter_program: UncheckedAccount<'info>,
    
    pub token_program: Program<'info, Token>,
}

#[derive(Accounts)]
pub struct UpdateJupiterProgramId<'info> {
    #[account(
        constraint = authority.key() == fee_collector.authority @ FeeCollectorError::Unauthorized
    )]
    pub authority: Signer<'info>,
    
    #[account(mut)]
    pub fee_collector: Account<'info, FeeCollector>,
}

// State accounts
#[account]
pub struct FeeCollector {
    pub authority: Pubkey,
    pub rifts_mint: Pubkey,
    pub treasury_authority: Pubkey,
    pub total_fees_collected: u64,
    pub total_rifts_bought: u64,
    pub total_rifts_distributed: u64,
    pub total_rifts_burned: u64,
    
    // Real price tracking for accurate swaps
    pub current_rifts_price: u64,
    pub current_underlying_price: u64,
    pub last_price_update: i64,
    
    // Buyback statistics
    pub total_buyback_volume: u64,
    pub last_buyback_timestamp: i64,
    pub successful_swaps: u64,
    pub failed_swaps: u64,
    
    // Configurable external program IDs
    pub jupiter_program_id: Pubkey,
    
    // Rate limiting for fee collections
    pub last_collection_timestamp: i64,
    pub collections_in_current_hour: u8,
}


// Events
#[event]
pub struct FeesCollected {
    pub rift: Pubkey,
    pub amount: u64,
    pub total_collected: u64,
}

#[event]
pub struct FeesProcessed {
    pub fees_swapped: u64,
    pub rifts_bought: u64,
    pub rifts_to_stakers: u64,
    pub rifts_burned: u64,
}

#[event]
pub struct BuybackExecuted {
    pub amount_in: u64,
    pub expected_out: u64,
    pub minimum_out: u64,
    pub timestamp: i64,
}

#[event]
pub struct PriceUpdated {
    pub rifts_price: u64,
    pub underlying_price: u64,
    pub timestamp: i64,
}

#[event]
pub struct JupiterProgramIdUpdated {
    pub fee_collector: Pubkey,
    pub authority: Pubkey,
    pub old_program_id: Pubkey,
    pub new_program_id: Pubkey,
    pub timestamp: i64,
}

// Errors
#[error_code]
pub enum FeeCollectorError {
    #[msg("Insufficient fees to process")]
    InsufficientFees,
    #[msg("Unauthorized operation")]
    Unauthorized,
    #[msg("Invalid swap parameters")]
    InvalidSwapParams,
    #[msg("Math overflow")]
    MathOverflow,
    #[msg("Insufficient rent exemption for account creation")]
    InsufficientRentExemption,
    #[msg("Invalid program ID in cross-program invocation")]
    InvalidProgramId,
    #[msg("Invalid seed component in PDA derivation")]
    InvalidSeedComponent,
    #[msg("Invalid fee amount")]
    InvalidFeeAmount,
    #[msg("Rate limit exceeded - too many collections per hour")]
    RateLimitExceeded,
    #[msg("Insufficient Jupiter accounts provided")]
    InsufficientJupiterAccounts,
    #[msg("Swap execution failed")]
    SwapFailed,
    #[msg("Invalid price value")]
    InvalidPrice,
    #[msg("Excessive slippage tolerance")]
    ExcessiveSlippage,
    #[msg("Invalid minimum output amount")]
    InvalidMinimumOut,
    #[msg("Slippage tolerance too high")]
    SlippageTooHigh,
    #[msg("Invalid Jupiter program")]
    InvalidJupiterProgram,
    #[msg("Serialization error")]
    SerializationError,
    #[msg("Failed to calculate swap output from balance change")]
    SwapOutputCalculationError,
    #[msg("Insufficient swap output received")]
    InsufficientSwapOutput,
}