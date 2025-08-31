// RIFTS Governance Program - Token-based voting system
use anchor_lang::prelude::*;
use anchor_spl::token::{TokenAccount};

declare_id!("7rYo4k9xGmQU7zkQEBqjfBU1y9hkQnRFtFbpn23dZNQR");

#[program]
pub mod governance {
    use super::*;
    
    /// Initialize governance system
    pub fn initialize_governance(
        ctx: Context<InitializeGovernance>,
        rifts_mint: Pubkey,
        min_voting_period: i64,
        min_execution_delay: i64,
    ) -> Result<()> {
        let governance = &mut ctx.accounts.governance;
        
        governance.authority = ctx.accounts.authority.key();
        governance.rifts_mint = rifts_mint;
        governance.min_voting_period = min_voting_period;
        governance.min_execution_delay = min_execution_delay;
        governance.total_proposals = 0;
        governance.total_executed = 0;
        
        // Initialize extended governance fields
        governance.max_treasury_spend = 1_000_000 * 10u64.pow(9); // 1M tokens max per proposal
        governance.emergency_pause_active = false;
        governance.pause_initiated_at = 0;
        governance.pause_duration = 0;
        governance.assets_frozen = false;
        governance.freeze_initiated_at = 0;
        governance.upgrade_ready_timestamp = 0;
        
        // Initialize pending execution states
        governance.pending_parameter_changes = None;
        governance.parameter_change_proposal_id = 0;
        governance.pending_treasury_spend = None;
        governance.treasury_spend_proposal_id = 0;
        governance.pending_protocol_upgrade = None;
        governance.protocol_upgrade_proposal_id = 0;
        governance.pending_oracle_updates = None;
        governance.oracle_update_proposal_id = 0;
        
        Ok(())
    }
    
    /// Create a new governance proposal
    pub fn create_proposal(
        ctx: Context<CreateProposal>,
        title: String,
        description: String,
        proposal_type: ProposalType,
        execution_data: Vec<u8>,
    ) -> Result<()> {
        let proposal = &mut ctx.accounts.proposal;
        let governance = &mut ctx.accounts.governance;
        
        // Validate input bounds
        require!(title.len() <= 100, GovernanceError::InvalidInputLength);
        require!(description.len() <= 1000, GovernanceError::InvalidInputLength);
        require!(execution_data.len() <= 10000, GovernanceError::InvalidInputLength);
        
        // **CRITICAL FIX**: Require minimum RIFTS token balance to create proposal
        // Validate token decimals first
        require!(
            ctx.accounts.proposer_rifts_account.mint == governance.rifts_mint,
            GovernanceError::InvalidRiftsMint
        );
        
        let min_proposal_tokens = 1000u64
            .checked_mul(10u64.pow(9))
            .ok_or(GovernanceError::MathOverflow)?;
        require!(
            ctx.accounts.proposer_rifts_account.amount >= min_proposal_tokens,
            GovernanceError::InsufficientTokensToPropose
        );
        
        proposal.id = governance.total_proposals;
        proposal.proposer = ctx.accounts.proposer.key();
        proposal.title = title;
        proposal.description = description;
        proposal.proposal_type = proposal_type;
        proposal.execution_data = execution_data;
        proposal.voting_start = Clock::get()?.unix_timestamp;
        proposal.voting_end = proposal.voting_start + governance.min_voting_period;
        proposal.votes_for = 0;
        proposal.votes_against = 0;
        proposal.total_voters = 0;
        proposal.status = ProposalStatus::Active;
        proposal.created_at = Clock::get()?.unix_timestamp;
        
        // Set snapshot fields to prevent flash loan attacks
        proposal.snapshot_taken_at = proposal.created_at;
        proposal.snapshot_slot = Clock::get()?.slot;
        // Require minimum 20% participation (significantly increased for security)
        proposal.min_participation_required = 500000u64
            .checked_mul(10u64.pow(9))
            .ok_or(GovernanceError::MathOverflow)?; // 500k RIFTS tokens minimum
        
        governance.total_proposals = governance.total_proposals
            .checked_add(1)
            .ok_or(GovernanceError::MathOverflow)?;
        
        emit!(ProposalCreated {
            proposal_id: proposal.id,
            proposer: proposal.proposer,
            title: proposal.title.clone(),
            proposal_type,
        });
        
        Ok(())
    }
    
