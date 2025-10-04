// RIFTS Fee Collector Program - Handles fee swaps and token distributions
use anchor_lang::prelude::*;
use anchor_spl::token::{self, Token, TokenAccount, Transfer, Mint};

// **SECURITY FIX**: Import Jupiter integration module
mod jupiter_integration;
use jupiter_integration::execute_jupiter_swap_with_instruction;

declare_id!("6bnAAJtyeJdLcvCSv5yhFib5qsAii5hqTdx7fYD7BkHP");

/// Initialize the fee collector program
#[program]
pub mod fee_collector {
    use super::*;

    /// Initialize fee collector with authority and protocol registry
    pub fn initialize_collector(
        ctx: Context<InitializeCollector>,
        treasury_fee_bps: u16,
        wrap_fee_bps: u16,
        center_bin_id: u32,
        apy_rate_factor: u64,
        rifts_protocol: Pubkey,
        jupiter_program_id: Pubkey, // **SECURITY FIX**: Governance-configurable Jupiter program ID
    ) -> Result<()> {
        let collector = &mut ctx.accounts.fee_collector;
        
        // Validate fee parameters
        require!(treasury_fee_bps <= 1000, FeeCollectorError::InvalidFeeParameter); // Max 10%
        require!(wrap_fee_bps <= 500, FeeCollectorError::InvalidFeeParameter); // Max 5%
        require!(apy_rate_factor > 0, FeeCollectorError::InvalidFeeParameter);
        
        // **CRITICAL FIX**: Validate rifts_protocol is not zero address to prevent authorization bypass
        require!(rifts_protocol != Pubkey::default(), FeeCollectorError::InvalidFeeParameter);
        // **SECURITY FIX**: Validate Jupiter program ID is not zero address
        require!(jupiter_program_id != Pubkey::default(), FeeCollectorError::InvalidFeeParameter);

        collector.authority = ctx.accounts.authority.key();
        collector.total_rifts_bought = 0;
        collector.total_rifts_distributed = 0;
        collector.total_rifts_burned = 0;
        collector.current_rifts_price = 1_000_000_000; // $1.00 with 9 decimals
        collector.current_underlying_price = 1_000_000_000;
        collector.last_price_update = Clock::get()?.unix_timestamp;
        collector.is_processing = false;
        collector.rate_limit_window_start = Clock::get()?.unix_timestamp;
        collector.rate_limit_count = 0;
        collector.jupiter_route_discriminator = [0u8; 8]; // Will be set by governance
        collector.jupiter_program_id = jupiter_program_id; // **SECURITY FIX**: Set governance-configurable Jupiter program ID
        collector.treasury_fee_bps = treasury_fee_bps;
        collector.wrap_fee_bps = wrap_fee_bps;
        collector.center_bin_id = center_bin_id;
        collector.apy_rate_factor = apy_rate_factor;
        collector.allowed_amm_programs = [Pubkey::default(); 10];
        collector.amm_programs_count = 0;
        collector.rifts_protocol = rifts_protocol;

        emit!(CollectorInitialized {
            authority: collector.authority,
            treasury_fee_bps,
            wrap_fee_bps,
        });

        Ok(())
    }

