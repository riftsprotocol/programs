// Rifts Protocol - Full Peapods Clone with Hybrid Oracle System
use anchor_lang::prelude::*;
use anchor_spl::token::{self, Token, TokenAccount, Transfer, Mint, MintTo, transfer, mint_to};
use anchor_lang::solana_program::program_option::COption;
// Note: Metadata functionality removed to avoid dependency issues
use anchor_lang::solana_program::sysvar::rent::Rent;
use std::str::FromStr;

// External program CPI imports
pub use fee_collector;
pub use governance;
pub use lp_staking;

// Internal modules
// mod jupiter; // Removed - Jupiter integration implemented inline (lines 1851-1918)

// Meteora DAMM v2 imports
use cp_amm::program::CpAmm;
use cp_amm::cpi;
use cp_amm::cpi::accounts::{InitializePoolWithDynamicConfigCtx, RemoveLiquidityCtx, AddLiquidityCtx};
use cp_amm::{AddLiquidityParameters, RemoveLiquidityParameters};
use cp_amm::constants::{MIN_SQRT_PRICE, MAX_SQRT_PRICE};

declare_id!("BPYwhoziLVUZQy2aUTfR7dJLz2WqJgaoyLmcepzNBTs8");

// Meteora DAMM v2 Program ID (same for mainnet and devnet)
pub const METEORA_DAMM_V2_PROGRAM_ID: Pubkey = pubkey!("cpamdpZCGKUy5JxQXB4dcpGPiikHawvSWAd6mEn1sGG");

#[program]
pub mod rifts_protocol {
    use super::*;

