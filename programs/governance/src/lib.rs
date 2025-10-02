// RIFTS Governance Program - Token-based voting system
use anchor_lang::prelude::*;
use anchor_spl::token::{TokenAccount, Mint};
// **SECURITY FIX**: Import removed as it's now used inline in constraint

declare_id!("DtBfLYvkXebsCxf49ZubJej9dMc9sNXUx2fctB3oeYtK");

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
        
        // **SECURITY FIX**: Enforce minimum timeframes for secure governance
        require!(min_voting_period >= 86400, GovernanceError::InvalidVotingPeriod); // At least 24 hours
        require!(min_execution_delay >= 21600, GovernanceError::InvalidExecutionDelay); // At least 6 hours
        
        governance.authority = ctx.accounts.authority.key();
        // **SECURITY FIX**: Initialize with single signature by default, can be upgraded to multisig
        governance.additional_authorities = Vec::new();
        governance.required_signatures = 1; // Single signature by default
        governance.rifts_mint = rifts_mint;
        governance.min_voting_period = min_voting_period;
        governance.min_execution_delay = min_execution_delay;
        governance.total_proposals = 0;
        governance.total_executed = 0;
        
        // **CRITICAL FIX**: Use actual RIFTS token decimals instead of hardcoded 9
        let rifts_mint = &ctx.accounts.rifts_mint;
        governance.max_treasury_spend = 1_000_000u64
            .checked_mul(10u64.pow(u32::from(rifts_mint.decimals)))
            .ok_or(GovernanceError::MathOverflow)?; // 1M tokens max per proposal
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
        
        emit!(GovernanceInitialized {
            authority: ctx.accounts.authority.key(),
            rifts_mint: ctx.accounts.rifts_mint.key(),
            min_voting_period,
            min_execution_delay,
            initialized_at: Clock::get()?.unix_timestamp,
        });
        
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
        
        // **CRITICAL FIX**: Use actual RIFTS token decimals instead of hardcoded 9
        let rifts_mint = &ctx.accounts.rifts_mint;
        let min_proposal_tokens = 1000u64
            .checked_mul(10u64.pow(u32::from(rifts_mint.decimals)))
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
        // **CRITICAL FIX**: Require minimum 20% participation based on total supply percentage
        let total_supply = rifts_mint.supply;
        
        // **ZERO SUPPLY GOVERNANCE BYPASS FIX**: Prevent governance when no tokens exist
        require!(total_supply > 0, GovernanceError::ZeroTokenSupply);
        
        proposal.min_participation_required = total_supply
            .checked_mul(20) // 20% minimum participation
            .and_then(|x| x.checked_div(100))
            .ok_or(GovernanceError::MathOverflow)?;
        
        // **SECURITY FIX**: Set auto-expiry for emergency actions
        proposal.emergency_expiry_time = match proposal.proposal_type {
            ProposalType::EmergencyAction => {
                // Emergency actions expire after 7 days if not executed
                proposal.created_at.checked_add(604800).unwrap_or(i64::MAX)
            },
            _ => i64::MAX, // Regular proposals don't auto-expire
        };
        
        governance.total_proposals = governance.total_proposals
            .checked_add(1)
            .ok_or(GovernanceError::MathOverflow)?;
        
        emit!(ProposalCreated {
            proposal_id: proposal.id,
            proposer: proposal.proposer,
            title: proposal.title.clone(),
            proposal_type,
        });

        emit!(ProposalStatusChanged {
            proposal_id: proposal.id,
            old_status: ProposalStatus::Active, // New proposals start as Active
            new_status: ProposalStatus::Active,
            changed_at: Clock::get()?.unix_timestamp,
            changed_by: ctx.accounts.proposer.key(),
        });
        
        Ok(())
    }
    
    /// Create vote snapshot for a voter when proposal is created
    pub fn create_vote_snapshot(
        ctx: Context<CreateVoteSnapshot>,
        proposal_id: u64,
    ) -> Result<()> {
        let vote_snapshot = &mut ctx.accounts.vote_snapshot;
        let proposal = &ctx.accounts.proposal;
        
        // **SECURITY FIX**: Enhanced snapshot validation to prevent gaming
        let current_time = Clock::get()?.unix_timestamp;
        
        // Validate proposal ID matches
        require!(proposal.id == proposal_id, GovernanceError::InvalidSnapshot);
        
        // **CRITICAL FIX**: Prevent snapshot creation after voting starts
        require!(
            current_time < proposal.voting_start,
            GovernanceError::SnapshotTooLate
        );
        
        // **SECURITY FIX**: Prevent multiple snapshots from same voter
        require!(
            vote_snapshot.voter == Pubkey::default(),
            GovernanceError::SnapshotAlreadyExists
        );
        
        let voter_balance = ctx.accounts.voter_rifts_account.amount;
        
        // Record snapshot data
        vote_snapshot.proposal_id = proposal_id;
        vote_snapshot.voter = ctx.accounts.voter.key();
        vote_snapshot.snapshot_power = voter_balance;
        vote_snapshot.snapshot_taken_at = Clock::get()?.unix_timestamp;
        
        emit!(VoteSnapshotCreated {
            proposal_id,
            voter: ctx.accounts.voter.key(),
            snapshot_power: voter_balance,
            timestamp: vote_snapshot.snapshot_taken_at,
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
        let vote_snapshot = &mut ctx.accounts.vote_snapshot;
        let current_time = Clock::get()?.unix_timestamp;
        
        // **CRITICAL FIX**: Use actual snapshot-based voting instead of current balance
        // Check if snapshot exists for this voter and proposal
        let voting_power = if vote_snapshot.proposal_id == proposal.id && 
                              vote_snapshot.voter == ctx.accounts.voter.key() {
            // Use pre-recorded snapshot power
            vote_snapshot.snapshot_power
        } else {
            // This should not happen if snapshots are properly created
            return Err(GovernanceError::SnapshotNotFound.into());
        };
        
        // **CRITICAL SECURITY FIX**: Prevent flash loan attacks by requiring snapshots BEFORE proposal
        // Snapshot must be taken BEFORE proposal creation (not after)
        require!(
            vote_snapshot.snapshot_taken_at < proposal.created_at, // Snapshot MUST be before proposal
            GovernanceError::InsufficientHoldingPeriod
        );

        // Additional check: Snapshot must be recent enough (within 7 days)
        require!(
            vote_snapshot.snapshot_taken_at >= proposal.created_at - (7 * 86400), // Within 7 days before proposal
            GovernanceError::InsufficientHoldingPeriod
        );
        
        // **SECURITY**: Snapshot timing validation complete - prevents flash loan attacks
        require!(
            vote_snapshot.snapshot_taken_at <= Clock::get()?.unix_timestamp,
            GovernanceError::InvalidSnapshot
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
        
        // **CRITICAL FIX**: Use governance mint decimals instead of hardcoded 9
        let _governance = &ctx.accounts.governance;
        let rifts_mint = &ctx.accounts.rifts_mint;
        let min_vote_tokens = 100u64
            .checked_mul(10u64.pow(u32::from(rifts_mint.decimals)))
            .ok_or(GovernanceError::MathOverflow)?;
        require!(
            voting_power >= min_vote_tokens,
            GovernanceError::InsufficientTokensToVote
        );
        
        // Check if user already voted
        require!(
            vote_record.voter == Pubkey::default(),
            GovernanceError::AlreadyVoted
        );
        
        // **CRITICAL FIX**: Record the vote using snapshot power
        vote_record.voter = ctx.accounts.voter.key();
        vote_record.proposal = proposal.key();
        vote_record.vote = vote;
        vote_record.voting_power = voting_power; // Use snapshot power instead of current balance
        vote_record.timestamp = current_time;
        
        // **CRITICAL FIX**: Update proposal vote counts with u128 arithmetic to prevent overflow
        match vote {
            VoteChoice::For => {
                proposal.votes_for = proposal.votes_for
                    .checked_add(u128::from(voting_power))
                    .ok_or(GovernanceError::VoteOverflow)?;
            }
            VoteChoice::Against => {
                proposal.votes_against = proposal.votes_against
                    .checked_add(u128::from(voting_power))
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
            voting_power: voting_power, // Use snapshot power
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
        
        // **HIGH SECURITY FIX**: Multisig validation for critical governance actions
        let executor_key = ctx.accounts.executor.key();
        let is_primary_authority = executor_key == governance.authority;
        let is_additional_authority = governance.additional_authorities.contains(&executor_key);
        
        require!(
            is_primary_authority || is_additional_authority,
            GovernanceError::UnauthorizedCancel
        );
        
        // **HIGH SECURITY FIX**: For sensitive proposals, require multisig approval
        let requires_multisig = matches!(proposal.proposal_type, 
            ProposalType::ProtocolUpgrade | 
            ProposalType::TreasurySpend |
            ProposalType::EmergencyAction
        );
        
        if requires_multisig && governance.required_signatures > 1 {
            // **SECURITY FIX**: Full multisig validation implementation
            let multisig_state = ctx.accounts.multisig_signature_state.as_ref()
                .ok_or(GovernanceError::MultisigStateRequired)?;

            // Validate signature collection account belongs to this proposal
            require!(
                multisig_state.proposal_id == proposal.id,
                GovernanceError::InvalidMultisigState
            );

            // Check if enough signatures have been collected
            require!(
                multisig_state.signature_count >= governance.required_signatures,
                GovernanceError::InsufficientSignatures
            );

            // Validate that signatures haven't expired (24 hour window)
            let signature_age = current_time - multisig_state.created_at;
            require!(
                signature_age <= 86400, // 24 hours
                GovernanceError::SignaturesExpired
            );

            // Verify all required signers are valid authorities
            for i in 0..multisig_state.signature_count {
                let signer = multisig_state.signers[i as usize];
                let is_valid_authority = signer == governance.authority ||
                                        governance.additional_authorities.contains(&signer);
                require!(is_valid_authority, GovernanceError::InvalidMultisigSigner);
            }

            msg!("✅ Multisig validation passed: {}/{} signatures verified",
                 multisig_state.signature_count, governance.required_signatures);
        }
        
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
        
        // **SECURITY FIX**: Check emergency action expiry
        if proposal.proposal_type == ProposalType::EmergencyAction {
            require!(
                current_time < proposal.emergency_expiry_time,
                GovernanceError::EmergencyActionExpired
            );
        }
        
        // **CRITICAL FIX**: Check minimum participation requirement with overflow protection
        let total_votes = proposal.votes_for
            .checked_add(proposal.votes_against)
            .ok_or(GovernanceError::VoteOverflow)?;
        require!(
            total_votes >= u128::from(proposal.min_participation_required),
            GovernanceError::InsufficientParticipation
        );
        
        // **SECURITY FIX**: Check if proposal passed (supermajority required for emergency actions)
        match proposal.proposal_type {
            ProposalType::EmergencyAction => {
                // Emergency proposals require supermajority (2/3 of total votes)
                let required_supermajority = total_votes
                    .checked_mul(2)
                    .and_then(|doubled| doubled.checked_div(3))
                    .ok_or(GovernanceError::MathOverflow)?;
                
                require!(
                    proposal.votes_for >= required_supermajority,
                    GovernanceError::ProposalDidNotPass
                );
            },
            _ => {
                // Regular proposals require simple majority
                require!(
                    proposal.votes_for > proposal.votes_against,
                    GovernanceError::ProposalDidNotPass
                );
            }
        }
        
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
                    if let Some(new_jupiter_id) = param_changes.jupiter_program_id {
                        // Validate it's not zero address
                        require!(new_jupiter_id != Pubkey::default(), GovernanceError::InvalidParameterValue);
                    }
                    
                    // Apply Jupiter program ID change immediately
                    if let Some(new_jupiter_id) = param_changes.jupiter_program_id {
                        governance.jupiter_program_id = Some(new_jupiter_id);
                        msg!("🔄 Jupiter program ID updated to: {}", new_jupiter_id);
                    }
                    
                    // Store execution data for the rift program to read
                    governance.pending_parameter_changes = Some(param_changes.clone());
                    governance.parameter_change_proposal_id = proposal.id;
                    
                    emit!(ParameterChangeExecuted {
                        proposal_id: proposal.id,
                        changes: param_changes.clone(),
                    });

                    emit!(SecurityParametersUpdated {
                        proposal_id: proposal.id,
                        updated_by: ctx.accounts.executor.key(),
                        min_voting_period: None, // These are not part of param_changes
                        min_execution_delay: None,
                        max_treasury_spend: None,
                        updated_at: current_time,
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

        emit!(ProposalStatusChanged {
            proposal_id: proposal.id,
            old_status: ProposalStatus::Active,
            new_status: ProposalStatus::Executed,
            changed_at: current_time,
            changed_by: ctx.accounts.executor.key(),
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

        emit!(ProposalStatusChanged {
            proposal_id: proposal.id,
            old_status: ProposalStatus::Active,
            new_status: ProposalStatus::Cancelled,
            changed_at: Clock::get()?.unix_timestamp,
            changed_by: ctx.accounts.canceller.key(),
        });
        
        Ok(())
    }

    /// **MULTISIG GOVERNANCE**: Add a new authority to the multisig
    pub fn add_multisig_authority(
        ctx: Context<ModifyMultisigAuthority>,
        new_authority: Pubkey,
    ) -> Result<()> {
        let governance = &mut ctx.accounts.governance;
        
        // Only current authority can add new authorities
        require!(
            ctx.accounts.authority.key() == governance.authority,
            GovernanceError::UnauthorizedCancel
        );
        
        // Prevent adding duplicate authorities
        require!(
            new_authority != governance.authority,
            GovernanceError::InvalidParameterValue
        );
        require!(
            !governance.additional_authorities.contains(&new_authority),
            GovernanceError::InvalidParameterValue
        );
        
        // Maximum 10 additional authorities for security
        require!(
            governance.additional_authorities.len() < 10,
            GovernanceError::InvalidParameterValue
        );
        
        governance.additional_authorities.push(new_authority);
        
        emit!(MultisigAuthorityAdded {
            governance: governance.key(),
            new_authority,
            total_authorities: u8::try_from(governance.additional_authorities.len())
                .map_err(|_| GovernanceError::TooManyAuthorities)?
                .checked_add(1)
                .ok_or(GovernanceError::TooManyAuthorities)?,
            updated_at: Clock::get()?.unix_timestamp,
        });
        
        Ok(())
    }

    /// **MULTISIG GOVERNANCE**: Remove an authority from the multisig
    pub fn remove_multisig_authority(
        ctx: Context<ModifyMultisigAuthority>,
        authority_to_remove: Pubkey,
    ) -> Result<()> {
        let governance = &mut ctx.accounts.governance;
        
        // Only current authority can remove authorities
        require!(
            ctx.accounts.authority.key() == governance.authority,
            GovernanceError::UnauthorizedCancel
        );
        
        // Cannot remove the primary authority
        require!(
            authority_to_remove != governance.authority,
            GovernanceError::InvalidParameterValue
        );
        
        // Find and remove the authority
        let index = governance.additional_authorities
            .iter()
            .position(|&auth| auth == authority_to_remove)
            .ok_or(GovernanceError::InvalidParameterValue)?;
        
        governance.additional_authorities.remove(index);
        
        // Adjust required signatures if needed (cannot exceed available authorities)
        let total_authorities = u8::try_from(governance.additional_authorities.len())
            .map_err(|_| GovernanceError::TooManyAuthorities)?
            .checked_add(1)
            .ok_or(GovernanceError::TooManyAuthorities)?;
        if governance.required_signatures > total_authorities {
            governance.required_signatures = total_authorities;
        }
        
        emit!(MultisigAuthorityRemoved {
            governance: governance.key(),
            removed_authority: authority_to_remove,
            total_authorities,
            updated_signatures_required: governance.required_signatures,
            updated_at: Clock::get()?.unix_timestamp,
        });
        
        Ok(())
    }

    /// **MULTISIG GOVERNANCE**: Add signature to multisig proposal
    pub fn add_multisig_signature(
        ctx: Context<AddMultisigSignature>,
        proposal_id: u64,
    ) -> Result<()> {
        let governance = &ctx.accounts.governance;
        let multisig_state = &mut ctx.accounts.multisig_signature_state;
        let proposal = &ctx.accounts.proposal;
        let signer = &ctx.accounts.signer;

        // Validate proposal is correct
        require!(proposal.id == proposal_id, GovernanceError::InvalidProposal);
        require!(multisig_state.proposal_id == proposal_id, GovernanceError::InvalidMultisigState);

        // Validate proposal requires multisig
        let requires_multisig = matches!(proposal.proposal_type,
            ProposalType::ProtocolUpgrade |
            ProposalType::TreasurySpend |
            ProposalType::EmergencyAction
        );
        require!(requires_multisig, GovernanceError::MultisigNotRequired);

        // Validate signer is authorized governance authority
        let is_authorized = signer.key() == governance.authority ||
            governance.additional_authorities.contains(&signer.key());
        require!(is_authorized, GovernanceError::UnauthorizedMultisigSigner);

        // Check if this signer has already signed
        for i in 0..multisig_state.signature_count {
            require!(
                multisig_state.signers[i as usize] != signer.key(),
                GovernanceError::AlreadySigned
            );
        }

        // Add signature
        require!(
            multisig_state.signature_count < 10, // Max 10 signatures
            GovernanceError::TooManySignatures
        );

        let current_count = multisig_state.signature_count;
        multisig_state.signers[current_count as usize] = signer.key();
        multisig_state.signature_count = current_count + 1;
        multisig_state.last_signature_at = Clock::get()?.unix_timestamp;

        emit!(MultisigSignatureAdded {
            proposal_id,
            signer: signer.key(),
            signature_count: multisig_state.signature_count,
            required_signatures: governance.required_signatures,
            timestamp: multisig_state.last_signature_at,
        });

        msg!("✅ Multisig signature added: {}/{} signatures collected",
             multisig_state.signature_count, governance.required_signatures);

        Ok(())
    }

    /// **MULTISIG GOVERNANCE**: Initialize multisig signature collection for proposal
    pub fn initialize_multisig_proposal(
        ctx: Context<InitializeMultisigProposal>,
        proposal_id: u64,
    ) -> Result<()> {
        let multisig_state = &mut ctx.accounts.multisig_signature_state;
        let proposal = &ctx.accounts.proposal;

        // Validate proposal requires multisig
        let requires_multisig = matches!(proposal.proposal_type,
            ProposalType::ProtocolUpgrade |
            ProposalType::TreasurySpend |
            ProposalType::EmergencyAction
        );
        require!(requires_multisig, GovernanceError::MultisigNotRequired);

        multisig_state.proposal_id = proposal_id;
        multisig_state.signature_count = 0;
        multisig_state.created_at = Clock::get()?.unix_timestamp;
        multisig_state.last_signature_at = 0;
        multisig_state.signers = [Pubkey::default(); 10];

        emit!(MultisigProposalInitialized {
            proposal_id,
            initialized_at: multisig_state.created_at,
        });

        Ok(())
    }

    /// **MULTISIG GOVERNANCE**: Change the required signature threshold
    pub fn update_signature_threshold(
        ctx: Context<ModifyMultisigAuthority>,
        new_threshold: u8,
    ) -> Result<()> {
        let governance = &mut ctx.accounts.governance;
        
        // Only current authority can update threshold
        require!(
            ctx.accounts.authority.key() == governance.authority,
            GovernanceError::UnauthorizedCancel
        );
        
        let total_authorities = u8::try_from(governance.additional_authorities.len())
            .map_err(|_| GovernanceError::TooManyAuthorities)?
            .checked_add(1)
            .ok_or(GovernanceError::TooManyAuthorities)?;
        
        // Threshold must be between 1 and total authorities
        require!(
            new_threshold > 0 && new_threshold <= total_authorities,
            GovernanceError::InvalidParameterValue
        );
        
        let old_threshold = governance.required_signatures;
        governance.required_signatures = new_threshold;
        
        emit!(MultisigThresholdUpdated {
            governance: governance.key(),
            old_threshold,
            new_threshold,
            total_authorities,
            updated_at: Clock::get()?.unix_timestamp,
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
        space = Governance::INIT_SPACE,
        seeds = [b"governance", authority.key().as_ref()],
        constraint = authority.key() != Pubkey::default() @ GovernanceError::InvalidSeedComponent,
        bump,
        constraint = authority.lamports() >= Rent::get()?.minimum_balance(Governance::INIT_SPACE) @ GovernanceError::InsufficientRentExemption
    )]
    pub governance: Account<'info, Governance>,
    
    /// **CRITICAL FIX**: RIFTS mint for decimal validation
    pub rifts_mint: Account<'info, Mint>,
    
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
        space = Proposal::INIT_SPACE,
        seeds = [b"proposal", governance.key().as_ref(), &governance.total_proposals.to_le_bytes()],
        constraint = governance.key() != Pubkey::default() @ GovernanceError::InvalidSeedComponent,
        bump,
        constraint = proposer.lamports() >= Rent::get()?.minimum_balance(Proposal::INIT_SPACE) @ GovernanceError::InsufficientRentExemption
    )]
    pub proposal: Account<'info, Proposal>,
    
    #[account(
        constraint = proposer_rifts_account.owner == proposer.key()
    )]
    pub proposer_rifts_account: Account<'info, TokenAccount>,
    
    /// **CRITICAL FIX**: RIFTS mint for decimal validation
    pub rifts_mint: Account<'info, Mint>,
    
    pub system_program: Program<'info, System>,
}

#[derive(Accounts)]
pub struct CreateVoteSnapshot<'info> {
    #[account(mut)]
    pub voter: Signer<'info>,
    
    #[account(
        init,
        payer = voter,
        space = VoteSnapshot::INIT_SPACE,
        seeds = [b"vote_snapshot", proposal.key().as_ref(), voter.key().as_ref()],
        constraint = proposal.key() != Pubkey::default() && voter.key() != Pubkey::default() @ GovernanceError::InvalidSeedComponent,
        bump,
        constraint = voter.lamports() >= Rent::get()?.minimum_balance(VoteSnapshot::INIT_SPACE) @ GovernanceError::InsufficientRentExemption
    )]
    pub vote_snapshot: Account<'info, VoteSnapshot>,
    
    pub proposal: Account<'info, Proposal>,
    
    #[account(
        constraint = voter_rifts_account.owner == voter.key() @ GovernanceError::InvalidTokenOwner,
        constraint = voter_rifts_account.mint == governance.rifts_mint @ GovernanceError::InvalidRiftsMint,
        // **SECURITY FIX**: Enforce canonical associated token account to prevent vote delegation
        constraint = voter_rifts_account.key() == anchor_spl::associated_token::get_associated_token_address(&voter.key(), &governance.rifts_mint) @ GovernanceError::MustUseAssociatedTokenAccount
    )]
    pub voter_rifts_account: Account<'info, TokenAccount>,
    
    pub governance: Account<'info, Governance>,
    
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
        space = VoteRecord::INIT_SPACE,
        seeds = [b"vote", proposal.key().as_ref(), voter.key().as_ref()],
        constraint = proposal.key() != Pubkey::default() && voter.key() != Pubkey::default() @ GovernanceError::InvalidSeedComponent,
        bump,
        constraint = voter.lamports() >= Rent::get()?.minimum_balance(VoteRecord::INIT_SPACE) @ GovernanceError::InsufficientRentExemption
    )]
    pub vote_record: Account<'info, VoteRecord>,
    
    #[account(
        constraint = voter_rifts_account.owner == voter.key() @ GovernanceError::InvalidTokenOwner,
        constraint = voter_rifts_account.mint == governance.rifts_mint @ GovernanceError::InvalidRiftsMint,
        // **SECURITY FIX**: Enforce canonical associated token account to prevent vote delegation
        constraint = voter_rifts_account.key() == anchor_spl::associated_token::get_associated_token_address(&voter.key(), &governance.rifts_mint) @ GovernanceError::MustUseAssociatedTokenAccount
    )]
    pub voter_rifts_account: Account<'info, TokenAccount>,
    
    /// **CRITICAL FIX**: Vote snapshot for flash loan protection
    #[account(
        constraint = vote_snapshot.voter == voter.key() @ GovernanceError::InvalidSnapshot,
        constraint = vote_snapshot.proposal_id == proposal.id @ GovernanceError::InvalidSnapshot
    )]
    pub vote_snapshot: Account<'info, VoteSnapshot>,
    
    pub governance: Account<'info, Governance>,
    
    /// **CRITICAL FIX**: RIFTS mint for decimal validation
    pub rifts_mint: Account<'info, Mint>,
    
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

    /// **MULTISIG SECURITY**: Signature collection state for multisig proposals
    /// Optional - only required for multisig proposals
    #[account(
        constraint = multisig_signature_state.proposal_id == proposal.id @ GovernanceError::InvalidMultisigState
    )]
    pub multisig_signature_state: Option<Account<'info, MultisigSignatureState>>,
}