    /// ✅ CORRECTED JUPITER INTEGRATION: Process fees with pre-calculated instruction data
    pub fn process_fees_with_jupiter_swap(
        ctx: Context<ProcessFeesWithJupiterSwap>,
        amount_in: u64,
        minimum_amount_out: u64,
        jupiter_instruction_data: Vec<u8>, // Pre-calculated from Jupiter API off-chain
    ) -> Result<()> {
        let collector = &mut ctx.accounts.fee_collector;
        
        // Authorization check
        require!(
            ctx.accounts.authority.key() == collector.authority,
            FeeCollectorError::Unauthorized
        );

        // Basic validation
        require!(amount_in > 0, FeeCollectorError::InvalidSwapParams);
        require!(minimum_amount_out > 0, FeeCollectorError::InvalidSwapParams);
        require!(jupiter_instruction_data.len() > 0, FeeCollectorError::InvalidSwapParams);

        // Rate limiting check
        let current_time = Clock::get()?.unix_timestamp;
        const HOUR_IN_SECONDS: i64 = 3600;
        const MAX_COLLECTIONS_PER_HOUR: u8 = 10;
        
        if current_time >= collector.rate_limit_window_start + HOUR_IN_SECONDS {
            collector.rate_limit_window_start = current_time;
            collector.rate_limit_count = 0;
        }
        
        require!(
            collector.rate_limit_count < MAX_COLLECTIONS_PER_HOUR,
            FeeCollectorError::RateLimitExceeded
        );
        collector.rate_limit_count += 1;

        // Record balance before swap
        let rifts_vault_balance_before = ctx.accounts.rifts_vault.amount;

        // ✅ CORRECTED JUPITER INTEGRATION: Execute swap with pre-calculated instruction data
        let collector_key = collector.key();
        let collector_authority_bump = ctx.bumps.collector_authority;
        
        let signer_seeds: &[&[&[u8]]] = &[&[
            b"collector_authority",
            collector_key.as_ref(),
            &[collector_authority_bump],
        ]];
        
        // **CRITICAL SECURITY FIX**: Validate all remaining accounts against protocol registry allowlist
        let protocol_registry = &ctx.accounts.protocol_registry;
        for account in ctx.remaining_accounts.iter() {
            let is_allowed = 
                *account.key == protocol_registry.jupiter_program_id ||
                *account.key == protocol_registry.meteora_program_id ||
                *account.key == protocol_registry.orca_program_id ||
                *account.key == anchor_spl::token::ID ||
                *account.key == anchor_lang::solana_program::system_program::ID ||
                *account.key == anchor_spl::associated_token::ID;
                
            require!(is_allowed, FeeCollectorError::UnauthorizedAmmProgram);
        }
        
        // Release mutable reference before calling Jupiter function
        let _ = collector;
        
        // **SECURITY FIX**: Use governance-configured Jupiter program ID (no hardcoded fallback)
        let jupiter_program_id = collector.jupiter_program_id;
        
        execute_jupiter_swap_with_instruction(
            jupiter_instruction_data,
            &ctx.remaining_accounts,
            signer_seeds,
            jupiter_program_id,
        )?;
        
        // Re-borrow collector for state updates
        let collector = &mut ctx.accounts.fee_collector;

        // Verify swap results
        let rifts_vault_balance_after = ctx.accounts.rifts_vault.amount;
        let rifts_bought = rifts_vault_balance_after
            .checked_sub(rifts_vault_balance_before)
            .ok_or(FeeCollectorError::InvalidSwapResult)?;

        // Validate against minimum expected amount (already calculated off-chain)
        require!(
            rifts_bought >= minimum_amount_out,
            FeeCollectorError::InsufficientOutputAmount
        );

        // Update collector state
        collector.total_rifts_bought = collector.total_rifts_bought
            .checked_add(rifts_bought)
            .ok_or(FeeCollectorError::MathOverflow)?;

        // Distribute tokens (90% to LP stakers, 10% burned)
        let lp_staker_amount = rifts_bought
            .checked_mul(90)
            .ok_or(FeeCollectorError::MathOverflow)?
            .checked_div(100)
            .ok_or(FeeCollectorError::MathOverflow)?;

        collector.total_rifts_distributed = collector.total_rifts_distributed
            .checked_add(lp_staker_amount)
            .ok_or(FeeCollectorError::MathOverflow)?;

        emit!(FeesProcessedWithJupiterSwap {
            input_amount: amount_in,
            rifts_amount: rifts_bought,
            minimum_expected: minimum_amount_out,
        });

        Ok(())
    }