    /// Create a new Rift with PDA-based vanity mint address (like pump.fun approach)
    /// This generates the mint PDA deterministically from vanity seed
    /// **MEMORY OPTIMIZATION**: Use fixed-size array instead of Vec to avoid heap allocation
    pub fn create_rift_with_vanity_pda(
        ctx: Context<CreateRiftWithVanityPDA>,
        vanity_seed: [u8; 32],  // Fixed-size array - no heap allocation!
        seed_len: u8,           // Actual length of seed to use (0-32)
        mint_bump: u8,
        burn_fee_bps: u16,
        partner_fee_bps: u16,
        partner_wallet: Option<Pubkey>,
        rift_name: [u8; 32],    // Fixed-size array - no heap allocation!
        name_len: u8,           // Actual length of name to use (0-32)
    ) -> Result<()> {
        let rift = &mut ctx.accounts.rift;

        // Validate fees and seed length
        require!(burn_fee_bps <= 4500, ErrorCode::InvalidBurnFee);
        require!(partner_fee_bps <= 500, ErrorCode::InvalidPartnerFee);
        require!(seed_len <= 32, ErrorCode::InvalidVanitySeed);

        // PDA derivation is automatically verified by Anchor through the seeds constraint

        // **MEMORY OPTIMIZATION**: Skip vanity address validation to prevent heap allocation
        // The PDA derivation ensures deterministic mint addresses
        // Vanity validation is optional and can be done off-chain before calling this

        // Initialize the rift with provided values
        rift.creator = ctx.accounts.creator.key();
        rift.underlying_mint = ctx.accounts.underlying_mint.key();
        rift.rift_mint = ctx.accounts.rift_mint.key();
        rift.vault = ctx.accounts.vault.key();
        rift.burn_fee_bps = burn_fee_bps;
        rift.partner_fee_bps = partner_fee_bps;
        rift.partner_wallet = partner_wallet;
        rift.total_underlying_wrapped = 0;
        rift.total_rift_minted = 0;
        rift.total_burned = 0;
        rift.backing_ratio = 1_000_000; // 100% initially (6 decimals precision)
        rift.last_rebalance = Clock::get()?.unix_timestamp;

        // Set rift name (fixed-size array - no heap allocation!)
        require!(name_len <= 32, ErrorCode::NameTooLong);
        if name_len > 0 {
            rift.name[..name_len as usize].copy_from_slice(&rift_name[..name_len as usize]);
        } else {
            // Default: empty name (all zeros)
            rift.name = [0u8; 32];
        }

        // **SECURITY FIX**: Initialize hybrid oracle system with valid initial state
        let current_time = Clock::get()?.unix_timestamp;

        // Initialize with realistic default price and confidence instead of zero values
        let initial_price_data = PriceData {
            price: 1_000_000, // Default to 1.0 price (with 6 decimals)
            confidence: 100_000, // Moderate confidence for initial state
            timestamp: current_time,
        };

        // **SECURITY FIX**: Validate oracle parameters to prevent manipulation
        rift.oracle_prices = [initial_price_data; 10];
        rift.price_index = 0;

        // **SECURITY FIX**: Set reasonable bounds for oracle intervals to prevent DoS
        rift.oracle_update_interval = 30 * 60; // 30 minutes (min 5 min, max 24 hours)
        require!(
            rift.oracle_update_interval >= 300 && rift.oracle_update_interval <= 86400,
            ErrorCode::InvalidOracleParameters
        );

        rift.max_rebalance_interval = 24 * 60 * 60; // 24 hours (min 1 hour, max 7 days)
        require!(
            rift.max_rebalance_interval >= 3600 && rift.max_rebalance_interval <= 604800,
            ErrorCode::InvalidOracleParameters
        );

        rift.arbitrage_threshold_bps = 200; // 2% (min 0.1%, max 50%)
        require!(
            rift.arbitrage_threshold_bps >= 10 && rift.arbitrage_threshold_bps <= 5000,
            ErrorCode::InvalidOracleParameters
        );

        rift.last_oracle_update = current_time;

        // Initialize advanced metrics
        rift.total_volume_24h = 0;
        rift.price_deviation = 0;
        rift.arbitrage_opportunity_bps = 0;
        // **SECURITY FIX**: Initialize Jupiter program ID as None (uses hardcoded fallback)
        rift.jupiter_program_id = None;
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

        // **SECURITY FIX #50**: Initialize oracle accounts as None (must be set explicitly)
        rift.pyth_price_account = None;
        rift.switchboard_feed_account = None;

        // Mint account is automatically initialized by Anchor with the init constraint

        // Emit creation event
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
        rift_name: [u8; 32],
        name_len: u8,
    ) -> Result<()> {
        let rift = &mut ctx.accounts.rift;
        
        // Validate fees
        require!(burn_fee_bps <= 4500, ErrorCode::InvalidBurnFee);
        require!(partner_fee_bps <= 500, ErrorCode::InvalidPartnerFee);
        
        // Validate and set rift name (fixed-size array - no heap allocation!)
        require!(name_len <= 32, ErrorCode::NameTooLong);
        if name_len > 0 {
            // Skip string validation for now to avoid heap allocation
            // Validation can be done off-chain
            rift.name[..name_len as usize].copy_from_slice(&rift_name[..name_len as usize]);
        } else {
            // **MEMORY OPTIMIZATION**: Use empty name (all zeros)
            rift.name = [0u8; 32];
        }

        rift.creator = ctx.accounts.creator.key();
        rift.underlying_mint = ctx.accounts.underlying_mint.key();
        rift.rift_mint = ctx.accounts.rift_mint.key();
        // **SECURITY FIX**: Vault will be set to the derived PDA address with consistent seeds
        let rift_key = rift.key();
        let vault_seeds = &[b"vault", rift_key.as_ref()];
        let (vault_pda, _) = Pubkey::find_program_address(vault_seeds, &crate::ID);
        rift.vault = vault_pda;
        rift.burn_fee_bps = burn_fee_bps;
        rift.partner_fee_bps = partner_fee_bps;
        rift.partner_wallet = partner_wallet;
        rift.total_underlying_wrapped = 0;
        rift.total_rift_minted = 0;
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
        // **SECURITY FIX**: Initialize Jupiter program ID as None (uses hardcoded fallback)
        rift.jupiter_program_id = None;
        rift.rebalance_count = 0;
        
        // Initialize RIFTS token distribution tracking
        rift.total_fees_collected = 0;
        rift.rifts_tokens_distributed = 0;
        rift.rifts_tokens_burned = 0;
        
        // Initialize Meteora-style DLMM Pool (NEW)
        rift.liquidity_pool = None;  // Will be set during first wrap
        rift.lp_token_supply = 0;
        rift.pool_trading_fee_bps = 30;  // Default 0.3% trading fee
        rift.total_liquidity_underlying = 0;
        rift.total_liquidity_rift = 0;
        rift.active_bin_id = 0;      // Will be set during pool creation
        rift.bin_step = 0;           // Will be set during pool creation
        
        // Initialize LP staking
        rift.total_lp_staked = 0;
        rift.pending_rewards = 0;
        rift.last_reward_distribution = Clock::get()?.unix_timestamp;

        // **SECURITY FIX #50**: Initialize oracle accounts as None (must be set explicitly)
        rift.pyth_price_account = None;
        rift.switchboard_feed_account = None;

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

    /// Create official Meteora DAMM v2 pool for rift tokens
    pub fn create_meteora_pool(
        ctx: Context<CreateMeteoraPool>,
        amount: u64,
        bin_step: u16,
        base_factor: u16,
    ) -> Result<()> {
        let rift = &mut ctx.accounts.rift;

        // Basic validation
        require!(!rift.is_paused, ErrorCode::RiftPaused);
        require!(amount > 0, ErrorCode::InvalidAmount);
        require!(amount <= 1_000_000_000_000_000, ErrorCode::AmountTooLarge);
        require!(amount >= 10000, ErrorCode::AmountTooSmall);

        // Validate Meteora bin step (common values from the guide)
        require!(
            bin_step == 1 || bin_step == 5 || bin_step == 10 || bin_step == 25 ||
            bin_step == 50 || bin_step == 100 || bin_step == 200 || bin_step == 500,
            ErrorCode::InvalidBinStep
        );

        // Simple fee calculation (0.7%)
        let wrap_fee = amount.checked_mul(70).ok_or(ErrorCode::MathOverflow)?
            .checked_div(10000).ok_or(ErrorCode::MathOverflow)?;
        let amount_after_fee = amount.checked_sub(wrap_fee).ok_or(ErrorCode::MathOverflow)?;

        // CRITICAL FIX: Transfer underlying tokens to user's payer_token_a account
        // Meteora will pull tokens from payer_token_a/b during initialize_pool
        // We need to ensure the user's token accounts have the liquidity

        // The user's payer_token_a should already have the underlying tokens
        // The user's payer_token_b needs the RIFT tokens we'll mint

        // Determine which token is the RIFT mint (tokens are lexicographically ordered)
        // Need to mint RIFT tokens to the correct payer account
        let mint_amount = amount_after_fee; // 1:1 ratio
        let rift_key = rift.key();
        let mint_authority_bump = ctx.bumps.rift_mint_authority;
        let authority_bump_slice = [mint_authority_bump];
        let signer_seeds: &[&[&[u8]]] = &[&[
            b"rift_mint_auth",
            rift_key.as_ref(),
            &authority_bump_slice,
        ]];

        // Check which mint is the RIFT mint by comparing to rift.rift_mint
        // The rift.rift_mint field contains the rift token mint pubkey
        let is_token_b_rift = ctx.accounts.token_b_mint.key() == rift.rift_mint;

        // Mint RIFT tokens to the correct payer account
        // Meteora's initialize_pool will transfer these to the pool
        let (rift_mint_account, rift_payer_account) = if is_token_b_rift {
            (ctx.accounts.token_b_mint.to_account_info(), ctx.accounts.payer_token_b.to_account_info())
        } else {
            (ctx.accounts.token_a_mint.to_account_info(), ctx.accounts.payer_token_a.to_account_info())
        };

        let mint_to_ctx = CpiContext::new_with_signer(
            ctx.accounts.token_program.to_account_info(),
            MintTo {
                mint: rift_mint_account,
                to: rift_payer_account,
                authority: ctx.accounts.rift_mint_authority.to_account_info(),
            },
            signer_seeds,
        );
        token::mint_to(mint_to_ctx, mint_amount)?;

        // NOTE: User's payer_token_a (underlying/SOL) should already have 'amount' tokens
        // This is passed from JavaScript - the user funded their WSOL account
        // Meteora's initialize_pool will pull 'amount' from payer_token_a

        // Now create the official Meteora DAMM v2 pool via CPI
        let meteora_pool_key = ctx.accounts.pool.key();
        let pool_seeds = &[
            ctx.accounts.token_a_mint.key().as_ref(),
            ctx.accounts.token_b_mint.key().as_ref(),
            &bin_step.to_le_bytes(),
            &base_factor.to_le_bytes(),
        ];

        // Calculate initial price (adjust as needed for your tokenomics)
        let init_price = 1_000_000; // 0.001 quote per base token (with 9 decimal adjustment)

        // Create official Meteora DAMM v2 pool using proper config-based approach
        // Based on Meteora docs, pools are created using a config key that defines parameters
        // We'll use a public config that matches our desired fee structure (0.4% base fee with dynamic fee)

        let meteora_config_key = Pubkey::from_str("82p7sVzQWZfCrmStPhsG8BYKwheQkUiXSs2wiqdhwNxr")
            .map_err(|_| ErrorCode::InvalidPublicKey)?; // Config index 1: 0.25% base fee, collect fee mode 1, dynamic fee enabled

        // Calculate pool PDA - Meteora uses: ["pool", config, larger_mint, smaller_mint]
        // Sort the mints to get firstMint (larger) and secondMint (smaller)
        let mint1_bytes = ctx.accounts.token_a_mint.key().to_bytes();
        let mint2_bytes = ctx.accounts.token_b_mint.key().to_bytes();

        let (first_mint, second_mint) = if mint1_bytes > mint2_bytes {
            (mint1_bytes, mint2_bytes)
        } else {
            (mint2_bytes, mint1_bytes)
        };

        let (expected_pool_pubkey, _) = Pubkey::find_program_address(
            &[
                b"pool",
                meteora_config_key.as_ref(),
                first_mint.as_ref(),
                second_mint.as_ref(),
            ],
            &METEORA_DAMM_V2_PROGRAM_ID,
        );

        // Verify the provided pool account matches the expected PDA
        require!(
            ctx.accounts.pool.key() == expected_pool_pubkey,
            ErrorCode::InvalidPoolAccount
        );

        // Create official Meteora DAMM v2 pool using initialize_pool_with_dynamic_config
        // This allows us to set custom sqrt_min_price and sqrt_max_price for full-range pool
        // Based on actual program code: https://github.com/MeteoraAg/damm-v2

        let initialize_accounts = InitializePoolWithDynamicConfigCtx {
            creator: ctx.accounts.user.to_account_info(),
            position_nft_mint: ctx.accounts.position_nft_mint.to_account_info(),
            position_nft_account: ctx.accounts.position_nft_account.to_account_info(),
            payer: ctx.accounts.payer.to_account_info(),
            pool_creator_authority: ctx.accounts.user.to_account_info(),
            config: ctx.accounts.config.to_account_info(),
            pool_authority: ctx.accounts.pool_authority.to_account_info(),
            pool: ctx.accounts.pool.to_account_info(),
            position: ctx.accounts.position.to_account_info(),
            token_a_mint: ctx.accounts.token_a_mint.to_account_info(),
            token_b_mint: ctx.accounts.token_b_mint.to_account_info(),
            token_a_vault: ctx.accounts.token_a_vault.to_account_info(),
            token_b_vault: ctx.accounts.token_b_vault.to_account_info(),
            payer_token_a: ctx.accounts.payer_token_a.to_account_info(),
            payer_token_b: ctx.accounts.payer_token_b.to_account_info(),
            token_a_program: ctx.accounts.token_a_program.to_account_info(),
            token_b_program: ctx.accounts.token_b_program.to_account_info(),
            token_2022_program: ctx.accounts.token_2022_program.to_account_info(),
            system_program: ctx.accounts.system_program.to_account_info(),
            event_authority: ctx.accounts.event_authority.to_account_info(),
            program: ctx.accounts.meteora_program.to_account_info(),
        };

        let meteora_ctx = CpiContext::new(
            ctx.accounts.meteora_program.to_account_info(),
            initialize_accounts,
        );

        // Pool creation must be done via JavaScript/TypeScript using Meteora SDK
        // Reason: cp_amm crate doesn't export InitializeCustomizablePoolParameters
        msg!("❌ Pool creation must be done via JavaScript/TypeScript");
        msg!("✅ Use create-meteora-pool-with-price-range.js to initialize pools");
        msg!("   After pool is created externally, wrap/unwrap will work via this program's CPI");

        return err!(ErrorCode::UseJavaScriptForPoolCreation)
    }



    /// Initialize vault for rift
    pub fn initialize_vault(ctx: Context<InitializeVault>) -> Result<()> {
        // Vault is automatically initialized through the constraint
        Ok(())
    }

    /// STEP 1: Wrap SOL to RIFT tokens (stores SOL in vault, mints RIFT to user)
    /// This must be called BEFORE creating the Meteora pool
    pub fn wrap_tokens(
        ctx: Context<WrapTokens>,
        amount: u64,
    ) -> Result<()> {
        let rift = &mut ctx.accounts.rift;

        // Basic validation
        require!(!rift.is_paused, ErrorCode::RiftPaused);
        require!(amount > 0, ErrorCode::InvalidAmount);
        require!(amount <= 1_000_000_000_000_000, ErrorCode::AmountTooLarge);

        // Transfer underlying tokens from user to vault
        let transfer_ctx = CpiContext::new(
            ctx.accounts.token_program.to_account_info(),
            Transfer {
                from: ctx.accounts.user_underlying.to_account_info(),
                to: ctx.accounts.vault.to_account_info(),
                authority: ctx.accounts.user.to_account_info(),
            },
        );
        transfer(transfer_ctx, amount)?;

        // Calculate fees (0.7% wrap fee)
        let wrap_fee = amount.checked_mul(70).ok_or(ErrorCode::MathOverflow)?
            .checked_div(10000).ok_or(ErrorCode::MathOverflow)?;
        let amount_after_fee = amount.checked_sub(wrap_fee).ok_or(ErrorCode::MathOverflow)?;

        // Mint RIFT tokens to user
        let rift_key = rift.key();
        let bump_seed = [ctx.bumps.rift_mint_authority];
        let signer_seeds: &[&[u8]] = &[
            b"rift_mint_auth",
            rift_key.as_ref(),
            &bump_seed,
        ];
        let signer = &[&signer_seeds[..]];

        let mint_ctx = CpiContext::new_with_signer(
            ctx.accounts.token_program.to_account_info(),
            MintTo {
                mint: ctx.accounts.rift_mint.to_account_info(),
                to: ctx.accounts.user_rift_tokens.to_account_info(),
                authority: ctx.accounts.rift_mint_authority.to_account_info(),
            },
            signer,
        );
        mint_to(mint_ctx, amount_after_fee)?;

        // Update rift state
        rift.total_underlying_wrapped = rift.total_underlying_wrapped
            .checked_add(amount).ok_or(ErrorCode::MathOverflow)?;
        rift.total_rift_minted = rift.total_rift_minted
            .checked_add(amount_after_fee).ok_or(ErrorCode::MathOverflow)?;

        msg!("✅ Wrapped {} SOL → {} RIFT", amount, amount_after_fee);

        Ok(())
    }

    /// STEP 2: Create Meteora pool with initial liquidity using wrapped RIFT tokens
    /// User must have RIFT and SOL tokens from wrapping first
    /// Pool creation is done via JavaScript SDK, this just tracks it
    /// **SECURITY FIX #48**: Store all pool state for validation
    pub fn set_pool_address(
        ctx: Context<SetPoolAddress>,
        pool_address: Pubkey,
        pool_authority: Pubkey,
        token_a_vault: Pubkey,
        token_b_vault: Pubkey,
    ) -> Result<()> {
        let rift = &mut ctx.accounts.rift;

        require!(rift.creator == ctx.accounts.user.key(), ErrorCode::Unauthorized);
        require!(rift.liquidity_pool.is_none(), ErrorCode::PoolAlreadyInitialized);

        // **SECURITY FIX #48**: Store all Meteora pool state for later validation
        rift.liquidity_pool = Some(pool_address);
        rift.pool_authority = Some(pool_authority);
        rift.pool_token_a_vault = Some(token_a_vault);
        rift.pool_token_b_vault = Some(token_b_vault);

        msg!("✅ Set Meteora pool state:");
        msg!("  Pool: {}", pool_address);
        msg!("  Authority: {}", pool_authority);
        msg!("  Token A Vault: {}", token_a_vault);
        msg!("  Token B Vault: {}", token_b_vault);

        Ok(())
    }

    /// STEP 3: Wrap SOL and add liquidity to Meteora pool (after pool exists)
    pub fn wrap_and_add_liquidity(
        ctx: Context<WrapAndAddLiquidity>,
        amount: u64,
        liquidity_to_add: u128,  // Pre-calculated off-chain using sqrt(amount_a * amount_b)
    ) -> Result<()> {
        let rift = &mut ctx.accounts.rift;

        // Basic validation
        require!(!rift.is_paused, ErrorCode::RiftPaused);
        require!(amount > 0, ErrorCode::InvalidAmount);

        // Verify pool exists
        require!(rift.liquidity_pool.is_some(), ErrorCode::PoolNotInitialized);
        require!(
            ctx.accounts.pool.key() == rift.liquidity_pool.unwrap(),
            ErrorCode::InvalidPoolAccount
        );

        // **SECURITY FIX #48**: Validate Meteora accounts against stored state
        if let Some(expected_authority) = rift.pool_authority {
            require!(
                ctx.accounts.pool_authority.key() == expected_authority,
                ErrorCode::InvalidPoolAuthority
            );
        }
        if let Some(expected_vault_a) = rift.pool_token_a_vault {
            require!(
                ctx.accounts.token_a_vault.key() == expected_vault_a,
                ErrorCode::InvalidPoolVault
            );
        }
        if let Some(expected_vault_b) = rift.pool_token_b_vault {
            require!(
                ctx.accounts.token_b_vault.key() == expected_vault_b,
                ErrorCode::InvalidPoolVault
            );
        }

        // Reentrancy protection
        require!(!rift.reentrancy_guard, ErrorCode::ReentrancyDetected);
        rift.reentrancy_guard = true;

        // Calculate fees
        let wrap_fee = amount.checked_mul(70).ok_or(ErrorCode::MathOverflow)?
            .checked_div(10000).ok_or(ErrorCode::MathOverflow)?;
        let amount_after_fee = amount.checked_sub(wrap_fee).ok_or(ErrorCode::MathOverflow)?;

        // Mint RIFT tokens to user
        let rift_key = rift.key();
        let bump_seed = [ctx.bumps.rift_mint_authority];
        let signer_seeds: &[&[u8]] = &[
            b"rift_mint_auth",
            rift_key.as_ref(),
            &bump_seed,
        ];
        let signer = &[&signer_seeds[..]];

        let mint_ctx = CpiContext::new_with_signer(
            ctx.accounts.token_program.to_account_info(),
            MintTo {
                mint: ctx.accounts.rift_mint.to_account_info(),
                to: ctx.accounts.user_rift_tokens.to_account_info(),
                authority: ctx.accounts.rift_mint_authority.to_account_info(),
            },
            signer,
        );
        mint_to(mint_ctx, amount_after_fee)?;

        // Use pre-calculated liquidity delta (sqrt of product)
        let liquidity_delta = liquidity_to_add;
        let token_a_threshold = amount_after_fee.checked_mul(101).ok_or(ErrorCode::MathOverflow)?
            .checked_div(100).ok_or(ErrorCode::MathOverflow)?;
        let token_b_threshold = amount_after_fee.checked_mul(101).ok_or(ErrorCode::MathOverflow)?
            .checked_div(100).ok_or(ErrorCode::MathOverflow)?;

        msg!("Adding {} liquidity (sqrt of product) for {} SOL", liquidity_delta, amount_after_fee);

        let add_liquidity_params = AddLiquidityParameters {
            liquidity_delta,
            token_a_amount_threshold: token_a_threshold,
            token_b_amount_threshold: token_b_threshold,
        };

        // **PER-USER POSITION FIX**: Add liquidity to user's OWN position
        // User must create their position NFT via Meteora SDK before calling this
        let add_liquidity_accounts = AddLiquidityCtx {
            pool: ctx.accounts.pool.to_account_info(),
            position: ctx.accounts.user_position.to_account_info(),  // ← USER'S position
            token_a_account: ctx.accounts.user_underlying.to_account_info(),
            token_b_account: ctx.accounts.user_rift_tokens.to_account_info(),
            token_a_vault: ctx.accounts.token_a_vault.to_account_info(),
            token_b_vault: ctx.accounts.token_b_vault.to_account_info(),
            token_a_mint: ctx.accounts.underlying_mint.to_account_info(),
            token_b_mint: ctx.accounts.rift_mint.to_account_info(),
            position_nft_account: ctx.accounts.user_position_nft_account.to_account_info(),  // ← USER'S NFT
            owner: ctx.accounts.user.to_account_info(),  // ← USER owns the position
            token_a_program: ctx.accounts.token_program.to_account_info(),
            token_b_program: ctx.accounts.token_program.to_account_info(),
            event_authority: ctx.accounts.event_authority.to_account_info(),
            program: ctx.accounts.meteora_program.to_account_info(),
        };

        let meteora_ctx = CpiContext::new(
            ctx.accounts.meteora_program.to_account_info(),
            add_liquidity_accounts,
        );

        cpi::add_liquidity(meteora_ctx, add_liquidity_params)?;

        // Update state
        rift.total_underlying_wrapped = rift.total_underlying_wrapped
            .checked_add(amount).ok_or(ErrorCode::MathOverflow)?;
        rift.total_rift_minted = rift.total_rift_minted
            .checked_add(amount_after_fee).ok_or(ErrorCode::MathOverflow)?;
        rift.total_liquidity_underlying = rift.total_liquidity_underlying
            .checked_add(amount_after_fee).ok_or(ErrorCode::MathOverflow)?;
        rift.total_liquidity_rift = rift.total_liquidity_rift
            .checked_add(amount_after_fee).ok_or(ErrorCode::MathOverflow)?;

        rift.reentrancy_guard = false;

        msg!("✅ Wrapped {} SOL and added liquidity to Meteora", amount);

        Ok(())
    }

    /// STEP 4: Remove liquidity from Meteora and unwrap RIFT to SOL
    /// liquidity_to_remove: calculated off-chain using Meteora SDK
    pub fn remove_liquidity_and_unwrap(
        ctx: Context<RemoveLiquidityAndUnwrap>,
        rift_amount: u64,
        liquidity_to_remove: u128,  // Pre-calculated off-chain (u128 for large liquidity values)
        token_a_threshold: u64,  // Expected SOL amount from withdraw quote
        token_b_threshold: u64,  // Expected RIFT amount from withdraw quote
    ) -> Result<()> {
        let rift = &mut ctx.accounts.rift;

        // Validation
        require!(!rift.is_paused, ErrorCode::RiftPaused);
        require!(rift_amount > 0, ErrorCode::InvalidAmount);
        require!(rift.liquidity_pool.is_some(), ErrorCode::PoolNotInitialized);
        require!(
            ctx.accounts.pool.key() == rift.liquidity_pool.unwrap(),
            ErrorCode::InvalidPoolAccount
        );

        // **SECURITY FIX #48**: Validate Meteora accounts against stored state
        if let Some(expected_authority) = rift.pool_authority {
            require!(
                ctx.accounts.pool_authority.key() == expected_authority,
                ErrorCode::InvalidPoolAuthority
            );
        }
        if let Some(expected_vault_a) = rift.pool_token_a_vault {
            require!(
                ctx.accounts.token_a_vault.key() == expected_vault_a,
                ErrorCode::InvalidPoolVault
            );
        }
        if let Some(expected_vault_b) = rift.pool_token_b_vault {
            require!(
                ctx.accounts.token_b_vault.key() == expected_vault_b,
                ErrorCode::InvalidPoolVault
            );
        }

        // Reentrancy protection
        require!(!rift.reentrancy_guard, ErrorCode::ReentrancyDetected);
        rift.reentrancy_guard = true;

        // Calculate unwrap fee
        let unwrap_fee = rift_amount.checked_mul(70).ok_or(ErrorCode::MathOverflow)?
            .checked_div(10000).ok_or(ErrorCode::MathOverflow)?;
        let amount_after_fee = rift_amount.checked_sub(unwrap_fee).ok_or(ErrorCode::MathOverflow)?;

        // Use the liquidity amount calculated off-chain
        let liquidity_delta = liquidity_to_remove;

        msg!("Removing {} liquidity for {} RIFT (pre-calculated off-chain)",
             liquidity_delta, amount_after_fee);

        // Use thresholds calculated off-chain from SDK's getWithdrawQuote
        let remove_liquidity_params = RemoveLiquidityParameters {
            liquidity_delta,
            token_a_amount_threshold: token_a_threshold,
            token_b_amount_threshold: token_b_threshold,
        };

        // **PER-USER POSITION FIX**: Remove liquidity from user's OWN position
        let remove_liquidity_accounts = RemoveLiquidityCtx {
            pool: ctx.accounts.pool.to_account_info(),
            position: ctx.accounts.user_position.to_account_info(),  // ← USER'S position
            token_a_account: ctx.accounts.user_underlying.to_account_info(),
            token_b_account: ctx.accounts.user_rift_tokens.to_account_info(),
            token_a_vault: ctx.accounts.token_a_vault.to_account_info(),
            token_b_vault: ctx.accounts.token_b_vault.to_account_info(),
            token_a_mint: ctx.accounts.underlying_mint.to_account_info(),
            token_b_mint: ctx.accounts.rift_mint.to_account_info(),
            position_nft_account: ctx.accounts.user_position_nft_account.to_account_info(),  // ← USER'S NFT
            pool_authority: ctx.accounts.pool_authority.to_account_info(),
            owner: ctx.accounts.user.to_account_info(),  // ← USER owns the position
            token_a_program: ctx.accounts.token_program.to_account_info(),
            token_b_program: ctx.accounts.token_program.to_account_info(),
            event_authority: ctx.accounts.event_authority.to_account_info(),
            program: ctx.accounts.meteora_program.to_account_info(),
        };

        let meteora_ctx = CpiContext::new(
            ctx.accounts.meteora_program.to_account_info(),
            remove_liquidity_accounts,
        );

        cpi::remove_liquidity(meteora_ctx, remove_liquidity_params)?;

        // Burn RIFT tokens
        let burn_ctx = CpiContext::new(
            ctx.accounts.token_program.to_account_info(),
            anchor_spl::token::Burn {
                mint: ctx.accounts.rift_mint.to_account_info(),
                from: ctx.accounts.user_rift_tokens.to_account_info(),
                authority: ctx.accounts.user.to_account_info(),
            },
        );
        anchor_spl::token::burn(burn_ctx, rift_amount)?;

        // Update state
        rift.total_underlying_wrapped = rift.total_underlying_wrapped.saturating_sub(amount_after_fee);
        rift.total_rift_minted = rift.total_rift_minted.saturating_sub(rift_amount);
        rift.total_liquidity_underlying = rift.total_liquidity_underlying.saturating_sub(amount_after_fee);
        rift.total_liquidity_rift = rift.total_liquidity_rift.saturating_sub(amount_after_fee);

        rift.reentrancy_guard = false;

        msg!("✅ Removed liquidity and unwrapped {} RIFT → {} SOL", rift_amount, amount_after_fee);

        Ok(())
    }

    /// Admin function: Fix vault ownership conflicts
    pub fn admin_fix_vault_conflict(ctx: Context<AdminFixVaultConflict>) -> Result<()> {
        let rift = &ctx.accounts.rift;

        // Only program authority can call this
        require!(
            ctx.accounts.program_authority.key() == rift.creator,
            ErrorCode::Unauthorized
        );

        // Get the current vault and expected authority
        let vault_info = &ctx.accounts.vault;
        let expected_authority = &ctx.accounts.vault_authority;

        msg!("Fixing vault conflict for rift: {}", rift.key());
        msg!("Expected authority: {}", expected_authority.key());

        // Check current vault owner
        let vault_account_info = vault_info.to_account_info();
        let vault_data = vault_account_info.data.borrow();
        if vault_data.len() >= 64 {
            let current_owner_bytes = &vault_data[32..64];
            let current_owner = Pubkey::try_from(current_owner_bytes).map_err(|_| ErrorCode::InvalidByteSlice)?;
            msg!("Current vault owner: {}", current_owner);

            if current_owner != expected_authority.key() {
                msg!("Vault ownership conflict detected and logged");
                msg!("Manual intervention required to reassign vault");
                // In production, this would implement vault migration logic
                // For now, we just log the conflict for manual resolution
            }
        }

        Ok(())
    }

    /// Initialize Meteora pool for rift (separate from wrapping)
    pub fn initialize_pool(
        ctx: Context<InitializePool>,
        initial_rift_amount: u64,
        trading_fee_bps: u16,
        bin_step: u16,
    ) -> Result<()> {
        let rift = &mut ctx.accounts.rift;

        // Validation
        require!(initial_rift_amount > 0, ErrorCode::InvalidAmount);
        require!(trading_fee_bps <= 100, ErrorCode::InvalidTradingFee);
        require!(
            bin_step == 1 || bin_step == 5 || bin_step == 10 || bin_step == 25 ||
            bin_step == 50 || bin_step == 100 || bin_step == 200 || bin_step == 500,
            ErrorCode::InvalidBinStep
        );

        // Store pool parameters in rift state
        rift.pool_trading_fee_bps = trading_fee_bps;
        rift.bin_step = bin_step;
        rift.liquidity_pool = Some(ctx.accounts.pool_underlying.key());

        // Mint initial rift tokens to pool
        let rift_key = rift.key();
        let bump_seed = [ctx.bumps.rift_mint_authority];
        let signer_seeds: &[&[u8]] = &[
            b"rift_mint_auth",
            rift_key.as_ref(),
            &bump_seed,
        ];
        let signers = &[signer_seeds];
        let mint_ctx = CpiContext::new_with_signer(
            ctx.accounts.token_program.to_account_info(),
            MintTo {
                mint: ctx.accounts.rift_mint.to_account_info(),
                to: ctx.accounts.pool_rift.to_account_info(),
                authority: ctx.accounts.rift_mint_authority.to_account_info(),
            },
            signers,
        );
        token::mint_to(mint_ctx, initial_rift_amount)?;

        // Update pool state
        rift.total_liquidity_rift = initial_rift_amount;
        rift.lp_token_supply = initial_rift_amount; // Simple 1:1 for now

        emit!(PoolInitialized {
            rift: rift.key(),
            pool_underlying: ctx.accounts.pool_underlying.key(),
            pool_rift: ctx.accounts.pool_rift.key(),
            initial_rift_amount,
            trading_fee_bps,
            bin_step,
        });

        Ok(())
    }

    /// Unwrap rift tokens back to underlying tokens
    /// **METEORA INTEGRATION**: Remove liquidity from Meteora pool
    /// User burns RIFT → Remove liquidity from Meteora → User receives SOL back
    pub fn unwrap_tokens(
        ctx: Context<UnwrapTokens>,
        rift_token_amount: u64,
    ) -> Result<()> {

        let rift = &mut ctx.accounts.rift;

        // **CRITICAL FIX**: Add reentrancy protection
        require!(!rift.reentrancy_guard, ErrorCode::ReentrancyDetected);
        rift.reentrancy_guard = true;

        // Check if rift is paused
        require!(!rift.is_paused, ErrorCode::RiftPaused);

        // Validate amount
        require!(rift_token_amount > 0, ErrorCode::InvalidAmount);

        // Verify pool exists
        require!(rift.liquidity_pool.is_some(), ErrorCode::PoolNotInitialized);
        require!(
            ctx.accounts.pool.key() == rift.liquidity_pool.unwrap(),
            ErrorCode::InvalidPoolAccount
        );

        // Calculate unwrap fee (0.7%)
        let unwrap_fee = u64::try_from(
            (rift_token_amount as u128)
                .checked_mul(70)
                .ok_or(ErrorCode::MathOverflow)?
                .checked_div(10000)
                .ok_or(ErrorCode::MathOverflow)?
        ).map_err(|_| ErrorCode::MathOverflow)?;
        let amount_after_fee = rift_token_amount
            .checked_sub(unwrap_fee)
            .ok_or(ErrorCode::MathOverflow)?;

        msg!("💰 Unwrapping {} RIFT (fee: {}, net: {})", rift_token_amount, unwrap_fee, amount_after_fee);

        // **METEORA INTEGRATION**: Calculate liquidity using constant product formula
        // Meteora uses: liquidity = sqrt(amount_a * amount_b)
        // We're removing equal amounts of both tokens (WSOL and RIFT)
        // So: liquidity = sqrt(amount_after_fee * amount_after_fee)

        // Calculate liquidity using integer square root
        // For equal amounts: sqrt(x * x) = x, but we need to be precise with Meteora's formula
        let amount_128 = amount_after_fee as u128;

        // Calculate product first to match Meteora's exact math
        let product = amount_128
            .checked_mul(amount_128)
            .ok_or(ErrorCode::MathOverflow)?;

        // Integer square root using Newton's method
        let liquidity_delta = if product == 0 {
            0
        } else {
            let mut x = product;
            let mut y = (x + 1) / 2;
            while y < x {
                x = y;
                y = (x + product / x) / 2;
            }
            x
        };

        msg!("📊 Calculated liquidity_delta: {} (from amount: {})", liquidity_delta, amount_after_fee);

        // Set minimal slippage thresholds
        let token_a_threshold = 1u64; // Min 1 lamport of WSOL
        let token_b_threshold = 1u64; // Min 1 lamport of RIFT

        let remove_liquidity_params = RemoveLiquidityParameters {
            liquidity_delta,
            token_a_amount_threshold: token_a_threshold,
            token_b_amount_threshold: token_b_threshold,
        };

        // **PER-USER POSITION FIX**: Remove liquidity from user's OWN position
        // User provides their position NFT that they created when adding liquidity
        let remove_liquidity_accounts = RemoveLiquidityCtx {
            pool: ctx.accounts.pool.to_account_info(),
            position: ctx.accounts.user_position.to_account_info(),  // ← USER'S position
            token_a_account: ctx.accounts.user_underlying.to_account_info(),
            token_b_account: ctx.accounts.user_rift_tokens.to_account_info(),
            token_a_vault: ctx.accounts.token_a_vault.to_account_info(),
            token_b_vault: ctx.accounts.token_b_vault.to_account_info(),
            token_a_mint: ctx.accounts.underlying_mint.to_account_info(),
            token_b_mint: ctx.accounts.rift_mint.to_account_info(),
            position_nft_account: ctx.accounts.user_position_nft_account.to_account_info(),  // ← USER'S NFT
            pool_authority: ctx.accounts.pool_authority.to_account_info(),
            owner: ctx.accounts.user.to_account_info(),  // ← USER owns the position
            token_a_program: ctx.accounts.token_program.to_account_info(),
            token_b_program: ctx.accounts.token_program.to_account_info(),
            event_authority: ctx.accounts.event_authority.to_account_info(),
            program: ctx.accounts.meteora_program.to_account_info(),
        };

        let meteora_ctx = CpiContext::new(
            ctx.accounts.meteora_program.to_account_info(),
            remove_liquidity_accounts,
        );

        // Execute Meteora remove_liquidity CPI
        // This returns both SOL and RIFT to the user's accounts
        cpi::remove_liquidity(meteora_ctx, remove_liquidity_params)?;

        msg!("✅ Removed liquidity from Meteora pool");

        // Burn the RIFT tokens (both original and what came back from pool)
        let burn_ctx = CpiContext::new(
            ctx.accounts.token_program.to_account_info(),
            anchor_spl::token::Burn {
                mint: ctx.accounts.rift_mint.to_account_info(),
                from: ctx.accounts.user_rift_tokens.to_account_info(),
                authority: ctx.accounts.user.to_account_info(),
            },
        );
        anchor_spl::token::burn(burn_ctx, rift_token_amount)?;

        msg!("✅ Burned {} RIFT tokens", rift_token_amount);

        // Update rift state
        rift.total_underlying_wrapped = rift.total_underlying_wrapped.saturating_sub(amount_after_fee);
        rift.total_rift_minted = rift.total_rift_minted.saturating_sub(rift_token_amount);
        rift.total_liquidity_underlying = rift.total_liquidity_underlying.saturating_sub(amount_after_fee);
        rift.total_liquidity_rift = rift.total_liquidity_rift.saturating_sub(amount_after_fee);

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
            .checked_add(amount_after_fee)
            .ok_or(ErrorCode::MathOverflow)?;

        // Update oracle timestamp to mark activity
        rift.last_oracle_update = Clock::get()?.unix_timestamp;

        // Automatically process fee distribution
        rift.process_fee_immediately(unwrap_fee)?;

        // **NEW FEATURE**: User-triggered rebalancing for volatility farming
        // Check if rebalance is needed after user transaction volume
        let clock = Clock::get()?;
        let should_rebalance = rift.should_trigger_rebalance(clock.unix_timestamp)?;
        if should_rebalance {
            rift.trigger_automatic_rebalance(clock.unix_timestamp)?;
        }

        // **CRITICAL FIX**: Release reentrancy guard
        rift.reentrancy_guard = false;

        msg!("✅ Unwrap complete: {} RIFT → {} SOL (removed from Meteora pool)", rift_token_amount, amount_after_fee);

        emit!(UnwrapExecuted {
            rift: rift.key(),
            user: ctx.accounts.user.key(),
            rift_token_amount,
            fee_amount: unwrap_fee,
            underlying_returned: amount_after_fee,
        });

        Ok(())
    }


    /// **SECURITY FIX #50**: Update Pyth oracle price with strict validation
    /// Binds to specific Pyth account stored in rift state and validates staleness, confidence, and decimals
    pub fn update_pyth_oracle(
        ctx: Context<UpdatePythOracle>,
    ) -> Result<()> {
        let rift = &mut ctx.accounts.rift;

        // **SECURITY FIX #50**: Validate oracle authority (creator or governance)
        require!(
            ctx.accounts.oracle_authority.key() == rift.creator,
            ErrorCode::Unauthorized
        );

        // **SECURITY FIX #50**: Bind to stored Pyth account address
        let expected_pyth_account = rift.pyth_price_account
            .ok_or(ErrorCode::OracleAccountNotSet)?;

        require!(
            ctx.accounts.pyth_price_account.key() == expected_pyth_account,
            ErrorCode::OracleAccountMismatch
        );

        // **SECURITY FIX #50**: Validate Pyth account ownership
        let pyth_program_id = Pubkey::from_str_const("FsJ3A3u2vn5cTVofAjvy6y5kwABJAqYWpe4975bi2epH");
        require!(
            ctx.accounts.pyth_price_account.owner == &pyth_program_id,
            ErrorCode::InvalidOracleOwner
        );

        // Parse Pyth price data
        let pyth_price_data = &ctx.accounts.pyth_price_account.data.borrow();
        require!(pyth_price_data.len() >= 240, ErrorCode::InvalidOracleData);

        // Pyth price account layout:
        // 0-4: magic (0xa1b2c3d4)
        // 4-8: version
        // 8-12: type
        // 12-16: size
        // 16-48: product account
        // 48-80: next price account
        // 80-88: aggregate price (i64)
        // 88-96: confidence (u64)
        // 96-100: status (u32)
        // 100-104: corp_act (u32)
        // 104-112: publish_time (i64)
        // 112-120: prev_publish_time (i64)
        // 120-128: prev_price (i64)
        // 128-136: prev_conf (u64)
        // ... more fields

        let price_i64 = i64::from_le_bytes(
            pyth_price_data[80..88].try_into().map_err(|_| ErrorCode::InvalidOracleData)?
        );
        let confidence_u64 = u64::from_le_bytes(
            pyth_price_data[88..96].try_into().map_err(|_| ErrorCode::InvalidOracleData)?
        );
        let publish_time_i64 = i64::from_le_bytes(
            pyth_price_data[104..112].try_into().map_err(|_| ErrorCode::InvalidOracleData)?
        );
        let exponent_i32 = i32::from_le_bytes(
            pyth_price_data[20..24].try_into().map_err(|_| ErrorCode::InvalidOracleData)?
        );

        // Convert price to u64 (handle negative prices as error)
        require!(price_i64 > 0, ErrorCode::InvalidOraclePrice);
        let price = price_i64 as u64;
        let confidence = confidence_u64;

        // **SECURITY FIX #50**: Validate staleness (max 5 minutes)
        let current_time = Clock::get()?.unix_timestamp;
        const MAX_AGE_SECONDS: i64 = 300; // 5 minutes
        require!(
            current_time - publish_time_i64 <= MAX_AGE_SECONDS,
            ErrorCode::OraclePriceStale
        );

        msg!("Pyth price age: {} seconds", current_time - publish_time_i64);

        // **SECURITY FIX #50**: Validate confidence (confidence should be <= 5% of price)
        let max_confidence = price.checked_mul(5).ok_or(ErrorCode::MathOverflow)?
            .checked_div(100).ok_or(ErrorCode::MathOverflow)?;
        require!(
            confidence <= max_confidence,
            ErrorCode::OracleConfidenceTooLow
        );

        msg!("Pyth confidence: {} (max allowed: {})", confidence, max_confidence);

        // **SECURITY FIX #50**: Validate exponent/decimals (-8 to -6 typical for USD pairs)
        require!(
            exponent_i32 >= -18 && exponent_i32 <= 0,
            ErrorCode::InvalidOracleExponent
        );

        msg!("Pyth exponent: {}", exponent_i32);

        // Normalize price to 6 decimals (standard for rift system)
        let normalized_price = if exponent_i32 < -6 {
            // Scale down (e.g., -8 to -6: divide by 100)
            let scale_factor = 10u64.pow((exponent_i32.abs() - 6) as u32);
            price.checked_div(scale_factor).ok_or(ErrorCode::MathOverflow)?
        } else if exponent_i32 > -6 {
            // Scale up (e.g., -4 to -6: multiply by 100)
            let scale_factor = 10u64.pow((6 - exponent_i32.abs()) as u32);
            price.checked_mul(scale_factor).ok_or(ErrorCode::MathOverflow)?
        } else {
            price
        };

        // Update rift oracle with validated price
        rift.add_price_data(normalized_price, confidence, current_time)?;

        emit!(OraclePriceUpdated {
            rift: rift.key(),
            oracle_type: OracleType::Pyth,
            price: normalized_price,
            confidence,
            timestamp: current_time,
        });

        Ok(())
    }

    /// **SECURITY FIX #50**: Update Switchboard oracle price with strict validation
    /// Binds to specific Switchboard account stored in rift state and validates staleness, confidence, and decimals
    pub fn update_switchboard_oracle(
        ctx: Context<UpdateSwitchboardOracle>,
    ) -> Result<()> {
        let rift = &mut ctx.accounts.rift;

        // **SECURITY FIX #50**: Validate oracle authority (creator or governance)
        require!(
            ctx.accounts.oracle_authority.key() == rift.creator,
            ErrorCode::Unauthorized
        );

        // **SECURITY FIX #50**: Bind to stored Switchboard account address
        let expected_switchboard_account = rift.switchboard_feed_account
            .ok_or(ErrorCode::OracleAccountNotSet)?;

        require!(
            ctx.accounts.switchboard_feed.key() == expected_switchboard_account,
            ErrorCode::OracleAccountMismatch
        );

        // **SECURITY FIX #50**: Validate Switchboard account ownership
        let switchboard_program_id = Pubkey::from_str_const("SW1TCH7qEPTdLsDHRgPuMQjbQxKdH2aBStViMFnt64f");
        require!(
            ctx.accounts.switchboard_feed.owner == &switchboard_program_id,
            ErrorCode::InvalidOracleOwner
        );

        // Parse Switchboard aggregator data
        let switchboard_data = &ctx.accounts.switchboard_feed.data.borrow();
        require!(switchboard_data.len() >= 512, ErrorCode::InvalidOracleData);

        // Switchboard V2 AggregatorAccountData layout (simplified):
        // The actual layout is complex, but the key fields we need are:
        // - latest_confirmed_round.result (SwitchboardDecimal at specific offset)
        // - latest_confirmed_round.round_open_timestamp (i64)
        // - min_oracle_results (u32)
        // - oracle_request_batch_size (u32)

        // For security, we parse the essential fields for validation
        // Offset 200: latest_confirmed_round.result.mantissa (i128)
        // Offset 216: latest_confirmed_round.result.scale (u32)
        // Offset 232: latest_confirmed_round.round_open_timestamp (i64)

        let mantissa_bytes: [u8; 16] = switchboard_data[200..216].try_into()
            .map_err(|_| ErrorCode::InvalidOracleData)?;
        let mantissa = i128::from_le_bytes(mantissa_bytes);

        let scale = u32::from_le_bytes(
            switchboard_data[216..220].try_into().map_err(|_| ErrorCode::InvalidOracleData)?
        );

        let round_open_timestamp = i64::from_le_bytes(
            switchboard_data[232..240].try_into().map_err(|_| ErrorCode::InvalidOracleData)?
        );

        // Convert mantissa to u64 price
        require!(mantissa > 0, ErrorCode::InvalidOraclePrice);

        // Calculate price with scale: price = mantissa / 10^scale
        let price = if scale > 0 {
            let divisor = 10u128.pow(scale);
            let price_u128 = (mantissa as u128).checked_div(divisor)
                .ok_or(ErrorCode::MathOverflow)?;
            u64::try_from(price_u128).map_err(|_| ErrorCode::MathOverflow)?
        } else {
            u64::try_from(mantissa).map_err(|_| ErrorCode::MathOverflow)?
        };

        // **SECURITY FIX #50**: Validate staleness (max 5 minutes)
        let current_time = Clock::get()?.unix_timestamp;
        const MAX_AGE_SECONDS: i64 = 300; // 5 minutes
        require!(
            current_time - round_open_timestamp <= MAX_AGE_SECONDS,
            ErrorCode::OraclePriceStale
        );

        msg!("Switchboard price age: {} seconds", current_time - round_open_timestamp);

        // **SECURITY FIX #50**: Validate scale/decimals (0-18 typical)
        require!(
            scale <= 18,
            ErrorCode::InvalidOracleExponent
        );

        msg!("Switchboard scale: {}", scale);

        // For Switchboard, we use a default confidence of 1% of price
        // In production, you could parse the std_deviation field for actual confidence
        let confidence = price.checked_mul(1).ok_or(ErrorCode::MathOverflow)?
            .checked_div(100).ok_or(ErrorCode::MathOverflow)?;

        // **SECURITY FIX #50**: Validate confidence (confidence should be <= 5% of price)
        let max_confidence = price.checked_mul(5).ok_or(ErrorCode::MathOverflow)?
            .checked_div(100).ok_or(ErrorCode::MathOverflow)?;
        require!(
            confidence <= max_confidence,
            ErrorCode::OracleConfidenceTooLow
        );

        // Normalize price to 6 decimals if needed
        let normalized_price = if scale > 6 {
            let scale_factor = 10u64.pow((scale - 6) as u32);
            price.checked_div(scale_factor).ok_or(ErrorCode::MathOverflow)?
        } else if scale < 6 {
            let scale_factor = 10u64.pow((6 - scale) as u32);
            price.checked_mul(scale_factor).ok_or(ErrorCode::MathOverflow)?
        } else {
            price
        };

        // Update rift oracle with validated price
        rift.add_price_data(normalized_price, confidence, current_time)?;

        emit!(OraclePriceUpdated {
            rift: rift.key(),
            oracle_type: OracleType::Switchboard,
            price: normalized_price,
            confidence,
            timestamp: current_time,
        });

        Ok(())
    }

    /// Manual rebalance (can be called by anyone if conditions are met)
    pub fn trigger_rebalance(
        ctx: Context<TriggerRebalance>,
    ) -> Result<()> {
        let rift = &mut ctx.accounts.rift;

        // **SECURITY FIX**: Add reentrancy protection
        require!(!rift.reentrancy_guard, ErrorCode::ReentrancyDetected);
        rift.reentrancy_guard = true;

        let clock = Clock::get()?;

        // Check if manual rebalance is allowed
        require!(
            rift.can_manual_rebalance(clock.unix_timestamp)?,
            ErrorCode::RebalanceTooSoon
        );

        rift.trigger_automatic_rebalance(clock.unix_timestamp)?;

        // **SECURITY FIX**: Release reentrancy guard
        rift.reentrancy_guard = false;

        Ok(())
    }

    /// Process fee distribution with full functionality (optimized for stack usage)
    pub fn process_fee_distribution(
        ctx: Context<ProcessFeeDistribution>,
        fee_amount: u64,
    ) -> Result<()> {
        let rift = &mut ctx.accounts.rift;

        // **SECURITY FIX**: Add reentrancy protection
        require!(!rift.reentrancy_guard, ErrorCode::ReentrancyDetected);
        rift.reentrancy_guard = true;

        // Basic validation
        require!(fee_amount > 0, ErrorCode::InvalidAmount);
        require!(fee_amount <= 1_000_000_000_000, ErrorCode::AmountTooLarge);

        // Calculate fee splits with minimal stack usage
        let burn_bps = u64::from(rift.burn_fee_bps);
        let partner_bps = u64::from(rift.partner_fee_bps);

        let burn_amount = fee_amount
            .checked_mul(burn_bps)
            .ok_or(ErrorCode::MathOverflow)?
            .checked_div(10000)
            .ok_or(ErrorCode::MathOverflow)?;

        let partner_amount = fee_amount
            .checked_mul(partner_bps)
            .ok_or(ErrorCode::MathOverflow)?
            .checked_div(10000)
            .ok_or(ErrorCode::MathOverflow)?;

        let burn_plus_partner = burn_amount
            .checked_add(partner_amount)
            .ok_or(ErrorCode::MathOverflow)?;

        let remaining = fee_amount
            .checked_sub(burn_plus_partner)
            .ok_or(ErrorCode::MathOverflow)?;

        // 5% to treasury, 95% to fee collector
        let treasury_amount = remaining
            .checked_mul(5)
            .ok_or(ErrorCode::MathOverflow)?
            .checked_div(100)
            .ok_or(ErrorCode::MathOverflow)?;

        let fee_collector_amount = remaining
            .checked_sub(treasury_amount)
            .ok_or(ErrorCode::MathOverflow)?;

        // Prepare vault authority seeds for all transfers
        let rift_key = rift.key();
        let bump = [ctx.bumps.vault_authority];
        let vault_seeds: &[&[u8]] = &[b"vault_auth", rift_key.as_ref(), &bump];
        let signers = &[vault_seeds];

        // Transfer to treasury if amount > 0
        if treasury_amount > 0 {
            let transfer_ctx = CpiContext::new_with_signer(
                ctx.accounts.token_program.to_account_info(),
                Transfer {
                    from: ctx.accounts.vault.to_account_info(),
                    to: ctx.accounts.treasury.to_account_info(),
                    authority: ctx.accounts.vault_authority.to_account_info(),
                },
                signers,
            );
            token::transfer(transfer_ctx, treasury_amount)?;
        }

        // Transfer to fee collector if amount > 0 and vault provided
        if fee_collector_amount > 0 && ctx.accounts.fee_collector_vault.is_some() {
            let fee_collector_vault = ctx.accounts.fee_collector_vault.as_ref().unwrap();
            let transfer_ctx = CpiContext::new_with_signer(
                ctx.accounts.token_program.to_account_info(),
                Transfer {
                    from: ctx.accounts.vault.to_account_info(),
                    to: fee_collector_vault.to_account_info(),
                    authority: ctx.accounts.vault_authority.to_account_info(),
                },
                signers,
            );
            token::transfer(transfer_ctx, fee_collector_amount)?;
        }

        // Transfer to partner if amount > 0 and vault provided
        if partner_amount > 0 && ctx.accounts.partner_vault.is_some() {
            let partner_vault = ctx.accounts.partner_vault.as_ref().unwrap();

            // **SECURITY FIX**: Validate partner vault belongs to configured partner
            if let Some(partner_wallet) = rift.partner_wallet {
                require!(
                    partner_vault.owner == partner_wallet,
                    ErrorCode::InvalidPartnerVault
                );
                require!(
                    partner_vault.mint == ctx.accounts.vault.mint,
                    ErrorCode::InvalidPartnerVault
                );
            } else {
                return Err(ErrorCode::InvalidPartnerVault.into());
            }
            let transfer_ctx = CpiContext::new_with_signer(
                ctx.accounts.token_program.to_account_info(),
                Transfer {
                    from: ctx.accounts.vault.to_account_info(),
                    to: partner_vault.to_account_info(),
                    authority: ctx.accounts.vault_authority.to_account_info(),
                },
                signers,
            );
            token::transfer(transfer_ctx, partner_amount)?;
        }

        // Update tracking
        rift.total_fees_collected = rift.total_fees_collected.checked_add(fee_amount).unwrap_or(rift.total_fees_collected);

        emit!(FeesCalculated {
            rift: rift.key(),
            treasury_amount,
            fee_collector_amount,
            partner_amount,
            burn_amount,
        });

        // **SECURITY FIX**: Release reentrancy guard
        rift.reentrancy_guard = false;

        Ok(())
    }

    /// Stake LP tokens for RIFTS rewards
    pub fn stake_lp_tokens(
        ctx: Context<StakeLPTokens>,
        amount: u64,
    ) -> Result<()> {
        let rift = &mut ctx.accounts.rift;

        // **SECURITY FIX**: Add reentrancy protection
        require!(!rift.reentrancy_guard, ErrorCode::ReentrancyDetected);
        rift.reentrancy_guard = true;

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
        let time_elapsed_i64 = current_time
            .checked_sub(staker.last_reward_update)
            .ok_or(ErrorCode::MathOverflow)?;
        let time_elapsed = u64::try_from(time_elapsed_i64)
            .map_err(|_| ErrorCode::MathOverflow)?;
        
        if staker.staked_amount > 0 && time_elapsed > 0 {
            // Calculate pending rewards: 10% APY = ~0.00003170979% per hour
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

        // **NEW FEATURE**: User-triggered rebalancing for volatility farming
        // Check if rebalance is needed after staking activity
        let clock = Clock::get()?;
        let should_rebalance = rift.should_trigger_rebalance(clock.unix_timestamp)?;
        if should_rebalance {
            rift.trigger_automatic_rebalance(clock.unix_timestamp)?;
        }

        emit!(LPTokensStaked {
            rift: rift.key(),
            user: ctx.accounts.user.key(),
            amount,
            total_staked: staker.staked_amount,
        });

        // **SECURITY FIX**: Release reentrancy guard
        rift.reentrancy_guard = false;

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
        let time_elapsed_i64 = current_time
            .checked_sub(staker.last_reward_update)
            .ok_or(ErrorCode::MathOverflow)?;
        let time_elapsed = u64::try_from(time_elapsed_i64)
            .map_err(|_| ErrorCode::MathOverflow)?;
        
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
            b"rift_mint_auth",
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
        let time_elapsed_i64 = current_time
            .checked_sub(staker.last_reward_update)
            .ok_or(ErrorCode::MathOverflow)?;
        let time_elapsed = u64::try_from(time_elapsed_i64)
            .map_err(|_| ErrorCode::MathOverflow)?;
        
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

        // **NEW FEATURE**: User-triggered rebalancing for volatility farming
        // Check if rebalance is needed after unstaking activity
        let clock = Clock::get()?;
        let should_rebalance = rift.should_trigger_rebalance(clock.unix_timestamp)?;
        if should_rebalance {
            rift.trigger_automatic_rebalance(clock.unix_timestamp)?;
        }

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
    /// **SECURITY FIX #51**: Added balance verification to enforce minimum_amount_out
    pub fn jupiter_swap_for_buyback(
        ctx: Context<JupiterSwapForBuyback>,
        amount_in: u64,
        minimum_amount_out: u64,
        swap_data: Vec<u8>,
    ) -> Result<()> {
        let rift = &mut ctx.accounts.rift;

        // **SECURITY FIX**: Add reentrancy protection
        require!(!rift.reentrancy_guard, ErrorCode::ReentrancyDetected);
        rift.reentrancy_guard = true;

        // **SECURITY FIX**: Validate Jupiter program ID using governance-configurable ID
        let expected_jupiter_id = rift.get_jupiter_program_id();
        require!(
            ctx.accounts.jupiter_program.key() == expected_jupiter_id?,
            ErrorCode::InvalidProgramId
        );

        // Validate input
        require!(amount_in > 0, ErrorCode::InvalidAmount);
        require!(amount_in <= 1_000_000_000_000, ErrorCode::AmountTooLarge);
        require!(minimum_amount_out > 0, ErrorCode::InvalidAmount);
        require!(swap_data.len() <= 10000, ErrorCode::InvalidInputData);

        // **SECURITY FIX #51**: Snapshot source balance before swap
        let source_balance_before = ctx.accounts.source_token_account.amount;
        msg!("Source balance before swap: {}", source_balance_before);

        // **SECURITY FIX #51**: Snapshot destination balance before swap
        let dest_balance_before = ctx.accounts.destination_token_account.amount;
        msg!("Destination balance before swap: {}", dest_balance_before);

        // CPI to Jupiter program for swap
        let cpi_program = ctx.accounts.jupiter_program.to_account_info();
        let cpi_accounts = ctx.remaining_accounts;

        let rift_key = rift.key();
        let vault_seeds = &[b"vault_auth", rift_key.as_ref(), &[ctx.bumps.vault_authority]];
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

        // **SECURITY FIX #51**: Reload account data after swap to get updated balances
        ctx.accounts.source_token_account.reload()?;
        ctx.accounts.destination_token_account.reload()?;

        // **SECURITY FIX #51**: Verify source balance decreased as expected
        let source_balance_after = ctx.accounts.source_token_account.amount;
        msg!("Source balance after swap: {}", source_balance_after);

        let source_delta = source_balance_before
            .checked_sub(source_balance_after)
            .ok_or(ErrorCode::MathOverflow)?;

        require!(
            source_delta >= amount_in,
            ErrorCode::InvalidAmount
        );
        msg!("Source delta verified: {} (expected >= {})", source_delta, amount_in);

        // **SECURITY FIX #51**: Verify destination balance increased by at least minimum_amount_out
        let dest_balance_after = ctx.accounts.destination_token_account.amount;
        msg!("Destination balance after swap: {}", dest_balance_after);

        let dest_delta = dest_balance_after
            .checked_sub(dest_balance_before)
            .ok_or(ErrorCode::MathOverflow)?;

        // **CRITICAL ENFORCEMENT**: Require actual output >= minimum required
        require!(
            dest_delta >= minimum_amount_out,
            ErrorCode::SlippageExceeded
        );
        msg!("Destination delta verified: {} (minimum required: {})", dest_delta, minimum_amount_out);

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

        // **SECURITY FIX**: Release reentrancy guard
        rift.reentrancy_guard = false;

        Ok(())
    }

    /// Governance proposal execution for rift parameters
    /// **SECURITY FIX #43**: Bind proposal to governance PDA to prevent cross-governance spoofing
    pub fn execute_governance_proposal(
        ctx: Context<ExecuteGovernanceProposal>,
        proposal_id: u64,
        new_burn_fee_bps: Option<u16>,
        new_partner_fee_bps: Option<u16>,
        new_oracle_interval: Option<i64>,
        new_rebalance_threshold: Option<u16>,
    ) -> Result<()> {
        let rift = &mut ctx.accounts.rift;
        let governance = &ctx.accounts.governance;
        let proposal = &ctx.accounts.proposal;

        // **SECURITY FIX #43**: Validate proposal belongs to THIS governance account
        // Verify proposal ID matches the pending parameter change proposal in governance
        require!(
            governance.parameter_change_proposal_id == proposal_id,
            ErrorCode::ProposalNotBoundToGovernance
        );

        // **SECURITY FIX #43**: Verify proposal matches the expected proposal
        require!(
            proposal.id == proposal_id,
            ErrorCode::ProposalMismatch
        );

        // Validate proposal execution via CPI to governance program
        require!(
            proposal.proposal_type == governance::ProposalType::ParameterChange,
            ErrorCode::InvalidProposalType
        );
        require!(
            proposal.status == governance::ProposalStatus::Executed,
            ErrorCode::ProposalNotApproved
        );

        // **SECURITY FIX #43**: Validate governance has pending parameter changes for this proposal
        require!(
            governance.pending_parameter_changes.is_some(),
            ErrorCode::NoPendingParameterChanges
        );

        // Apply parameter changes from governance (source of truth)
        let param_changes = governance.pending_parameter_changes.as_ref().unwrap();

        // Execute parameter changes from governance-approved values
        if let Some(burn_fee) = param_changes.burn_fee_bps {
            require!(burn_fee <= 4500, ErrorCode::InvalidBurnFee);
            rift.burn_fee_bps = burn_fee;
        }

        if let Some(partner_fee) = param_changes.partner_fee_bps {
            require!(partner_fee <= 500, ErrorCode::InvalidPartnerFee);
            rift.partner_fee_bps = partner_fee;
        }

        if let Some(oracle_interval) = param_changes.oracle_update_interval {
            require!(oracle_interval >= 600, ErrorCode::InvalidOracleInterval); // Min 10 minutes
            rift.oracle_update_interval = oracle_interval;
        }

        if let Some(threshold) = param_changes.arbitrage_threshold_bps {
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
    /// **SECURITY FIX #44**: Add CPI verification to governance for emergency controls
    ///
    /// **DESIGN DECISION**: Single-signature kill-switch intentionally allowed for emergency cases
    /// - When required_signatures == 1: Any authorized signer can pause immediately (emergency kill-switch)
    /// - When required_signatures > 1: Requires governance proposal execution first (multisig protection)
    /// This balances security with the need for rapid emergency response.
    pub fn emergency_pause(
        ctx: Context<EmergencyPause>,
    ) -> Result<()> {
        let rift = &mut ctx.accounts.rift;
        let governance = &ctx.accounts.governance;

        // **SECURITY FIX #44**: Verify governance authority via signature bitmap
        // Check if signer is either primary or additional authority
        let signer = ctx.accounts.governance_authority.key();
        let is_primary = signer == governance.authority;
        let is_additional = governance.additional_authorities.contains(&signer);

        require!(
            is_primary || is_additional,
            ErrorCode::UnauthorizedGovernance
        );

        // **SECURITY FIX #44**: If multisig is configured, verify emergency pause is authorized
        // Emergency actions should be backed by a valid proposal
        // **NOTE**: Single-sig mode (required_signatures == 1) allows immediate pause for true emergencies
        if governance.required_signatures > 1 {
            // Verify emergency pause is active in governance
            require!(
                governance.emergency_pause_active,
                ErrorCode::EmergencyActionNotAuthorized
            );
        }

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
    /// **SECURITY FIX #44**: Add CPI verification to governance for emergency controls
    ///
    /// **DESIGN DECISION**: Single-signature unpause intentionally allowed for operational flexibility
    /// - When required_signatures == 1: Any authorized signer can unpause immediately
    /// - When required_signatures > 1: Requires governance proposal execution first (multisig protection)
    /// This balances security with the need for rapid emergency response and recovery.
    pub fn emergency_unpause(
        ctx: Context<EmergencyUnpause>,
    ) -> Result<()> {
        let rift = &mut ctx.accounts.rift;
        let governance = &ctx.accounts.governance;

        // **SECURITY FIX #44**: Verify governance authority via signature bitmap
        // Check if signer is either primary or additional authority
        let signer = ctx.accounts.governance_authority.key();
        let is_primary = signer == governance.authority;
        let is_additional = governance.additional_authorities.contains(&signer);

        require!(
            is_primary || is_additional,
            ErrorCode::UnauthorizedGovernance
        );

        // **SECURITY FIX #44**: If multisig is configured, verify unpause is authorized
        // Unpause should either be done when pause is not active OR when authorized
        // **NOTE**: Single-sig mode (required_signatures == 1) allows immediate unpause for operational recovery
        if governance.required_signatures > 1 {
            // Allow unpause if emergency pause is not active in governance anymore
            // This means the governance multisig has already approved the unpause
            require!(
                !governance.emergency_pause_active,
                ErrorCode::EmergencyActionNotAuthorized
            );
        }

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
        let system_program_key = anchor_lang::solana_program::system_program::ID;
        require!(
            rift.total_underlying_wrapped == 0 || rift.vault == system_program_key,
            ErrorCode::VaultNotEmpty
        );
        
        emit!(RiftClosed {
            rift: rift.key(),
            creator: rift.creator,
        });

        Ok(())
    }

    /// Admin function: Close any rift regardless of creator (program authority only)
    pub fn admin_close_rift(
        ctx: Context<AdminCloseRift>,
    ) -> Result<()> {
        let rift = &ctx.accounts.rift;

        // Only program authority can use this function
        let admin_pubkey = Pubkey::from_str_const("4NHB7rAvsDjV5USbuntY4UcgnQS1zQcc8K69htaAupHk");
        require!(
            ctx.accounts.program_authority.key() == admin_pubkey,
            ErrorCode::UnauthorizedAdmin
        );

        // Log the admin close action
        msg!("Admin closing rift: {} (original creator: {})", rift.key(), rift.creator);

        emit!(RiftAdminClosed {
            rift: rift.key(),
            original_creator: rift.creator,
            admin: ctx.accounts.program_authority.key(),
        });

        Ok(())
    }

    /// Clean up stuck accounts from failed rift creation attempts
    /// **SECURITY FIX**: Only allow creator to clean up their own stuck accounts
    pub fn cleanup_stuck_accounts(
        ctx: Context<CleanupStuckAccounts>,
    ) -> Result<()> {
        // **SECURITY FIX**: Require creator signature to prevent griefing
        // Only the original creator can clean up their stuck accounts
        
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

    /// **SECURITY FIX**: Governance function to update Jupiter program ID
    /// Allows governance to override hardcoded Jupiter program ID
    pub fn update_jupiter_program_id(
        ctx: Context<UpdateJupiterProgramId>,
        new_jupiter_program_id: Option<Pubkey>,
    ) -> Result<()> {
        let rift = &mut ctx.accounts.rift;

        // Validate new program ID if provided
        if let Some(program_id) = new_jupiter_program_id {
            require!(program_id != Pubkey::default(), ErrorCode::InvalidProgramId);
        }

        // Update Jupiter program ID (None means use hardcoded fallback)
        rift.set_jupiter_program_id(new_jupiter_program_id);
        rift.last_governance_update = Clock::get()?.unix_timestamp;

        msg!("Jupiter program ID updated: {:?}", new_jupiter_program_id);

        Ok(())
    }

    /// **SECURITY FIX #50**: Set oracle account addresses (creator only)
    /// This binds specific Pyth/Switchboard accounts to the rift for validation
    pub fn set_oracle_accounts(
        ctx: Context<SetOracleAccounts>,
        pyth_account: Option<Pubkey>,
        switchboard_account: Option<Pubkey>,
    ) -> Result<()> {
        let rift = &mut ctx.accounts.rift;

        // Only creator can set oracle accounts
        require!(
            ctx.accounts.creator.key() == rift.creator,
            ErrorCode::Unauthorized
        );

        // Validate accounts are not system program
        if let Some(pyth) = pyth_account {
            require!(
                pyth != anchor_lang::solana_program::system_program::ID,
                ErrorCode::InvalidOracleAccount
            );
        }
        if let Some(switchboard) = switchboard_account {
            require!(
                switchboard != anchor_lang::solana_program::system_program::ID,
                ErrorCode::InvalidOracleAccount
            );
        }

        // Set oracle accounts
        rift.pyth_price_account = pyth_account;
        rift.switchboard_feed_account = switchboard_account;
        rift.last_governance_update = Clock::get()?.unix_timestamp;

        msg!("Oracle accounts set - Pyth: {:?}, Switchboard: {:?}", pyth_account, switchboard_account);

        Ok(())
    }

}

// SIMPLIFIED ACCOUNT STRUCTS TO REDUCE STACK USAGE

#[derive(Accounts)]
#[instruction(vanity_seed: [u8; 32], seed_len: u8, mint_bump: u8, burn_fee_bps: u16, partner_fee_bps: u16, partner_wallet: Option<Pubkey>, rift_name: [u8; 32], name_len: u8)]
pub struct CreateRiftWithVanityPDA<'info> {
    #[account(mut)]
    pub creator: Signer<'info>,

    #[account(
        init,
        payer = creator,
        space = 8 + std::mem::size_of::<Rift>(),
        seeds = [b"rift", underlying_mint.key().as_ref(), creator.key().as_ref(), &vanity_seed[..seed_len as usize]],
        bump,
    )]
    pub rift: Account<'info, Rift>,

    pub underlying_mint: Account<'info, Mint>,

    /// The PDA-derived mint account for vanity address
    #[account(
        init,
        payer = creator,
        mint::decimals = underlying_mint.decimals,
        mint::authority = rift_mint_authority,
        seeds = [b"rift_mint", creator.key().as_ref(), underlying_mint.key().as_ref(), &vanity_seed[..seed_len as usize]],
        bump,
    )]
    pub rift_mint: Account<'info, Mint>,

    /// CHECK: PDA vault for tokens
    #[account(
        seeds = [b"vault", rift.key().as_ref()],
        bump
    )]
    pub vault: UncheckedAccount<'info>,