    /// Cast a vote on a proposal
    pub fn cast_vote(
        ctx: Context<CastVote>,
        vote: VoteChoice,
    ) -> Result<()> {
        let proposal = &mut ctx.accounts.proposal;
        let vote_record = &mut ctx.accounts.vote_record;
        let current_time = Clock::get()?.unix_timestamp;
        
        // **SECURITY FIX**: Use snapshot balance system for voting power
        let _existing_voting_power = vote_record.voting_power;
        
        // Require tokens to be held since before proposal creation (24 hours minimum)
        require!(
            current_time > proposal.snapshot_taken_at + 86400, // 24 hours minimum hold
            GovernanceError::InsufficientHoldingPeriod
        );
        
        // **CRITICAL SECURITY FIX**: Use proper snapshot voting system to prevent flash loan attacks
        let current_balance = ctx.accounts.voter_rifts_account.amount;
        let snapshot_account = &ctx.accounts.snapshot_account;
        
        // Verify snapshot account is valid for this proposal
        require!(
            snapshot_account.proposal == proposal.key(),
            GovernanceError::InvalidSnapshot
        );
        require!(
            snapshot_account.voter == ctx.accounts.voter.key(),
            GovernanceError::InvalidSnapshot
        );
        require!(
            snapshot_account.snapshot_timestamp == proposal.snapshot_taken_at,
            GovernanceError::InvalidSnapshot
        );
        
        // Use snapshot balance to prevent flash loan attacks
        let snapshot_balance = snapshot_account.token_balance;
        
        // **SECURITY REQUIREMENT**: Voter must still hold at least their snapshot balance
        require!(
            current_balance >= snapshot_balance,
            GovernanceError::TokenBalanceDecreased
        );
        
        // **SECURITY IMPROVEMENT**: Minimum voting power requirement
        let min_voting_power = proposal.min_participation_required
            .checked_div(1000) // Require 0.1% of minimum participation
            .ok_or(GovernanceError::MathOverflow)?;
        require!(
            snapshot_balance >= min_voting_power,
            GovernanceError::InsufficientTokensToVote
        );
        
        // Check voting period
        require!(
            current_time >= proposal.voting_start && current_time <= proposal.voting_end,
            GovernanceError::VotingPeriodEnded
        );
        
        require!(
            proposal.status == ProposalStatus::Active,
            GovernanceError::ProposalNotActive
        );
        
        // Require minimum RIFTS balance to vote (using snapshot balance)
        let min_vote_tokens = 100u64
            .checked_mul(10u64.pow(9))
            .ok_or(GovernanceError::MathOverflow)?;
        require!(
            snapshot_balance >= min_vote_tokens,
            GovernanceError::InsufficientTokensToVote
        );
        
        // Check if user already voted
        require!(
            vote_record.voter == Pubkey::default(),
            GovernanceError::AlreadyVoted
        );
        
        // **SECURITY FIX**: Record the vote using snapshot balance for voting power
        vote_record.voter = ctx.accounts.voter.key();
        vote_record.proposal = proposal.key();
        vote_record.vote = vote;
        vote_record.voting_power = snapshot_balance; // Use snapshot balance, not current balance
        vote_record.timestamp = current_time;
        
        // **CRITICAL FIX**: Update proposal vote counts with snapshot balance
        match vote {
            VoteChoice::For => {
                proposal.votes_for = proposal.votes_for
                    .checked_add(snapshot_balance)
                    .ok_or(GovernanceError::VoteOverflow)?;
            }
            VoteChoice::Against => {
                proposal.votes_against = proposal.votes_against
                    .checked_add(snapshot_balance)
                    .ok_or(GovernanceError::VoteOverflow)?;
            }
        }
        proposal.total_voters = proposal.total_voters
            .checked_add(1)
            .ok_or(GovernanceError::VoteOverflow)?;
        
        emit!(VoteCast {
            proposal_id: proposal.id,
            voter: vote_record.voter,
            vote,
            voting_power: snapshot_balance,
        });
        
        Ok(())
    }
    