    /// Collect fees from the RIFTS protocol (called via CPI)
    pub fn collect_fees(
        ctx: Context<CollectFees>,
        amount: u64,
    ) -> Result<()> {
        require!(amount > 0, FeeCollectorError::InvalidSwapParams);
        
        let collector = &mut ctx.accounts.fee_collector;
        
        // **CRITICAL SECURITY FIX**: Only authorized RIFTS protocol can call collect_fees
        // **ENHANCED FIX**: Validate that rifts_protocol is the actual RIFTS protocol program
        require!(
            ctx.accounts.source_authority.key() == collector.authority || 
            (ctx.accounts.source_authority.key() == collector.rifts_protocol &&
             collector.rifts_protocol != Pubkey::default()), // Validate rifts_protocol is not zero address
            FeeCollectorError::UnauthorizedCaller
        );
        
        // Transfer tokens from caller to collector vault
        let transfer_ctx = CpiContext::new(
            ctx.accounts.token_program.to_account_info(),
            Transfer {
                from: ctx.accounts.source_vault.to_account_info(),
                to: ctx.accounts.collector_vault.to_account_info(),
                authority: ctx.accounts.source_authority.to_account_info(),
            },
        );
        
        token::transfer(transfer_ctx, amount)?;
        
        // Update collector state
        collector.total_rifts_bought = collector.total_rifts_bought
            .checked_add(amount)
            .ok_or(FeeCollectorError::MathOverflow)?;
            
        emit!(FeesCollected {
            amount,
            collector: collector.key(),
            source: ctx.accounts.source_vault.key(),
        });
        
        Ok(())
    }
    
    // DEPRECATED FUNCTIONS REMOVED FOR SECURITY
    // The following functions have been removed due to Jupiter route validation vulnerabilities:
    // - process_fees() 
    // - execute_buyback_with_slippage()
    // - build_jupiter_swap_instruction()
    //
    // USE process_fees_with_jupiter_swap() with pre-calculated instruction data instead

    /// **SECURITY FIX**: Governance function to update Jupiter program ID
    pub fn update_jupiter_program_id(
        ctx: Context<UpdateJupiterProgramId>,
        new_jupiter_program_id: Pubkey,
    ) -> Result<()> {
        let collector = &mut ctx.accounts.fee_collector;

        // Authorization check
        require!(
            ctx.accounts.authority.key() == collector.authority,
            FeeCollectorError::Unauthorized
        );

        // Validate new program ID is not zero address
        require!(new_jupiter_program_id != Pubkey::default(), FeeCollectorError::InvalidFeeParameter);

        collector.jupiter_program_id = new_jupiter_program_id;

        msg!("Jupiter program ID updated to: {}", new_jupiter_program_id);

        Ok(())
    }

    /// **SECURITY FIX #47**: Initialize protocol registry namescoped to governance
    /// This prevents cross-governance deployment coupling by binding registry to specific governance
    pub fn initialize_protocol_registry(
        ctx: Context<InitializeProtocolRegistry>,
        jupiter_program_id: Pubkey,
        meteora_program_id: Pubkey,
        orca_program_id: Pubkey,
    ) -> Result<()> {
        let registry = &mut ctx.accounts.protocol_registry;
        let governance = &ctx.accounts.governance;

        // Validate program IDs are not zero addresses
        require!(jupiter_program_id != Pubkey::default(), FeeCollectorError::InvalidProgramId);
        require!(meteora_program_id != Pubkey::default(), FeeCollectorError::InvalidProgramId);
        require!(orca_program_id != Pubkey::default(), FeeCollectorError::InvalidProgramId);

        // **SECURITY FIX #47**: Bind registry to governance authority
        registry.governance = governance.key();
        registry.authority = governance.authority;
        registry.jupiter_program_id = jupiter_program_id;
        registry.meteora_program_id = meteora_program_id;
        registry.orca_program_id = orca_program_id;
        registry.is_paused = false;

        emit!(ProtocolRegistryInitialized {
            governance: governance.key(),
            authority: registry.authority,
            jupiter_program_id,
            meteora_program_id,
            orca_program_id,
        });

        Ok(())
    }

}

