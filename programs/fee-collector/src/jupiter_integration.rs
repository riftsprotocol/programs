use anchor_lang::prelude::*;
use anchor_lang::solana_program::{
    instruction::{Instruction, AccountMeta},
    program::invoke_signed,
};

// **SECURITY FIX**: Jupiter program ID is now governance-configurable
// Removed hardcoded program ID to prevent security vulnerabilities

/// Execute Jupiter swap with pre-calculated instruction data from off-chain Jupiter API
/// This is the CORRECT architecture - no manual ABI construction needed!
/// **SECURITY FIX #52**: Added strict vault binding validation
pub fn execute_jupiter_swap_with_instruction<'info>(
    jupiter_instruction_data: Vec<u8>,
    remaining_accounts: &[AccountInfo<'info>],
    signer_seeds: &[&[&[u8]]],
    jupiter_program_id: Pubkey, // **SECURITY FIX**: Use governance-configurable Jupiter program ID
    expected_source_vault: Pubkey, // **FIX #52**: Expected source vault for strict binding
    expected_destination_vault: Pubkey, // **FIX #52**: Expected destination vault for strict binding
) -> Result<()> {
    msg!("🚀 Fee Collector: Executing Jupiter swap with off-chain calculated instruction");

    // **SECURITY FIX**: Enhanced Jupiter instruction validation
    require!(remaining_accounts.len() >= 2, FeeCollectorError::InsufficientJupiterAccounts);
    require!(remaining_accounts.len() <= 64, FeeCollectorError::TooManyJupiterAccounts); // Prevent DoS
    
    // Validate instruction data bounds and basic structure
    require!(
        jupiter_instruction_data.len() >= 8 && jupiter_instruction_data.len() <= 1024, 
        FeeCollectorError::InvalidSwapParams
    );
    
    // **ENHANCED SECURITY FIX**: Validate Jupiter ABI format compatibility
    // First 8 bytes should be instruction discriminator - check it's valid Jupiter format
    let discriminator = &jupiter_instruction_data[0..8];
    require!(
        discriminator != [0u8; 8],
        FeeCollectorError::InvalidJupiterInstruction
    );

    // Validate against known Jupiter instruction discriminators
    // These are the actual Jupiter instruction discriminators used in production
    let valid_jupiter_discriminators = [
        [233, 18, 43, 172, 101, 160, 87, 108], // Jupiter swap instruction
        [248, 198, 158, 145, 225, 117, 135, 200], // Jupiter route instruction
        [14, 102, 181, 146, 13, 194, 150, 36], // Jupiter shared accounts route
        [162, 133, 39, 73, 151, 193, 221, 99], // Jupiter exact out route
    ];

    let is_valid_jupiter_discriminator = valid_jupiter_discriminators
        .iter()
        .any(|&valid_disc| discriminator == valid_disc);

    require!(
        is_valid_jupiter_discriminator,
        FeeCollectorError::InvalidJupiterInstruction
    );

    // Validate Jupiter program is first account
    let jupiter_program = &remaining_accounts[0];
    require!(
        *jupiter_program.key == jupiter_program_id,
        FeeCollectorError::InvalidProgramId
    );

    // **SECURITY FIX #52**: Strict vault binding validation with explicit vault checks
    let mut has_token_program = false;
    let mut valid_account_count = 0;
    let mut token_account_count = 0;
    let mut source_vault_found = false;
    let mut destination_vault_found = false;
    let mut fee_collector_authority_found = false;

    for account in &remaining_accounts[1..] {
        // Validate each account is a legitimate Solana account
        require!(
            account.key != &Pubkey::default(),
            FeeCollectorError::InvalidJupiterInstruction
        );

        // **HIGH SECURITY FIX**: Validate account executable status for security
        if account.executable {
            // Only allow known program IDs for executable accounts
            let is_allowed_program =
                *account.key == anchor_spl::token::ID ||
                *account.key == anchor_lang::solana_program::system_program::ID ||
                *account.key == anchor_spl::associated_token::ID ||
                *account.key == jupiter_program_id;
            require!(is_allowed_program, FeeCollectorError::UnauthorizedAmmProgram);
        }

        // Track required programs
        if *account.key == anchor_spl::token::ID {
            has_token_program = true;
        }

        // **SECURITY FIX #52**: Strict vault binding - validate against expected vaults
        if account.owner == &anchor_spl::token::ID {
            token_account_count += 1;

            // **CRITICAL SECURITY**: Validate token accounts are writable where expected
            require!(account.is_writable, FeeCollectorError::InvalidJupiterInstruction);

            // **STRICT BINDING FIX #52**: Match against expected source/destination vaults
            if *account.key == expected_source_vault {
                source_vault_found = true;
                msg!("✅ Source vault validated: {}", expected_source_vault);
            } else if *account.key == expected_destination_vault {
                destination_vault_found = true;
                msg!("✅ Destination vault validated: {}", expected_destination_vault);
            }

            // Track if authority is signing
            if account.is_signer {
                fee_collector_authority_found = true;
            }
        }

        valid_account_count += 1;
    }

    // **CRITICAL FIX #52**: Require EXACT vault matches (not just any token accounts)
    require!(
        source_vault_found,
        FeeCollectorError::VaultNotFoundInRoute
    );
    require!(
        destination_vault_found,
        FeeCollectorError::VaultNotFoundInRoute
    );
    require!(fee_collector_authority_found, FeeCollectorError::InvalidJupiterInstruction);
    
    // Jupiter swaps must include token program
    require!(has_token_program, FeeCollectorError::InvalidJupiterInstruction);
    // **HIGH SECURITY FIX**: Validate minimum token accounts for legitimate swap
    require!(
        token_account_count >= 2, // At minimum: source and destination token accounts
        FeeCollectorError::InvalidJupiterInstruction
    );
    // Reasonable account count limits (Jupiter swaps typically use 8-20 accounts)
    require!(
        valid_account_count >= 4 && valid_account_count <= 32,
        FeeCollectorError::InvalidSwapParams
    );

    // **CRITICAL SECURITY**: Additional route validation to prevent value extraction
    // Validate that the instruction discriminator matches expected Jupiter swap types
    let discriminator = &jupiter_instruction_data[0..8];
    let is_swap_instruction = discriminator == [233, 18, 43, 172, 101, 160, 87, 108]; // Jupiter swap
    let is_route_instruction = discriminator == [248, 198, 158, 145, 225, 117, 135, 200]; // Jupiter route

    require!(
        is_swap_instruction || is_route_instruction,
        FeeCollectorError::InvalidJupiterInstruction
    );

    // **ROUTE SECURITY**: Validate instruction data doesn't contain suspicious patterns
    // Check for patterns that might indicate value extraction or unauthorized transfers
    let instruction_str = String::from_utf8_lossy(&jupiter_instruction_data);
    require!(
        !instruction_str.contains("withdraw") &&
        !instruction_str.contains("transfer_all") &&
        !instruction_str.contains("drain"),
        FeeCollectorError::InvalidJupiterInstruction
    );

    // Build Jupiter instruction with validated accounts
    let jupiter_instruction = Instruction {
        program_id: jupiter_program_id,
        accounts: remaining_accounts[1..] // Skip Jupiter program account
            .iter()
            .map(|account| AccountMeta {
                pubkey: *account.key,
                is_signer: account.is_signer,
                is_writable: account.is_writable,
            })
            .collect(),
        data: jupiter_instruction_data,
    };

    msg!("📋 Jupiter instruction built with {} accounts and {} bytes of data", 
         jupiter_instruction.accounts.len(), 
         jupiter_instruction.data.len());

    // **SECURITY FIX**: Validate signer seeds match expected signers in accounts
    // Generate expected signer pubkey from seeds to verify it matches instruction accounts
    if !signer_seeds.is_empty() {
        let expected_signer = Pubkey::create_program_address(
            signer_seeds[0], 
            &crate::ID
        ).map_err(|_| FeeCollectorError::InvalidSwapParams)?;
        
        // Verify at least one account in the instruction matches our signer
        let mut found_matching_signer = false;
        for account in &jupiter_instruction.accounts {
            if account.pubkey == expected_signer && account.is_signer {
                found_matching_signer = true;
                break;
            }
        }
        
        require!(
            found_matching_signer,
            FeeCollectorError::InvalidSwapParams
        );
    }

    // Execute Jupiter swap via CPI with validated signer seeds
    invoke_signed(
        &jupiter_instruction,
        &remaining_accounts[1..], // Skip Jupiter program account
        signer_seeds,
    )?;

    msg!("✅ Jupiter swap executed successfully via CPI");
    Ok(())
}

use crate::FeeCollectorError;