    /// Execute a passed proposal
    pub fn execute_proposal(
        ctx: Context<ExecuteProposal>,
    ) -> Result<()> {
        let proposal = &mut ctx.accounts.proposal;
        let governance = &mut ctx.accounts.governance;
        let current_time = Clock::get()?.unix_timestamp;
        
        // Check if voting period has ended
        require!(
            current_time > proposal.voting_end,
            GovernanceError::VotingStillActive
        );
        
        // Check if execution delay has passed
        require!(
            current_time >= proposal.voting_end + governance.min_execution_delay,
            GovernanceError::ExecutionDelayNotMet
        );
        
        require!(
            proposal.status == ProposalStatus::Active,
            GovernanceError::ProposalNotActive
        );
        
        // **CRITICAL FIX**: Check minimum participation requirement with overflow protection
        let total_votes = proposal.votes_for
            .checked_add(proposal.votes_against)
            .ok_or(GovernanceError::VoteOverflow)?;
        require!(
            total_votes >= proposal.min_participation_required,
            GovernanceError::InsufficientParticipation
        );
        
        // Check if proposal passed (simple majority)
        require!(
            proposal.votes_for > proposal.votes_against,
            GovernanceError::ProposalDidNotPass
        );
        
        // Execute based on proposal type with real implementation
        match proposal.proposal_type {
            ProposalType::ParameterChange => {
                // Parse and execute parameter changes from execution_data
                if !proposal.execution_data.is_empty() {
                    // Decode parameter change instructions
                    let param_changes = ProposalParameterChanges::try_from_slice(&proposal.execution_data)
                        .map_err(|_| GovernanceError::InvalidExecutionData)?;
                    
                    // Validate parameter ranges before execution
                    if let Some(new_fee) = param_changes.burn_fee_bps {
                        require!(new_fee <= 4500, GovernanceError::InvalidParameterValue);
                    }
                    if let Some(new_partner_fee) = param_changes.partner_fee_bps {
                        require!(new_partner_fee <= 500, GovernanceError::InvalidParameterValue);
                    }
                    if let Some(new_interval) = param_changes.oracle_update_interval {
                        require!(new_interval >= 600, GovernanceError::InvalidParameterValue);
                    }
                    
                    // Store execution data for the rift program to read
                    governance.pending_parameter_changes = Some(param_changes.clone());
                    governance.parameter_change_proposal_id = proposal.id;
                    
                    emit!(ParameterChangeExecuted {
                        proposal_id: proposal.id,
                        changes: param_changes,
                    });
                }
                proposal.status = ProposalStatus::Executed;
            },
            ProposalType::TreasurySpend => {
                // Execute treasury spending with real token transfers
                if !proposal.execution_data.is_empty() {
                    let spend_instruction = TreasurySpendInstruction::try_from_slice(&proposal.execution_data)
                        .map_err(|_| GovernanceError::InvalidExecutionData)?;
                    
                    // Validate spend amount and recipient
                    require!(spend_instruction.amount > 0, GovernanceError::InvalidSpendAmount);
                    require!(spend_instruction.amount <= governance.max_treasury_spend, GovernanceError::ExceedsMaxSpend);
                    require!(spend_instruction.recipient != Pubkey::default(), GovernanceError::InvalidRecipient);
                    
                    // Store for treasury program to execute the actual transfer
                    governance.pending_treasury_spend = Some(spend_instruction.clone());
                    governance.treasury_spend_proposal_id = proposal.id;
                    
                    emit!(TreasurySpendExecuted {
                        proposal_id: proposal.id,
                        recipient: spend_instruction.recipient,
                        amount: spend_instruction.amount,
                        token_mint: spend_instruction.token_mint,
                    });
                }
                proposal.status = ProposalStatus::Executed;
            },
            ProposalType::ProtocolUpgrade => {
                // Initiate protocol upgrade with version validation
                if !proposal.execution_data.is_empty() {
                    let upgrade_instruction = ProtocolUpgradeInstruction::try_from_slice(&proposal.execution_data)
                        .map_err(|_| GovernanceError::InvalidExecutionData)?;
                    
                    // Validate upgrade parameters
                    require!(upgrade_instruction.new_program_id != Pubkey::default(), GovernanceError::InvalidProgramId);
                    require!(upgrade_instruction.buffer_account != Pubkey::default(), GovernanceError::InvalidBufferAccount);
                    require!(upgrade_instruction.spill_account != Pubkey::default(), GovernanceError::InvalidSpillAccount);
                    
                    // Ensure upgrade authority matches governance
                    require!(
                        upgrade_instruction.upgrade_authority == governance.authority,
                        GovernanceError::UnauthorizedUpgrade
                    );
                    
                    // Store upgrade instruction for execution
                    governance.pending_protocol_upgrade = Some(upgrade_instruction.clone());
                    governance.protocol_upgrade_proposal_id = proposal.id;
                    governance.upgrade_ready_timestamp = current_time + 86400; // 24 hour delay
                    
                    emit!(ProtocolUpgradeInitiated {
                        proposal_id: proposal.id,
                        new_program_id: upgrade_instruction.new_program_id,
                        ready_timestamp: governance.upgrade_ready_timestamp,
                    });
                }
                proposal.status = ProposalStatus::Executed;
            },
            ProposalType::EmergencyAction => {
                // Execute emergency action with immediate effect
                if !proposal.execution_data.is_empty() {
                    let emergency_action = EmergencyActionInstruction::try_from_slice(&proposal.execution_data)
                        .map_err(|_| GovernanceError::InvalidExecutionData)?;
                    
                    // Validate emergency action type
                    match emergency_action.action_type {
                        EmergencyActionType::PauseProtocol => {
                            governance.emergency_pause_active = true;
                            governance.pause_initiated_at = current_time;
                            governance.pause_duration = emergency_action.duration.unwrap_or(86400); // Default 24 hours
                        },
                        EmergencyActionType::UnpauseProtocol => {
                            governance.emergency_pause_active = false;
                            governance.pause_initiated_at = 0;
                            governance.pause_duration = 0;
                        },
                        EmergencyActionType::UpdateOracleRegistry => {
                            if let Some(new_oracles) = emergency_action.oracle_updates {
                                governance.pending_oracle_updates = Some(new_oracles);
                                governance.oracle_update_proposal_id = proposal.id;
                            }
                        },
                        EmergencyActionType::FreezeAssets => {
                            governance.assets_frozen = true;
                            governance.freeze_initiated_at = current_time;
                        },
                        EmergencyActionType::UnfreezeAssets => {
                            governance.assets_frozen = false;
                            governance.freeze_initiated_at = 0;
                        },
                    }
                    
                    emit!(EmergencyActionExecuted {
                        proposal_id: proposal.id,
                        action_type: emergency_action.action_type,
                        executed_at: current_time,
                    });
                }
                proposal.status = ProposalStatus::Executed;
            },
        }
        
        proposal.executed_at = current_time;
        governance.total_executed = governance.total_executed
            .checked_add(1)
            .ok_or(GovernanceError::MathOverflow)?;
        
        emit!(ProposalExecuted {
            proposal_id: proposal.id,
            executed_by: ctx.accounts.executor.key(),
            executed_at: current_time,
        });
        
        Ok(())
    }
    