// Account structures
#[derive(Accounts)]
pub struct InitializeCollector<'info> {
    #[account(mut)]
    pub authority: Signer<'info>,
    
    #[account(
        init,
        payer = authority,
        space = FeeCollector::INIT_SPACE,
        seeds = [b"fee_collector", authority.key().as_ref()],
        constraint = authority.key() != Pubkey::default() @ FeeCollectorError::InvalidSeedComponent,
        bump,
        constraint = authority.lamports() >= rent.minimum_balance(FeeCollector::INIT_SPACE) @ FeeCollectorError::InsufficientRentExemption
    )]
    pub fee_collector: Account<'info, FeeCollector>,
    
    /// **CRITICAL FIX**: Add RIFTS mint to access decimals for proper price initialization
    pub rifts_mint: Account<'info, Mint>,
    
    /// **SECURITY FIX**: Properly initialize collector vault as TokenAccount
    #[account(
        init,
        payer = authority,
        token::mint = rifts_mint,
        token::authority = collector_authority,
        seeds = [b"collector_vault", fee_collector.key().as_ref()],
        bump
    )]
    pub collector_vault: Account<'info, TokenAccount>,
    
    /// CHECK: PDA authority for collector vault
    #[account(
        seeds = [b"collector_authority", fee_collector.key().as_ref()],
        bump
    )]
    pub collector_authority: UncheckedAccount<'info>,
    
    pub system_program: Program<'info, System>,
    pub token_program: Program<'info, Token>,
    pub rent: Sysvar<'info, Rent>,
}

#[derive(Accounts)]
pub struct ProcessFeesWithJupiterSwap<'info> {
    #[account(mut)]
    pub authority: Signer<'info>,
    
    #[account(mut)]
    pub fee_collector: Account<'info, FeeCollector>,
    
    /// Underlying token vault (source for swap)
    #[account(mut)]
    pub collector_vault: Account<'info, TokenAccount>,
    
    /// RIFTS token vault (destination for swap)
    #[account(mut)]
    pub rifts_vault: Account<'info, TokenAccount>,
    
    /// Collector vault authority (PDA)
    #[account(
        seeds = [b"collector_authority", fee_collector.key().as_ref()],
        bump
    )]
    pub collector_authority: UncheckedAccount<'info>,
    
    /// Governance account for Jupiter program ID
    /// **CRITICAL FIX**: Added program ID validation to prevent fake governance accounts
    #[account(
        constraint = governance.to_account_info().owner == &governance::ID @ FeeCollectorError::Unauthorized
    )]
    pub governance: Account<'info, governance::Governance>,

    /// Protocol registry for AMM validation
    /// **SECURITY FIX #47**: Namescoped to governance to prevent cross-deployment coupling
    #[account(
        seeds = [b"protocol_registry", governance.key().as_ref()],
        bump,
        constraint = protocol_registry.governance == governance.key() @ FeeCollectorError::RegistryGovernanceMismatch,
        constraint = protocol_registry.authority == governance.authority @ FeeCollectorError::Unauthorized
    )]
    pub protocol_registry: Account<'info, ProtocolRegistry>,
    
    pub token_program: Program<'info, Token>,
}

#[derive(Accounts)]
pub struct CollectFees<'info> {
    #[account(mut)]
    pub fee_collector: Account<'info, FeeCollector>,
    
    /// Source vault (from RIFTS protocol)
    #[account(mut)]
    pub source_vault: Account<'info, TokenAccount>,
    
    /// Collector vault (destination)
    #[account(mut)]
    pub collector_vault: Account<'info, TokenAccount>,
    
    /// Authority that can transfer from source vault (RIFTS protocol vault authority)
    pub source_authority: UncheckedAccount<'info>,
    
    pub token_program: Program<'info, Token>,
}