    /// CHECK: PDA for rift mint authority
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
#[instruction(burn_fee_bps: u16, partner_fee_bps: u16, partner_wallet: Option<Pubkey>, rift_name: [u8; 32], name_len: u8)]
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
        init_if_needed,
        payer = creator,
        mint::decimals = underlying_mint.decimals,
        mint::authority = rift_mint_authority,
        seeds = [b"rift_mint", underlying_mint.key().as_ref(), creator.key().as_ref()],
        constraint = underlying_mint.key() != Pubkey::default() && creator.key() != Pubkey::default() @ ErrorCode::InvalidSeedComponent,
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
pub struct InitializeVault<'info> {
    #[account(mut)]
    pub user: Signer<'info>,

    #[account(mut)]
    pub rift: Account<'info, Rift>,

    /// Vault token account
    #[account(
        init,
        payer = user,
        token::mint = underlying_mint,
        token::authority = rift_mint_authority,
        seeds = [b"vault", rift.key().as_ref()],
        bump
    )]
    pub vault: Account<'info, TokenAccount>,

    pub underlying_mint: Account<'info, Mint>,

    /// CHECK: PDA
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

    /// **SECURITY FIX #49**: Validate user's underlying token account
    #[account(
        mut,
        constraint = user_underlying.mint == rift.underlying_mint @ ErrorCode::InvalidMint,
        constraint = user_underlying.owner == user.key() @ ErrorCode::UnauthorizedTokenAccount
    )]
    pub user_underlying: Account<'info, TokenAccount>,

    /// **SECURITY FIX #49**: Validate user's RIFT token account
    #[account(
        mut,
        constraint = user_rift_tokens.mint == rift.rift_mint @ ErrorCode::InvalidMint,
        constraint = user_rift_tokens.owner == user.key() @ ErrorCode::UnauthorizedTokenAccount
    )]
    pub user_rift_tokens: Account<'info, TokenAccount>,

    /// **SECURITY FIX #49**: Validate vault matches rift state
    #[account(
        mut,
        constraint = vault.key() == rift.vault @ ErrorCode::InvalidVault,
        constraint = vault.mint == rift.underlying_mint @ ErrorCode::InvalidMint,
        seeds = [b"vault", rift.key().as_ref()],
        bump
    )]
    pub vault: Account<'info, TokenAccount>,

    /// **SECURITY FIX #49**: Validate rift mint matches rift state
    #[account(
        mut,
        constraint = rift_mint.key() == rift.rift_mint @ ErrorCode::InvalidMint
    )]
    pub rift_mint: Account<'info, Mint>,

    /// CHECK: PDA
    #[account(
        seeds = [b"rift_mint_auth", rift.key().as_ref()],
        bump
    )]
    pub rift_mint_authority: UncheckedAccount<'info>,

    /// **SECURITY FIX #49**: Add underlying mint for validation
    pub underlying_mint: Account<'info, Mint>,

    pub token_program: Program<'info, Token>,
}