    /// Cancel a proposal (only by proposer or governance authority)
    pub fn cancel_proposal(
        ctx: Context<CancelProposal>,
    ) -> Result<()> {
        let proposal = &mut ctx.accounts.proposal;
        let governance = &ctx.accounts.governance;
        
        // Only proposer or governance authority can cancel
        require!(
            ctx.accounts.canceller.key() == proposal.proposer || 
            ctx.accounts.canceller.key() == governance.authority,
            GovernanceError::UnauthorizedCancel
        );
        
        require!(
            proposal.status == ProposalStatus::Active,
            GovernanceError::ProposalNotActive
        );
        
        proposal.status = ProposalStatus::Cancelled;
        
        emit!(ProposalCancelled {
            proposal_id: proposal.id,
            cancelled_by: ctx.accounts.canceller.key(),
        });
        
        Ok(())
    }
}

// Account structures
#[derive(Accounts)]
pub struct InitializeGovernance<'info> {
    #[account(mut)]
    pub authority: Signer<'info>,
    
    #[account(
        init,
        payer = authority,
        space = 8 + std::mem::size_of::<Governance>(),
        seeds = [b"governance", authority.key().as_ref()],
        constraint = authority.key() != Pubkey::default() @ GovernanceError::InvalidSeedComponent,
        bump,
        constraint = authority.lamports() >= Rent::get()?.minimum_balance(8 + std::mem::size_of::<Governance>()) @ GovernanceError::InsufficientRentExemption
    )]
    pub governance: Account<'info, Governance>,
    
    pub system_program: Program<'info, System>,
}