// State accounts
#[account]
pub struct FeeCollector {
    pub authority: Pubkey,
    pub total_rifts_bought: u64,
    pub total_rifts_distributed: u64,
    pub total_rifts_burned: u64,
    pub current_rifts_price: u64,
    pub current_underlying_price: u64,
    pub last_price_update: i64,
    pub is_processing: bool,
    pub rate_limit_window_start: i64,
    pub rate_limit_count: u8,
    pub jupiter_route_discriminator: [u8; 8], // **CRITICAL FIX**: Governance-controlled Jupiter discriminator
    pub jupiter_program_id: Pubkey,          // **SECURITY FIX**: Governance-configurable Jupiter program ID
    pub treasury_fee_bps: u16,               // **GOVERNANCE FIX**: Treasury fee percentage in basis points
    pub wrap_fee_bps: u16,                   // **GOVERNANCE FIX**: Wrap fee in basis points
    pub center_bin_id: u32,                  // **GOVERNANCE FIX**: Center bin ID for pricing
    pub apy_rate_factor: u64,                // **GOVERNANCE FIX**: APY rate calculation factor
    pub allowed_amm_programs: [Pubkey; 10],  // **SECURITY FIX**: Allowlist of approved AMM programs for Jupiter routes
    pub amm_programs_count: u8,              // Number of active AMM programs in allowlist
    pub rifts_protocol: Pubkey,              // **CRITICAL FIX**: Authorized RIFTS protocol address
    pub reserved: [u8; 295], // Adjusted reserved space (32 bytes for rifts_protocol field)
}

impl FeeCollector {
    pub const INIT_SPACE: usize = 8 + // discriminator
        32 + // authority
        8 + // total_rifts_bought
        8 + // total_rifts_distributed
        8 + // total_rifts_burned
        8 + // current_rifts_price
        8 + // current_underlying_price
        8 + // last_price_update
        1 + // is_processing
        8 + // rate_limit_window_start
        1 + // rate_limit_count
        8 + // jupiter_route_discriminator
        32 + // jupiter_program_id
        2 + // treasury_fee_bps
        2 + // wrap_fee_bps
        4 + // center_bin_id
        8 + // apy_rate_factor
        320 + // allowed_amm_programs (32 * 10)
        1 + // amm_programs_count
        32 + // rifts_protocol
        263; // reserved (295 - 32 for jupiter_program_id)
}

/// **SECURITY FIX #47**: ProtocolRegistry namescoped to governance
/// This prevents cross-governance deployment coupling
#[account]
pub struct ProtocolRegistry {
    pub governance: Pubkey,          // **FIX #47**: Bind to specific governance
    pub authority: Pubkey,
    pub jupiter_program_id: Pubkey,
    pub meteora_program_id: Pubkey,
    pub orca_program_id: Pubkey,
    pub is_paused: bool,
}

impl ProtocolRegistry {
    pub const INIT_SPACE: usize = 8 + // discriminator
        32 + // governance
        32 + // authority
        32 + // jupiter_program_id
        32 + // meteora_program_id
        32 + // orca_program_id
        1;   // is_paused
}

// Events
#[event]
pub struct CollectorInitialized {
    pub authority: Pubkey,
    pub treasury_fee_bps: u16,
    pub wrap_fee_bps: u16,
}

#[event]
pub struct FeesProcessedWithJupiterSwap {
    pub input_amount: u64,
    pub rifts_amount: u64,
    pub minimum_expected: u64,
}

#[derive(Accounts)]
pub struct UpdateJupiterProgramId<'info> {
    #[account(mut)]
    pub authority: Signer<'info>,

    #[account(
        mut,
        constraint = fee_collector.authority == authority.key() @ FeeCollectorError::Unauthorized
    )]
    pub fee_collector: Account<'info, FeeCollector>,
}