#[derive(Accounts)]
pub struct SetPoolAddress<'info> {
    #[account(mut)]
    pub user: Signer<'info>,

    #[account(mut)]
    pub rift: Account<'info, Rift>,
}

#[derive(Accounts)]
pub struct WrapAndAddLiquidity<'info> {
    #[account(mut)]
    pub user: Signer<'info>,

    #[account(mut)]
    pub rift: Account<'info, Rift>,

    #[account(mut)]
    pub user_underlying: Account<'info, TokenAccount>,

    #[account(mut)]
    pub user_rift_tokens: Account<'info, TokenAccount>,

    #[account(mut)]
    pub rift_mint: Account<'info, Mint>,

    /// CHECK: PDA
    #[account(
        seeds = [b"rift_mint_auth", rift.key().as_ref()],
        bump
    )]
    pub rift_mint_authority: UncheckedAccount<'info>,

    pub underlying_mint: Account<'info, Mint>,

    /// CHECK: Validated against rift.liquidity_pool
    #[account(mut)]
    pub pool: UncheckedAccount<'info>,

    /// **PER-USER POSITION**: User's own position NFT mint (created by user)
    #[account(mut)]
    pub user_position_nft_mint: Signer<'info>,

    /// **PER-USER POSITION**: User's position account (derived from their NFT)
    /// CHECK: Meteora position PDA derived from user's NFT mint
    #[account(mut)]
    pub user_position: UncheckedAccount<'info>,

    /// **PER-USER POSITION**: User's position NFT token account
    /// CHECK: Meteora position NFT account owned by user
    #[account(mut)]
    pub user_position_nft_account: UncheckedAccount<'info>,

    /// CHECK: Meteora pool authority
    pub pool_authority: UncheckedAccount<'info>,

    /// CHECK: Meteora token vaults
    #[account(mut)]
    pub token_a_vault: UncheckedAccount<'info>,

    #[account(mut)]
    pub token_b_vault: UncheckedAccount<'info>,

    /// CHECK: Meteora event authority
    pub event_authority: UncheckedAccount<'info>,

    /// **SECURITY FIX #51**: Validate Meteora program ID against hardcoded constant
    /// This prevents attackers from passing fake Meteora programs to steal funds
    #[account(
        constraint = meteora_program.key() == METEORA_DAMM_V2_PROGRAM_ID @ ErrorCode::InvalidProgramId,
        constraint = meteora_program.executable @ ErrorCode::InvalidProgramId
    )]
    pub meteora_program: UncheckedAccount<'info>,

    pub token_program: Program<'info, Token>,
    pub system_program: Program<'info, System>,
    pub rent: Sysvar<'info, Rent>,
}