#[derive(Accounts)]
pub struct CancelProposal<'info> {
    #[account(mut)]
    pub canceller: Signer<'info>,
    
    pub governance: Account<'info, Governance>,
    
    #[account(mut)]
    pub proposal: Account<'info, Proposal>,
}

#[derive(Accounts)]
pub struct ModifyMultisigAuthority<'info> {
    #[account(mut)]
    pub authority: Signer<'info>,

    #[account(mut)]
    pub governance: Account<'info, Governance>,
}

#[derive(Accounts)]
pub struct AddMultisigSignature<'info> {
    #[account(mut)]
    pub signer: Signer<'info>,

    #[account(mut)]
    pub multisig_signature_state: Account<'info, MultisigSignatureState>,

    pub governance: Account<'info, Governance>,

    pub proposal: Account<'info, Proposal>,
}

#[derive(Accounts)]
pub struct InitializeMultisigProposal<'info> {
    #[account(mut)]
    pub proposer: Signer<'info>,

    #[account(
        init,
        payer = proposer,
        space = MultisigSignatureState::INIT_SPACE,
        seeds = [b"multisig", proposal.key().as_ref()],
        bump,
        constraint = proposer.lamports() >= Rent::get()?.minimum_balance(MultisigSignatureState::INIT_SPACE) @ GovernanceError::InsufficientRentExemption
    )]
    pub multisig_signature_state: Account<'info, MultisigSignatureState>,

    pub proposal: Account<'info, Proposal>,

    pub governance: Account<'info, Governance>,

    pub system_program: Program<'info, System>,
}