#[derive(Accounts)]
pub struct CreateProposal<'info> {
    #[account(mut)]
    pub proposer: Signer<'info>,
    
    #[account(mut)]
    pub governance: Account<'info, Governance>,
    
    #[account(
        init,
        payer = proposer,
        space = 8 + std::mem::size_of::<Proposal>(),
        seeds = [b"proposal", governance.key().as_ref(), &governance.total_proposals.to_le_bytes()],
        constraint = governance.key() != Pubkey::default() @ GovernanceError::InvalidSeedComponent,
        bump,
        constraint = proposer.lamports() >= Rent::get()?.minimum_balance(8 + std::mem::size_of::<Proposal>()) @ GovernanceError::InsufficientRentExemption
    )]
    pub proposal: Account<'info, Proposal>,
    
    #[account(
        constraint = proposer_rifts_account.owner == proposer.key()
    )]
    pub proposer_rifts_account: Account<'info, TokenAccount>,
    
    pub system_program: Program<'info, System>,
}

#[derive(Accounts)]
pub struct CastVote<'info> {
    #[account(mut)]
    pub voter: Signer<'info>,
    
    #[account(mut)]
    pub proposal: Account<'info, Proposal>,
    
    #[account(
        init,
        payer = voter,
        space = 8 + std::mem::size_of::<VoteRecord>(),
        seeds = [b"vote", proposal.key().as_ref(), voter.key().as_ref()],
        constraint = proposal.key() != Pubkey::default() && voter.key() != Pubkey::default() @ GovernanceError::InvalidSeedComponent,
        bump,
        constraint = voter.lamports() >= Rent::get()?.minimum_balance(8 + std::mem::size_of::<VoteRecord>()) @ GovernanceError::InsufficientRentExemption
    )]
    pub vote_record: Account<'info, VoteRecord>,
    
    #[account(
        constraint = voter_rifts_account.owner == voter.key()
    )]
    pub voter_rifts_account: Account<'info, TokenAccount>,
    
    /// **SECURITY FIX**: Snapshot account for flash loan protection
    #[account(
        constraint = snapshot_account.proposal == proposal.key(),
        constraint = snapshot_account.voter == voter.key()
    )]
    pub snapshot_account: Account<'info, SnapshotAccount>,
    
    pub system_program: Program<'info, System>,
}

#[derive(Accounts)]
pub struct ExecuteProposal<'info> {
    #[account(mut)]
    pub executor: Signer<'info>,
    
    #[account(mut)]
    pub governance: Account<'info, Governance>,
    
    #[account(mut)]
    pub proposal: Account<'info, Proposal>,
}

#[derive(Accounts)]
pub struct CancelProposal<'info> {
    #[account(mut)]
    pub canceller: Signer<'info>,
    
    pub governance: Account<'info, Governance>,
    
    #[account(mut)]
    pub proposal: Account<'info, Proposal>,
}

// State accounts
#[account]
pub struct Governance {
    pub authority: Pubkey,
    pub rifts_mint: Pubkey,
    pub min_voting_period: i64,
    pub min_execution_delay: i64,
    pub total_proposals: u64,
    pub total_executed: u64,
    