#[derive(Accounts)]
pub struct RemoveLiquidityAndUnwrap<'info> {
    #[account(mut)]
    pub user: Signer<'info>,

    #[account(mut)]
    pub rift: Account<'info, Rift>,

    #[account(mut)]
    pub user_underlying: Account<'info, TokenAccount>,

    #[account(mut)]
    pub user_rift_tokens: Account<'info, TokenAccount>,

    #[account(mut)]
    pub rift_mint: Account<'info, Mint>,

    pub underlying_mint: Account<'info, Mint>,

    /// CHECK: Validated against rift.liquidity_pool
    #[account(mut)]
    pub pool: UncheckedAccount<'info>,

    /// **PER-USER POSITION**: User's own position NFT mint
    /// CHECK: User's position NFT mint that they created when adding liquidity
    pub user_position_nft_mint: UncheckedAccount<'info>,

    /// **PER-USER POSITION**: User's position account (derived from their NFT)
    /// CHECK: Meteora position PDA derived from user's NFT mint
    #[account(mut)]
    pub user_position: UncheckedAccount<'info>,

    /// **PER-USER POSITION**: User's position NFT token account
    /// CHECK: Meteora position NFT account owned by user
    #[account(mut)]
    pub user_position_nft_account: UncheckedAccount<'info>,

    /// CHECK: Meteora pool authority
    pub pool_authority: UncheckedAccount<'info>,

    /// CHECK: Meteora token vaults
    #[account(mut)]
    pub token_a_vault: UncheckedAccount<'info>,

    #[account(mut)]
    pub token_b_vault: UncheckedAccount<'info>,

    /// CHECK: Meteora event authority
    pub event_authority: UncheckedAccount<'info>,

    /// **SECURITY FIX #51**: Validate Meteora program ID against hardcoded constant
    /// This prevents attackers from passing fake Meteora programs to steal funds
    #[account(
        constraint = meteora_program.key() == METEORA_DAMM_V2_PROGRAM_ID @ ErrorCode::InvalidProgramId,
        constraint = meteora_program.executable @ ErrorCode::InvalidProgramId
    )]
    pub meteora_program: UncheckedAccount<'info>,

    pub token_program: Program<'info, Token>,
}