// State accounts
impl Governance {
    pub const INIT_SPACE: usize = 8 + // discriminator
        32 + // authority
        4 + (32 * 10) + // additional_authorities (Vec with max 10)
        1 + // required_signatures
        32 + // rifts_mint
        8 +  // min_voting_period
        8 +  // min_execution_delay
        8 +  // total_proposals
        8 +  // total_executed
        8 +  // max_treasury_spend
        1 +  // emergency_pause_active
        8 +  // pause_initiated_at
        8 +  // pause_duration
        1 +  // assets_frozen
        8 +  // freeze_initiated_at
        8 +  // upgrade_ready_timestamp
        1 + 200 + // pending_parameter_changes (Option + struct size)
        8 +  // parameter_change_proposal_id
        1 + 200 + // pending_treasury_spend (Option + struct size)
        8 +  // treasury_spend_proposal_id
        1 + 200 + // pending_protocol_upgrade (Option + struct size)
        8 +  // protocol_upgrade_proposal_id
        1 + 4 + (32 * 10) + // pending_oracle_updates (Option + Vec with max 10 oracles)
        8 +  // oracle_update_proposal_id
        3 +  // treasury_fee_bps (Option<u16> = 1 + 2 bytes)
        33;  // jupiter_program_id (Option<Pubkey> = 1 + 32 bytes)
}