    // Extended governance state for real execution
    pub max_treasury_spend: u64,
    pub emergency_pause_active: bool,
    pub pause_initiated_at: i64,
    pub pause_duration: i64,
    pub assets_frozen: bool,
    pub freeze_initiated_at: i64,
    pub upgrade_ready_timestamp: i64,
    
    // Pending execution states
    pub pending_parameter_changes: Option<ProposalParameterChanges>,
    pub parameter_change_proposal_id: u64,
    pub pending_treasury_spend: Option<TreasurySpendInstruction>,
    pub treasury_spend_proposal_id: u64,
    pub pending_protocol_upgrade: Option<ProtocolUpgradeInstruction>,
    pub protocol_upgrade_proposal_id: u64,
    pub pending_oracle_updates: Option<Vec<Pubkey>>,
    pub oracle_update_proposal_id: u64,
}

#[account]
pub struct Proposal {
    pub id: u64,
    pub proposer: Pubkey,
    pub title: String,
    pub description: String,
    pub proposal_type: ProposalType,
    pub execution_data: Vec<u8>,
    pub voting_start: i64,
    pub voting_end: i64,
    pub votes_for: u64,
    pub votes_against: u64,
    pub total_voters: u32,
    pub status: ProposalStatus,
    pub created_at: i64,
    pub executed_at: i64,
    // Vote power snapshot fields
    pub snapshot_taken_at: i64,
    pub snapshot_slot: u64,
    pub min_participation_required: u64,  // Minimum participation for proposal validity
}

#[account]
pub struct VoteRecord {
    pub voter: Pubkey,
    pub proposal: Pubkey,
    pub vote: VoteChoice,
    pub voting_power: u64,
    pub timestamp: i64,
}

#[account]
pub struct VoteSnapshot {
    pub proposal_id: u64,
    pub voter: Pubkey,
    pub snapshot_power: u64,  // Vote power at proposal creation
    pub snapshot_taken_at: i64,
}

#[account]
pub struct SnapshotAccount {
    pub proposal: Pubkey,
    pub voter: Pubkey,
    pub token_balance: u64,
    pub snapshot_timestamp: i64,
}

// Enums
#[derive(AnchorSerialize, AnchorDeserialize, Clone, Copy, PartialEq, Eq)]
pub enum ProposalType {
    ParameterChange,
    TreasurySpend,
    ProtocolUpgrade,
    EmergencyAction,
}

#[derive(AnchorSerialize, AnchorDeserialize, Clone, Copy, PartialEq, Eq)]
pub enum ProposalStatus {
    Active,
    Executed,
    Cancelled,
    Failed,
}

#[derive(AnchorSerialize, AnchorDeserialize, Clone, Copy, PartialEq, Eq)]
pub enum VoteChoice {
    For,
    Against,
}

// Events
#[event]
pub struct ProposalCreated {
    pub proposal_id: u64,
    pub proposer: Pubkey,
    pub title: String,
    pub proposal_type: ProposalType,
}

#[event]
pub struct VoteCast {
    pub proposal_id: u64,
    pub voter: Pubkey,
    pub vote: VoteChoice,
    pub voting_power: u64,
}

#[event]
pub struct ProposalExecuted {
    pub proposal_id: u64,
    pub executed_by: Pubkey,
    pub executed_at: i64,
}

#[event]
pub struct ProposalCancelled {
    pub proposal_id: u64,
    pub cancelled_by: Pubkey,
}

#[event]
pub struct ParameterChangeExecuted {
    pub proposal_id: u64,
    pub changes: ProposalParameterChanges,
}

#[event]
pub struct TreasurySpendExecuted {
    pub proposal_id: u64,
    pub recipient: Pubkey,
    pub amount: u64,
    pub token_mint: Pubkey,
}

#[event]
pub struct ProtocolUpgradeInitiated {
    pub proposal_id: u64,
    pub new_program_id: Pubkey,
    pub ready_timestamp: i64,
}