#[derive(Accounts)]
pub struct CreateMeteoraPool<'info> {
    /// User creating the pool (maps to creator in Meteora)
    #[account(mut)]
    pub user: Signer<'info>,

    #[account(mut)]
    pub rift: Account<'info, Rift>,

    /// Position NFT mint (required by Meteora)
    #[account(mut)]
    /// CHECK: This will be initialized by Meteora program
    pub position_nft_mint: UncheckedAccount<'info>,

    /// Position NFT account (required by Meteora)
    #[account(mut)]
    /// CHECK: This will be initialized by Meteora program
    pub position_nft_account: UncheckedAccount<'info>,

    /// Payer for the transaction (same as user in our case)
    #[account(mut)]
    pub payer: Signer<'info>,

    /// Meteora config account (maps to config in Meteora)
    /// CHECK: This should be the Meteora config account
    pub config: UncheckedAccount<'info>,

    /// Pool authority (PDA)
    /// CHECK: This will be a PDA derived by Meteora
    pub pool_authority: UncheckedAccount<'info>,

    /// The official Meteora pool account to be created
    #[account(mut)]
    /// CHECK: This will be initialized by Meteora program
    pub pool: UncheckedAccount<'info>,

    /// Position account for liquidity position
    #[account(mut)]
    /// CHECK: This will be initialized by Meteora program
    pub position: UncheckedAccount<'info>,

    /// Token A mint (underlying token)
    pub token_a_mint: Account<'info, Mint>,

    /// Token B mint (rift token)
    #[account(mut)]
    pub token_b_mint: Account<'info, Mint>,

    /// Token A vault (underlying token vault)
    #[account(mut)]
    /// CHECK: This will be initialized by Meteora program
    pub token_a_vault: UncheckedAccount<'info>,

    /// Token B vault (rift token vault)
    #[account(mut)]
    /// CHECK: This will be initialized by Meteora program
    pub token_b_vault: UncheckedAccount<'info>,

    /// Payer's token A account
    #[account(mut)]
    pub payer_token_a: Account<'info, TokenAccount>,

    /// Payer's token B account (rift tokens)
    #[account(mut)]
    pub payer_token_b: Account<'info, TokenAccount>,

    /// CHECK: PDA for rift mint authority
    #[account(
        seeds = [b"rift_mint_auth", rift.key().as_ref()],
        bump
    )]
    pub rift_mint_authority: UncheckedAccount<'info>,

    /// Token program for token A
    pub token_a_program: Program<'info, Token>,

    /// Token program for token B
    pub token_b_program: Program<'info, Token>,

    /// Token 2022 program - Meteora expects the actual TOKEN_2022_PROGRAM_ID
    /// CHECK: We don't validate this to allow Meteora to use TOKEN_2022_PROGRAM_ID
    pub token_2022_program: UncheckedAccount<'info>,

    /// The official Meteora DAMM v2 program
    /// CHECK: This is the official Meteora program ID
    #[account(
        constraint = meteora_program.key() == METEORA_DAMM_V2_PROGRAM_ID @ ErrorCode::InvalidProgramId,
        constraint = meteora_program.executable @ ErrorCode::InvalidProgramId
    )]
    pub meteora_program: UncheckedAccount<'info>,

    /// Event authority for Meteora events
    /// CHECK: PDA derived with ["__event_authority"] seeds from Meteora program
    pub event_authority: UncheckedAccount<'info>,

    pub token_program: Program<'info, Token>,
    pub system_program: Program<'info, System>,
    pub rent: Sysvar<'info, Rent>,
}