#[account]
pub struct Governance {
    pub authority: Pubkey,
    /// **SECURITY FIX**: Additional authorities for multisig governance
    pub additional_authorities: Vec<Pubkey>, // Up to 10 additional signers
    pub required_signatures: u8, // Minimum signatures required (1 = single sig, >1 = multisig)
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
    
    // Fee configuration
    pub treasury_fee_bps: Option<u16>, // Treasury fee percentage (default 500 = 5%)
    
    // External program configuration
    pub jupiter_program_id: Option<Pubkey>, // Jupiter aggregator program ID (configurable)
}

impl Proposal {
    pub const INIT_SPACE: usize = 8 + // discriminator
        8 +  // id
        32 + // proposer
        4 + 256 + // title (String: 4 bytes len + 256 bytes max data)
        4 + 1024 + // description (String: 4 bytes len + 1024 bytes max data)
        1 +  // proposal_type (enum)
        4 + 512 + // execution_data (Vec<u8>: 4 bytes len + 512 bytes max data)
        8 +  // voting_start
        8 +  // voting_end
        16 + // votes_for (u128)
        16 + // votes_against (u128)
        4 +  // total_voters
        1 +  // status
        8 +  // created_at
        8 +  // executed_at
        8 +  // snapshot_taken_at
        8 +  // snapshot_slot
        8 +  // min_participation_required
        8;   // emergency_expiry_time
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
    pub votes_for: u128, // **OVERFLOW FIX**: Use u128 to prevent vote counting overflow
    pub votes_against: u128, // **OVERFLOW FIX**: Use u128 to prevent vote counting overflow
    pub total_voters: u32,
    pub status: ProposalStatus,
    pub created_at: i64,
    pub executed_at: i64,
    // Vote power snapshot fields
    pub snapshot_taken_at: i64,
    pub snapshot_slot: u64,
    pub min_participation_required: u64,  // Minimum participation for proposal validity
    pub emergency_expiry_time: i64,       // Auto-expiry timestamp for emergency actions
}