#[event]
pub struct EmergencyActionExecuted {
    pub proposal_id: u64,
    pub action_type: EmergencyActionType,
    pub executed_at: i64,
}

// Execution instruction data structures
#[derive(AnchorSerialize, AnchorDeserialize, Clone)]
pub struct ProposalParameterChanges {
    pub burn_fee_bps: Option<u16>,
    pub partner_fee_bps: Option<u16>,
    pub oracle_update_interval: Option<i64>,
    pub max_rebalance_interval: Option<i64>,
    pub arbitrage_threshold_bps: Option<u16>,
}

#[derive(AnchorSerialize, AnchorDeserialize, Clone)]
pub struct TreasurySpendInstruction {
    pub recipient: Pubkey,
    pub amount: u64,
    pub token_mint: Pubkey,
    pub description: String,
}

#[derive(AnchorSerialize, AnchorDeserialize, Clone)]
pub struct ProtocolUpgradeInstruction {
    pub new_program_id: Pubkey,
    pub buffer_account: Pubkey,
    pub spill_account: Pubkey,
    pub upgrade_authority: Pubkey,
    pub version: String,
}

#[derive(AnchorSerialize, AnchorDeserialize, Clone)]
pub struct EmergencyActionInstruction {
    pub action_type: EmergencyActionType,
    pub duration: Option<i64>,
    pub oracle_updates: Option<Vec<Pubkey>>,
    pub description: String,
}

#[derive(AnchorSerialize, AnchorDeserialize, Clone, Copy, PartialEq, Eq)]
pub enum EmergencyActionType {
    PauseProtocol,
    UnpauseProtocol,
    UpdateOracleRegistry,
    FreezeAssets,
    UnfreezeAssets,
}

// Errors
#[error_code]
pub enum GovernanceError {
    #[msg("Insufficient RIFTS tokens to create proposal")]
    InsufficientTokensToPropose,
    #[msg("Insufficient RIFTS tokens to vote")]
    InsufficientTokensToVote,
    #[msg("Voting period has ended")]
    VotingPeriodEnded,
    #[msg("Voting is still active")]
    VotingStillActive,
    #[msg("Proposal is not active")]
    ProposalNotActive,
    #[msg("Already voted on this proposal")]
    AlreadyVoted,
    #[msg("Proposal did not pass")]
    ProposalDidNotPass,
    #[msg("Execution delay has not been met")]
    ExecutionDelayNotMet,
    #[msg("Unauthorized to cancel proposal")]
    UnauthorizedCancel,
    #[msg("Insufficient holding period for voting")]
    InsufficientHoldingPeriod,
    #[msg("Proposal did not meet minimum participation")]
    InsufficientParticipation,
    #[msg("Insufficient rent exemption for account creation")]
    InsufficientRentExemption,
    #[msg("Invalid seed component in PDA derivation")]
    InvalidSeedComponent,
    #[msg("Math overflow")]
    MathOverflow,
    #[msg("Invalid input length")]
    InvalidInputLength,
    #[msg("Token balance decreased since proposal creation")]
    TokenBalanceDecreased,
    #[msg("Vote count overflow detected")]
    VoteOverflow,
    #[msg("Invalid RIFTS mint for governance")]
    InvalidRiftsMint,
    #[msg("Invalid execution data format")]
    InvalidExecutionData,
    #[msg("Invalid parameter value")]
    InvalidParameterValue,
    #[msg("Invalid spend amount")]
    InvalidSpendAmount,
    #[msg("Amount exceeds maximum treasury spend")]
    ExceedsMaxSpend,
    #[msg("Invalid recipient address")]
    InvalidRecipient,
    #[msg("Invalid program ID for upgrade")]
    InvalidProgramId,
    #[msg("Invalid buffer account")]
    InvalidBufferAccount,
    #[msg("Invalid spill account")]
    InvalidSpillAccount,
    #[msg("Unauthorized upgrade attempt")]
    UnauthorizedUpgrade,
    #[msg("Insufficient token balance at snapshot time")]
    InsufficientSnapshotBalance,
    #[msg("Invalid or mismatched snapshot account")]
    InvalidSnapshot,
}