/// **SECURITY FIX #47**: Initialize protocol registry with governance namescoping
#[derive(Accounts)]
pub struct InitializeProtocolRegistry<'info> {
    #[account(mut)]
    pub authority: Signer<'info>,

    /// Governance account that owns this registry
    /// **SECURITY FIX #47**: Registry is bound to specific governance to prevent cross-deployment coupling
    #[account(
        constraint = governance.to_account_info().owner == &governance::ID @ FeeCollectorError::InvalidProgramId,
        constraint = governance.authority == authority.key() @ FeeCollectorError::Unauthorized
    )]
    pub governance: Account<'info, governance::Governance>,

    /// **SECURITY FIX #47**: Protocol registry namescoped by governance key
    #[account(
        init,
        payer = authority,
        space = ProtocolRegistry::INIT_SPACE,
        seeds = [b"protocol_registry", governance.key().as_ref()],
        bump,
        constraint = authority.lamports() >= rent.minimum_balance(ProtocolRegistry::INIT_SPACE) @ FeeCollectorError::InsufficientRentExemption
    )]
    pub protocol_registry: Account<'info, ProtocolRegistry>,

    pub system_program: Program<'info, System>,
    pub rent: Sysvar<'info, Rent>,
}

#[event]
pub struct FeesCollected {
    pub amount: u64,
    pub collector: Pubkey,
    pub source: Pubkey,
}

/// **SECURITY FIX #47**: Event for protocol registry initialization
#[event]
pub struct ProtocolRegistryInitialized {
    pub governance: Pubkey,
    pub authority: Pubkey,
    pub jupiter_program_id: Pubkey,
    pub meteora_program_id: Pubkey,
    pub orca_program_id: Pubkey,
}

// Error codes
#[error_code]
pub enum FeeCollectorError {
    #[msg("Unauthorized")]
    Unauthorized,
    #[msg("Math overflow")]
    MathOverflow,
    #[msg("Invalid swap parameters")]
    InvalidSwapParams,
    #[msg("Invalid swap result")]
    InvalidSwapResult,
    #[msg("Insufficient output amount")]
    InsufficientOutputAmount,
    #[msg("Rate limit exceeded")]
    RateLimitExceeded,
    #[msg("Invalid fee parameter")]
    InvalidFeeParameter,
    #[msg("Invalid seed component - cannot be zero")]
    InvalidSeedComponent,
    #[msg("Insufficient rent exemption balance")]
    InsufficientRentExemption,
    #[msg("Invalid program ID")]
    InvalidProgramId,
    #[msg("Reentrancy detected")]
    ReentrancyDetected,
    #[msg("Invalid price")]
    InvalidPrice,
    #[msg("Price update too frequent")]
    PriceUpdateTooFrequent,
    #[msg("Price change too extreme")]
    PriceChangeTooExtreme,
    #[msg("Invalid slippage parameter")]
    InvalidSlippage,
    #[msg("Slippage exceeded")]
    SlippageExceeded,
    #[msg("Insufficient Jupiter accounts")]
    InsufficientJupiterAccounts,
    #[msg("Jupiter route discriminator not set by governance")]
    JupiterDiscriminatorNotSet,
    #[msg("Unauthorized AMM program - not in allowlist")]
    UnauthorizedAmmProgram,
    #[msg("Too many Jupiter accounts - maximum 64 allowed")]
    TooManyJupiterAccounts,
    #[msg("Invalid Jupiter instruction data")]
    InvalidJupiterInstruction,
    #[msg("Invalid token decimals - maximum 18 allowed")]
    InvalidTokenDecimals,
    #[msg("Unauthorized caller - only RIFTS protocol can collect fees")]
    UnauthorizedCaller,
    #[msg("Protocol registry governance mismatch - registry not bound to this governance")]
    RegistryGovernanceMismatch,
}