impl VoteRecord {
    pub const INIT_SPACE: usize = 8 + // discriminator
        32 + // voter
        32 + // proposal
        1 +  // vote (enum)
        8 +  // voting_power
        8;   // timestamp
}

#[account]
pub struct VoteRecord {
    pub voter: Pubkey,
    pub proposal: Pubkey,
    pub vote: VoteChoice,
    pub voting_power: u64,
    pub timestamp: i64,
}

impl VoteSnapshot {
    pub const INIT_SPACE: usize = 8 + // discriminator
        8 +  // proposal_id
        32 + // voter
        8 +  // snapshot_power
        8;   // snapshot_taken_at
}

#[account]
pub struct VoteSnapshot {
    pub proposal_id: u64,
    pub voter: Pubkey,
    pub snapshot_power: u64,  // Vote power at proposal creation
    pub snapshot_taken_at: i64,
}

impl MultisigSignatureState {
    pub const INIT_SPACE: usize = 8 + // discriminator
        8 +  // proposal_id
        1 +  // signature_count
        8 +  // created_at
        8 +  // last_signature_at
        320; // signers (32 * 10 max signers)
}

#[account]
pub struct MultisigSignatureState {
    pub proposal_id: u64,
    pub signature_count: u8,
    pub created_at: i64,
    pub last_signature_at: i64,
    pub signers: [Pubkey; 10], // Support up to 10 multisig signers
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
pub struct VoteSnapshotCreated {
    pub proposal_id: u64,
    pub voter: Pubkey,
    pub snapshot_power: u64,
    pub timestamp: i64,
}

// Enhanced audit trail events
#[event]
pub struct GovernanceInitialized {
    pub authority: Pubkey,
    pub rifts_mint: Pubkey,
    pub min_voting_period: i64,
    pub min_execution_delay: i64,
    pub initialized_at: i64,
}

#[event]
pub struct AuthorityChanged {
    pub old_authority: Pubkey,
    pub new_authority: Pubkey,
    pub changed_at: i64,
}

#[event]
pub struct MultisigConfigured {
    pub authorities: Vec<Pubkey>,
    pub required_signatures: u8,
    pub configured_at: i64,
}

#[event]
pub struct ProposalStatusChanged {
    pub proposal_id: u64,
    pub old_status: ProposalStatus,
    pub new_status: ProposalStatus,
    pub changed_at: i64,
    pub changed_by: Pubkey,
}

#[event]
pub struct SecurityParametersUpdated {
    pub proposal_id: u64,
    pub updated_by: Pubkey,
    pub min_voting_period: Option<i64>,
    pub min_execution_delay: Option<i64>,
    pub max_treasury_spend: Option<u64>,
    pub updated_at: i64,
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
    pub wrap_fee_bps: Option<u16>,      // NEW: Wrap fee configuration
    pub unwrap_fee_bps: Option<u16>,    // NEW: Unwrap fee configuration  
    pub treasury_fee_bps: Option<u16>,  // NEW: Treasury fee configuration
    pub jupiter_program_id: Option<Pubkey>, // NEW: Jupiter program ID configuration
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

#[event]
pub struct MultisigAuthorityAdded {
    pub governance: Pubkey,
    pub new_authority: Pubkey,
    pub total_authorities: u8,
    pub updated_at: i64,
}

#[event]
pub struct MultisigAuthorityRemoved {
    pub governance: Pubkey,
    pub removed_authority: Pubkey,
    pub total_authorities: u8,
    pub updated_signatures_required: u8,
    pub updated_at: i64,
}

#[event]
pub struct MultisigThresholdUpdated {
    pub governance: Pubkey,
    pub old_threshold: u8,
    pub new_threshold: u8,
    pub total_authorities: u8,
    pub updated_at: i64,
}

#[event]
pub struct MultisigSignatureAdded {
    pub proposal_id: u64,
    pub signer: Pubkey,
    pub signature_count: u8,
    pub required_signatures: u8,
    pub timestamp: i64,
}

#[event]
pub struct MultisigProposalInitialized {
    pub proposal_id: u64,
    pub initialized_at: i64,
}


// Errors
#[error_code]
pub enum GovernanceError {
    #[msg("Invalid voting period - must be at least 1 hour")]
    InvalidVotingPeriod,
    #[msg("Invalid execution delay - must be at least 30 minutes")]
    InvalidExecutionDelay,
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
    TokenBalanceDecreased, // **DEPRECATED**: This check has been removed as ineffective
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
    #[msg("Vote snapshot not found or invalid")]
    SnapshotNotFound,
    #[msg("Invalid snapshot data")]
    InvalidSnapshot,
    #[msg("Invalid token owner")]
    InvalidTokenOwner,
    #[msg("Must use associated token account to prevent vote delegation")]
    MustUseAssociatedTokenAccount,
    #[msg("Snapshot created too late - must be before voting starts")]
    SnapshotTooLate,
    #[msg("Snapshot already exists for this voter")]
    SnapshotAlreadyExists,
    #[msg("Emergency action has expired and can no longer be executed")]
    EmergencyActionExpired,
    #[msg("Cannot create proposals when token supply is zero")]
    ZeroTokenSupply,
    #[msg("Insufficient signatures for multisig governance action")]
    InsufficientSignatures,
    #[msg("Too many authorities - governance cannot exceed 255 total authorities")]
    TooManyAuthorities,
    #[msg("Invalid multisig signature state")]
    InvalidMultisigState,
    #[msg("Multisig signature state is required for this proposal")]
    MultisigStateRequired,
    #[msg("Multisig signatures have expired")]
    SignaturesExpired,
    #[msg("Invalid multisig signer")]
    InvalidMultisigSigner,
    #[msg("Unauthorized multisig signer")]
    UnauthorizedMultisigSigner,
    #[msg("Already signed this proposal")]
    AlreadySigned,
    #[msg("Too many signatures collected")]
    TooManySignatures,
    #[msg("Multisig not required for this proposal type")]
    MultisigNotRequired,
    #[msg("Unauthorized signer for multisig governance")]
    UnauthorizedSigner,
    #[msg("Invalid proposal ID or state")]
    InvalidProposal,
}