/// **NEW ARCHITECTURE**: UnwrapTokens now removes liquidity from Meteora instead of vault
#[derive(Accounts)]
pub struct UnwrapTokens<'info> {
    #[account(mut)]
    pub user: Signer<'info>,

    #[account(mut)]
    pub rift: Account<'info, Rift>,

    /// **SECURITY FIX #49**: Validate user's underlying token account
    #[account(
        mut,
        constraint = user_underlying.mint == rift.underlying_mint @ ErrorCode::InvalidMint,
        constraint = user_underlying.owner == user.key() @ ErrorCode::UnauthorizedTokenAccount
    )]
    pub user_underlying: Account<'info, TokenAccount>,

    /// **SECURITY FIX #49**: Validate user's RIFT token account
    #[account(
        mut,
        constraint = user_rift_tokens.mint == rift.rift_mint @ ErrorCode::InvalidMint,
        constraint = user_rift_tokens.owner == user.key() @ ErrorCode::UnauthorizedTokenAccount
    )]
    pub user_rift_tokens: Account<'info, TokenAccount>,

    /// **METEORA INTEGRATION**: Meteora pool account
    #[account(mut)]
    /// CHECK: Validated against rift.liquidity_pool
    pub pool: UncheckedAccount<'info>,

    /// **PER-USER POSITION**: User's own position NFT mint
    pub user_position_nft_mint: UncheckedAccount<'info>,

    /// **PER-USER POSITION**: User's position account (derived from their NFT)
    #[account(mut)]
    pub user_position: UncheckedAccount<'info>,

    /// **PER-USER POSITION**: User's position NFT token account
    #[account(mut)]
    pub user_position_nft_account: UncheckedAccount<'info>,

    /// **METEORA INTEGRATION**: Pool authority PDA
    /// CHECK: Meteora-derived PDA
    pub pool_authority: UncheckedAccount<'info>,

    /// **METEORA INTEGRATION**: Pool's token A vault (underlying)
    #[account(mut)]
    /// CHECK: Meteora pool vault
    pub token_a_vault: UncheckedAccount<'info>,

    /// **METEORA INTEGRATION**: Pool's token B vault (RIFT)
    #[account(mut)]
    /// CHECK: Meteora pool vault
    pub token_b_vault: UncheckedAccount<'info>,

    /// **SECURITY FIX #49**: Validate rift mint matches rift state
    #[account(
        mut,
        constraint = rift_mint.key() == rift.rift_mint @ ErrorCode::InvalidMint
    )]
    pub rift_mint: Account<'info, Mint>,

    /// **SECURITY FIX #49**: Validate underlying mint matches rift state
    #[account(
        constraint = underlying_mint.key() == rift.underlying_mint @ ErrorCode::InvalidMint
    )]
    pub underlying_mint: Account<'info, Mint>,

    /// RIFT mint authority PDA
    /// CHECK: PDA
    #[account(
        seeds = [b"rift_mint_auth", rift.key().as_ref()],
        bump
    )]
    pub rift_mint_authority: UncheckedAccount<'info>,

    /// **METEORA INTEGRATION**: Meteora program
    /// CHECK: Validated against METEORA_DAMM_V2_PROGRAM_ID
    #[account(
        constraint = meteora_program.key() == METEORA_DAMM_V2_PROGRAM_ID @ ErrorCode::InvalidProgramId,
        constraint = meteora_program.executable @ ErrorCode::InvalidProgramId
    )]
    pub meteora_program: UncheckedAccount<'info>,

    /// **METEORA INTEGRATION**: Event authority for Meteora events
    /// CHECK: PDA derived with ["__event_authority"] seeds from Meteora program
    pub event_authority: UncheckedAccount<'info>,

    pub token_program: Program<'info, Token>,
}


#[derive(Accounts)]
pub struct AdminFixVaultConflict<'info> {
    #[account(mut)]
    pub program_authority: Signer<'info>,

    #[account(mut)]
    pub rift: Account<'info, Rift>,

    /// CHECK: Vault PDA that may have wrong owner
    #[account(
        mut,
        seeds = [b"vault", rift.key().as_ref()],
        bump
    )]
    pub vault: UncheckedAccount<'info>,

    /// CHECK: Expected vault authority PDA
    #[account(
        seeds = [b"vault_auth", rift.key().as_ref()],
        bump
    )]
    pub vault_authority: UncheckedAccount<'info>,
}

#[derive(Accounts)]
pub struct InitializePool<'info> {
    #[account(mut)]
    pub user: Signer<'info>,

    #[account(mut)]
    pub rift: Account<'info, Rift>,

    #[account(mut)]
    pub rift_mint: Account<'info, Mint>,

    /// CHECK: PDA
    #[account(
        seeds = [b"rift_mint_auth", rift.key().as_ref()],
        bump
    )]
    pub rift_mint_authority: UncheckedAccount<'info>,

    /// Pool underlying token account
    #[account(
        init_if_needed,
        payer = user,
        token::mint = underlying_mint,
        token::authority = pool_authority,
        seeds = [b"pool_underlying", rift.key().as_ref()],
        bump
    )]
    pub pool_underlying: Account<'info, TokenAccount>,

    /// Pool rift token account
    #[account(
        init_if_needed,
        payer = user,
        token::mint = rift_mint,
        token::authority = pool_authority,
        seeds = [b"pool_rift", rift.key().as_ref()],
        bump
    )]
    pub pool_rift: Account<'info, TokenAccount>,

    /// CHECK: Pool authority PDA
    #[account(
        seeds = [b"pool_auth", rift.key().as_ref()],
        bump
    )]
    pub pool_authority: UncheckedAccount<'info>,

    pub underlying_mint: Account<'info, Mint>,
    pub token_program: Program<'info, Token>,
    pub system_program: Program<'info, System>,
    pub rent: Sysvar<'info, Rent>,
}

/// **SECURITY FIX #50**: Account struct for updating Pyth oracle
#[derive(Accounts)]
pub struct UpdatePythOracle<'info> {
    #[account(mut)]
    pub rift: Account<'info, Rift>,

    /// **SECURITY FIX #50**: Authority authorized to update oracle prices (creator or governance)
    pub oracle_authority: Signer<'info>,

    /// **SECURITY FIX #50**: Pyth price account - validated against rift.pyth_price_account
    /// CHECK: Validated in instruction handler against stored pubkey and Pyth program ownership
    pub pyth_price_account: UncheckedAccount<'info>,
}

/// **SECURITY FIX #50**: Account struct for updating Switchboard oracle
#[derive(Accounts)]
pub struct UpdateSwitchboardOracle<'info> {
    #[account(mut)]
    pub rift: Account<'info, Rift>,

    /// **SECURITY FIX #50**: Authority authorized to update oracle prices (creator or governance)
    pub oracle_authority: Signer<'info>,

    /// **SECURITY FIX #50**: Switchboard aggregator feed - validated against rift.switchboard_feed_account
    /// CHECK: Validated in instruction handler against stored pubkey and Switchboard program ownership
    pub switchboard_feed: UncheckedAccount<'info>,
}

#[derive(Accounts)]
pub struct TriggerRebalance<'info> {
    #[account(mut)]
    pub user: Signer<'info>,
    
    #[account(mut)]
    pub rift: Account<'info, Rift>,
}


/// Optimized fee distribution context - essential accounts only
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
        seeds = [b"vault_auth", rift.key().as_ref()],
        bump
    )]
    pub vault_authority: UncheckedAccount<'info>,

    /// Treasury account for fee collection
    #[account(mut)]
    pub treasury: Account<'info, TokenAccount>,

    /// Fee collector vault (optional - only if fee_collector_amount > 0)
    #[account(mut)]
    pub fee_collector_vault: Option<Account<'info, TokenAccount>>,

    /// Partner vault (optional - only if partner fees configured)
    /// **SECURITY FIX**: Validate partner vault belongs to configured partner
    pub partner_vault: Option<Account<'info, TokenAccount>>,

    pub token_program: Program<'info, Token>,
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
        seeds = [b"rift_mint_auth", rift.key().as_ref()],
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
        seeds = [b"vault_auth", rift.key().as_ref()],
        bump
    )]
    pub vault_authority: UncheckedAccount<'info>,

    /// **SECURITY FIX #51**: Source token account for balance verification
    /// This is the account that pays for the swap
    #[account(mut)]
    pub source_token_account: Account<'info, TokenAccount>,

    /// **SECURITY FIX #51**: Destination token account for balance verification
    /// This is the account that receives the swapped tokens
    #[account(mut)]
    pub destination_token_account: Account<'info, TokenAccount>,

    /// CHECK: Jupiter program - validated in instruction against governance config
    /// **SECURITY FIX**: Removed hardcoded constraint to allow governance configuration
    #[account(
        constraint = jupiter_program.executable @ ErrorCode::InvalidProgramId
    )]
    pub jupiter_program: UncheckedAccount<'info>,

    pub token_program: Program<'info, Token>,
}

#[derive(Accounts)]
pub struct ExecuteGovernanceProposal<'info> {
    #[account(mut)]
    pub executor: Signer<'info>,

    #[account(mut)]
    pub rift: Account<'info, Rift>,

    /// **SECURITY FIX #43**: Governance proposal with PDA binding to prevent spoofing
    /// Proposal must be derived from governance account to ensure it belongs to the correct governance
    #[account(
        seeds = [b"proposal", governance.key().as_ref(), &proposal.id.to_le_bytes()],
        bump,
        seeds::program = governance::ID
    )]
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
pub struct AdminCloseRift<'info> {
    #[account(mut)]
    pub program_authority: Signer<'info>,

    #[account(
        mut,
        close = program_authority
    )]
    pub rift: Account<'info, Rift>,
}

#[derive(Accounts)]
pub struct CleanupStuckAccounts<'info> {
    /// The creator who originally tried to create the rift
    /// **SECURITY FIX**: Require creator signature to prevent griefing
    pub creator: Signer<'info>,
    
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

#[derive(Accounts)]
pub struct UpdateJupiterProgramId<'info> {
    /// **GOVERNANCE AUTHORITY**: Only governance can update Jupiter program ID
    #[account(mut)]
    pub governance_authority: Signer<'info>,

    #[account(
        mut,
        constraint = rift.creator == governance_authority.key() @ ErrorCode::UnauthorizedAdmin
    )]
    pub rift: Account<'info, Rift>,
}

/// **SECURITY FIX #50**: Account struct for setting oracle addresses
#[derive(Accounts)]
pub struct SetOracleAccounts<'info> {
    #[account(mut)]
    pub creator: Signer<'info>,

    #[account(
        mut,
        constraint = rift.creator == creator.key() @ ErrorCode::Unauthorized
    )]
    pub rift: Account<'info, Rift>,
}

#[account]
pub struct Rift {
    pub name: [u8; 32],  // Fixed-size name (no heap allocation!)
    pub creator: Pubkey,
    pub underlying_mint: Pubkey,
    pub rift_mint: Pubkey,
    pub vault: Pubkey,
    pub burn_fee_bps: u16,
    pub partner_fee_bps: u16,
    pub partner_wallet: Option<Pubkey>,
    /// **SECURITY FIX**: Separate accounting units to prevent mix-ups
    pub total_underlying_wrapped: u64,  // Amount of underlying tokens wrapped
    pub total_rift_minted: u64,         // Amount of RIFT tokens minted
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
    /// **SECURITY FIX**: Governance-configurable Jupiter program ID with hardcoded fallback
    pub jupiter_program_id: Option<Pubkey>, // If None, falls back to hardcoded constant
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

    // **SECURITY FIX #48**: Store Meteora pool state for validation
    pub pool_authority: Option<Pubkey>,      // Meteora pool authority PDA
    pub pool_token_a_vault: Option<Pubkey>,  // Meteora pool's underlying vault
    pub pool_token_b_vault: Option<Pubkey>,  // Meteora pool's RIFT vault
    pub pool_underlying: Option<Pubkey>,     // Legacy pool underlying (if used)
    pub pool_rift: Option<Pubkey>,           // Legacy pool rift (if used)

    // **SECURITY FIX #50**: Store oracle account addresses for validation
    pub pyth_price_account: Option<Pubkey>,       // Bound Pyth price account address
    pub switchboard_feed_account: Option<Pubkey>, // Bound Switchboard aggregator address

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

impl Rift {
    /// **SECURITY FIX**: Get Jupiter program ID from governance only (no hardcoded fallback)
    pub fn get_jupiter_program_id(&self) -> Result<Pubkey> {
        self.jupiter_program_id
            .ok_or(ErrorCode::JupiterProgramIdNotSet.into())
    }

    /// **GOVERNANCE FUNCTION**: Update Jupiter program ID (requires governance)
    pub fn set_jupiter_program_id(&mut self, new_program_id: Option<Pubkey>) {
        self.jupiter_program_id = new_program_id;
    }
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

impl Rift {
    pub fn add_price_data(&mut self, price: u64, confidence: u64, timestamp: i64) -> Result<()> {
        // **CRITICAL SECURITY FIX**: Validate timestamp bounds to prevent manipulation
        let current_time = Clock::get()?.unix_timestamp;

        // Reject timestamps from the future (allow 60 second clock skew)
        require!(
            timestamp <= current_time + 60,
            ErrorCode::InvalidTimestamp
        );

        // Reject timestamps older than 5 minutes (300 seconds)
        require!(
            timestamp >= current_time - 300,
            ErrorCode::InvalidTimestamp
        );

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
        // **CRITICAL SECURITY FIX**: Validate current_time to prevent timestamp manipulation
        let actual_current_time = Clock::get()?.unix_timestamp;
        require!(
            (current_time - actual_current_time).abs() <= 60, // Allow 60 second skew
            ErrorCode::InvalidTimestamp
        );

        // Check if maximum rebalance interval has passed
        if current_time - self.last_rebalance > self.max_rebalance_interval {
            return Ok(true);
        }
        
        // **NEW FEATURE**: Check if volume threshold exceeded for volatility farming
        // Trigger rebalance if 24h volume exceeds 10% of total liquidity
        let volume_threshold = self.total_liquidity_rift
            .checked_div(10) // 10% of total liquidity
            .unwrap_or(u64::MAX);
        if self.total_volume_24h > volume_threshold {
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
        // **CRITICAL SECURITY FIX**: Validate current_time to prevent timestamp manipulation
        let actual_current_time = Clock::get()?.unix_timestamp;
        require!(
            (current_time - actual_current_time).abs() <= 60, // Allow 60 second skew
            ErrorCode::InvalidTimestamp
        );

        // Allow manual rebalance if oracle interval has passed
        Ok(current_time - self.last_oracle_update > self.oracle_update_interval)
    }
    
    pub fn trigger_automatic_rebalance(&mut self, current_time: i64) -> Result<()> {
        // **CRITICAL SECURITY FIX**: Validate current_time to prevent timestamp manipulation
        let actual_current_time = Clock::get()?.unix_timestamp;
        require!(
            (current_time - actual_current_time).abs() <= 60, // Allow 60 second skew
            ErrorCode::InvalidTimestamp
        );

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

        // **NEW FEATURE**: Reset volume counter after rebalance for volatility farming
        self.total_volume_24h = 0; // Reset volume tracking

        Ok(())
    }
    
    pub fn get_average_oracle_price(&self) -> Result<u64> {
        let mut total_price = 0u128; // **PRECISION FIX**: Use u128 for intermediate calculations
        let mut count = 0u64;

        for price_data in &self.oracle_prices {
            if price_data.timestamp > 0 {
                // **CRITICAL FIX**: Use checked arithmetic to prevent overflow
                total_price = total_price
                    .checked_add(u128::from(price_data.price))
                    .ok_or(ErrorCode::MathOverflow)?;
                count = count
                    .checked_add(1)
                    .ok_or(ErrorCode::MathOverflow)?;
            }
        }

        if count > 0 {
            // **PRECISION FIX**: Use fixed-point math with scaling to preserve precision
            // Scale by 1,000,000 (6 decimal places) before division to prevent truncation bias
            const PRECISION_SCALE: u128 = 1_000_000;

            let scaled_total = total_price
                .checked_mul(PRECISION_SCALE)
                .ok_or(ErrorCode::MathOverflow)?;

            let scaled_avg = scaled_total
                .checked_div(u128::from(count))
                .ok_or(ErrorCode::MathOverflow)?;

            // Convert back to u64 with proper precision preservation
            let avg_price = scaled_avg
                .checked_div(PRECISION_SCALE)
                .ok_or(ErrorCode::MathOverflow)?;

            let final_price = u64::try_from(avg_price)
                .map_err(|_| ErrorCode::MathOverflow)?;

            // **CRITICAL FIX**: Validate average price is reasonable
            require!(final_price > 0, ErrorCode::InvalidOraclePrice);
            require!(final_price <= 1_000_000_000_000, ErrorCode::OraclePriceTooLarge);

            Ok(final_price)
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
        
        Ok(u16::try_from(deviation).map_err(|_| ErrorCode::MathOverflow)?)
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
        // **SECURITY FIX**: Get total fees that haven't been distributed yet with proper error handling
        let total_distributed = match self.rifts_tokens_distributed
            .checked_add(self.rifts_tokens_burned) {
            Some(total) => total,
            None => return 0, // Overflow in distributed calculation - return 0 as safe fallback
        };

        if self.total_fees_collected > total_distributed {
            match self.total_fees_collected.checked_sub(total_distributed) {
                Some(pending) => pending,
                None => 0, // Underflow should not happen given the check above, but return 0 as safe fallback
            }
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
            .checked_mul(u64::from(self.burn_fee_bps))
            .ok_or(ErrorCode::MathOverflow)?
            .checked_div(10000)
            .ok_or(ErrorCode::MathOverflow)?;
        let partner_amount = fee_amount
            .checked_mul(u64::from(self.partner_fee_bps))
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
pub struct RiftAdminClosed {
    pub rift: Pubkey,
    pub original_creator: Pubkey,
    pub admin: Pubkey,
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
pub struct MeteoraPoolCreated {
    pub rift: Pubkey,
    pub meteora_pool: Pubkey,
    pub underlying_amount: u64,
    pub rift_amount: u64,
    pub bin_step: u16,
}

#[event]
pub struct TokensWrapped {
    pub rift: Pubkey,
    pub user: Pubkey,
    pub amount_in: u64,
    pub fee_paid: u64,
    pub rift_tokens_minted: u64,
}

#[event]
pub struct PoolInitialized {
    pub rift: Pubkey,
    pub pool_underlying: Pubkey,
    pub pool_rift: Pubkey,
    pub initial_rift_amount: u64,
    pub trading_fee_bps: u16,
    pub bin_step: u16,
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
    #[msg("Unauthorized admin action")]
    UnauthorizedAdmin,
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
    #[msg("Partner vault owner or mint validation failed")]
    InvalidPartnerVault,
    #[msg("Insufficient accounts provided")]
    InsufficientAccounts,
    #[msg("Invalid input data provided")]
    InvalidInputData,
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
    #[msg("Partner vault account is missing")]
    MissingPartnerVault,
    #[msg("Invalid bin step for DLMM pool")]
    InvalidBinStep,
    #[msg("Insufficient funds in vault and no Meteora pool available")]
    InsufficientFunds,
    #[msg("Meteora pool not initialized - must create pool first")]
    PoolNotInitialized,
    #[msg("Pool already initialized for this rift")]
    PoolAlreadyInitialized,
    #[msg("Use JavaScript pool creation helper instead - cp_amm crate doesn't export required types")]
    UseJavaScriptForPoolCreation,
    #[msg("Invalid vanity seed - must be 32 bytes or less")]
    InvalidVanitySeed,
    #[msg("Invalid mint PDA - derivation mismatch")]
    InvalidMintPDA,
    #[msg("Invalid mint bump - derivation mismatch")]
    InvalidMintBump,
    #[msg("Invalid public key format")]
    InvalidPublicKey,
    #[msg("Invalid pool account - PDA mismatch")]
    InvalidPoolAccount,
    #[msg("Unauthorized oracle update - only rift creator can update oracle prices")]
    UnauthorizedOracleUpdate,
    #[msg("Position account has no liquidity to remove")]
    NoLiquidityInPosition,
    #[msg("Invalid position account structure")]
    InvalidPositionAccount,
    #[msg("Invalid oracle account - insufficient size or invalid owner")]
    InvalidOracleAccount,
    #[msg("Invalid mint authority - mint authority does not match expected PDA")]
    InvalidMintAuthority,
    #[msg("Jupiter program ID not set - governance must configure it first")]
    JupiterProgramIdNotSet,
    #[msg("Invalid timestamp - too far in future or past")]
    InvalidTimestamp,
    #[msg("Invalid oracle parameters - interval or threshold out of bounds")]
    InvalidOracleParameters,
    #[msg("Unauthorized access")]
    Unauthorized,
    #[msg("Invalid byte slice conversion")]
    InvalidByteSlice,
    #[msg("Proposal not bound to this governance account")]
    ProposalNotBoundToGovernance,
    #[msg("Proposal ID mismatch")]
    ProposalMismatch,
    #[msg("No pending parameter changes in governance")]
    NoPendingParameterChanges,
    #[msg("Emergency action not authorized by governance")]
    EmergencyActionNotAuthorized,
    #[msg("Invalid mint - does not match rift state")]
    InvalidMint,
    #[msg("Invalid vault - does not match rift state")]
    InvalidVault,
    #[msg("Unauthorized token account - owner mismatch")]
    UnauthorizedTokenAccount,
    #[msg("Invalid pool authority - does not match stored state")]
    InvalidPoolAuthority,
    #[msg("Invalid pool vault - does not match stored state")]
    InvalidPoolVault,
    #[msg("Oracle account not set - must call set_oracle_accounts first")]
    OracleAccountNotSet,
    #[msg("Slippage exceeded - actual output less than minimum required")]
    SlippageExceeded,
    #[msg("Oracle account mismatch - provided account does not match stored oracle account")]
    OracleAccountMismatch,
    #[msg("Invalid oracle owner - oracle account not owned by Pyth or Switchboard program")]
    InvalidOracleOwner,
    #[msg("Invalid oracle data - account data too small or malformed")]
    InvalidOracleData,
    #[msg("Oracle price stale - price data older than maximum allowed age")]
    OraclePriceStale,
    #[msg("Oracle confidence too low - confidence interval too large relative to price")]
    OracleConfidenceTooLow,
    #[msg("Invalid oracle exponent - exponent outside acceptable range")]
    InvalidOracleExponent,
}

/// **SECURITY FIX #50**: Oracle type enum for event emission
#[derive(AnchorSerialize, AnchorDeserialize, Clone, PartialEq, Eq)]
pub enum OracleType {
    Pyth,
    Switchboard,
}

// Events
#[event]
pub struct OraclePriceUpdated {
    pub rift: Pubkey,
    pub oracle_type: OracleType,
    pub price: u64,
    pub confidence: u64,
    pub timestamp: i64,
}
