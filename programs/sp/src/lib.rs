use anchor_lang::prelude::*;
use anchor_lang::system_program::{transfer, Transfer};

declare_id!("5C73TX7gSriX7MXzMAWNiKDU5ZRBcEx6yAQ2KaGKzeLW");

// Program constants
const MAX_AD_ID_LENGTH: usize = 32;
const MAX_AD_URL_LENGTH: usize = 200;
const MAX_AD_CONTENT_LENGTH: usize = 500;
const DEFAULT_FEE_PER_AD: u64 = 5_000; // 0.005 SOL
const MIN_AD_REWARD: u64 = 1_000; // 0.001 SOL
const TRANSACTION_TIMEOUT: i64 = 300; // 5 minutes
const MAX_SINGLE_DEPOSIT: u64 = 10_000_000_000; // 10 SOL
const BASE_TRANSACTION_FEE: u64 = 5_000; // Base fee in lamports (0.005 SOL)
const MIN_AD_VIEW_TIME: i64 = 5; // Minimum 5 seconds to view ad

#[program]
pub mod fee_payment_dapp {
    use super::*;

    /// Initialize program with deployer as admin
    pub fn initialize(ctx: Context<Initialize>) -> Result<()> {
        let state = &mut ctx.accounts.state;
        
        state.admin = ctx.accounts.deployer.key();
        state.total_funds = 0;
        state.total_ads_viewed = 0;
        state.total_transactions = 0;
        state.fee_per_ad = DEFAULT_FEE_PER_AD;
        state.base_transaction_fee = BASE_TRANSACTION_FEE;
        state.is_paused = false;
        state.bump = ctx.bumps.state;

        emit!(ProgramInitialized {
            admin: state.admin,
            timestamp: Clock::get()?.unix_timestamp,
        });

        Ok(())
    }

    /// Admin deposits funds into the program for gas fee sponsorship
    pub fn deposit_funds(ctx: Context<DepositFunds>, amount: u64) -> Result<()> {
        require!(!ctx.accounts.state.is_paused, FeePaymentError::ProgramPaused);
        require!(amount > 0 && amount <= MAX_SINGLE_DEPOSIT, FeePaymentError::InvalidAmount);

        // Do the transfer first before borrowing state mutably
        transfer(
            CpiContext::new(
                ctx.accounts.system_program.to_account_info(),
                Transfer {
                    from: ctx.accounts.admin.to_account_info(),
                    to: ctx.accounts.state.to_account_info(),
                },
            ),
            amount,
        )?;

        // Now borrow state mutably to update it
        let state = &mut ctx.accounts.state;
        state.total_funds = state.total_funds
            .checked_add(amount)
            .ok_or(FeePaymentError::MathOverflow)?;

        emit!(FundsDeposited {
            admin: ctx.accounts.admin.key(),
            amount,
            total_funds: state.total_funds,
        });

        Ok(())
    }

    /// Create a new advertisement with content for popup display
    pub fn create_ad(
        ctx: Context<CreateAd>,
        ad_id: String,
        ad_url: String,
        ad_content: String,
        reward_amount: u64,
        display_duration: i64,
    ) -> Result<()> {
        require!(!ctx.accounts.state.is_paused, FeePaymentError::ProgramPaused);
        require!(
            !ad_id.is_empty() && ad_id.len() <= MAX_AD_ID_LENGTH,
            FeePaymentError::InvalidAdId
        );
        require!(
            ad_url.starts_with("https://") && ad_url.len() <= MAX_AD_URL_LENGTH,
            FeePaymentError::InvalidAdUrl
        );
        require!(
            !ad_content.is_empty() && ad_content.len() <= MAX_AD_CONTENT_LENGTH,
            FeePaymentError::InvalidAdContent
        );
        require!(reward_amount >= MIN_AD_REWARD, FeePaymentError::RewardTooLow);
        require!(display_duration >= MIN_AD_VIEW_TIME, FeePaymentError::InvalidDisplayTime);

        let ad = &mut ctx.accounts.ad;
        let clock = Clock::get()?;

        ad.id = ad_id.clone();
        ad.url = ad_url.clone();
        ad.content = ad_content.clone();
        ad.reward_amount = reward_amount;
        ad.display_duration = display_duration;
        ad.is_active = true;
        ad.view_count = 0;
        ad.created_at = clock.unix_timestamp;
        ad.bump = ctx.bumps.ad;

        emit!(AdCreated {
            ad_id,
            reward_amount,
            display_duration,
            creator: ctx.accounts.admin.key(),
            timestamp: clock.unix_timestamp,
        });

        Ok(())
    }
    /// Toggle advertisement status
    pub fn toggle_ad(ctx: Context<ToggleAd>) -> Result<()> {
        let ad = &mut ctx.accounts.ad;
        ad.is_active = !ad.is_active;

        emit!(AdToggled {
            ad_id: ad.id.clone(),
            is_active: ad.is_active,
        });

        Ok(())
    }

    /// STEP 1: User initiates send transaction - gets available ad for viewing
    pub fn initiate_send_transaction(
        ctx: Context<InitiateSend>,
        recipient: Pubkey,
        amount: u64,
    ) -> Result<()> {
        require!(!ctx.accounts.state.is_paused, FeePaymentError::ProgramPaused);
        require!(recipient != Pubkey::default(), FeePaymentError::InvalidRecipient);
        require!(amount > 0, FeePaymentError::InvalidAmount);

        let state = &ctx.accounts.state;
        let calculated_fee = calculate_gas_fee(amount, state);
        
        require!(
            state.total_funds >= calculated_fee,
            FeePaymentError::InsufficientProgramFunds
        );

        let request = &mut ctx.accounts.request;
        let ad = &ctx.accounts.selected_ad;
        let clock = Clock::get()?;

        // Validate ad is active
        require!(ad.is_active, FeePaymentError::AdNotActive);

        request.user = ctx.accounts.user.key();
        request.recipient = recipient;
        request.amount = amount;
        request.calculated_fee = calculated_fee;
        request.status = RequestStatus::WaitingForAd;
        request.selected_ad_id = ad.id.clone();
        request.ad_display_started_at = Some(clock.unix_timestamp);
        request.created_at = clock.unix_timestamp;
        request.expires_at = clock.unix_timestamp + TRANSACTION_TIMEOUT;
        request.bump = ctx.bumps.request;

        // Emit event with ad content for frontend to display
        emit!(TransactionInitiated {
            user: request.user,
            recipient,
            amount,
            calculated_fee,
            ad_id: ad.id.clone(),
            ad_content: ad.content.clone(),
            ad_url: ad.url.clone(),
            display_duration: ad.display_duration,
            request_id: request.key(),
        });

        Ok(())
    }

    /// STEP 2: Complete transaction - Program sponsors gas fee separately
    /// User amount goes directly to recipient, program pays gas fee separately
    pub fn complete_transaction_after_ad(
        ctx: Context<CompleteTransaction>,
        view_duration: i64,
    ) -> Result<()> {
        let request = &mut ctx.accounts.request;
        let ad = &mut ctx.accounts.ad;
        let clock = Clock::get()?;

        // Validate request state
        require!(
            request.status == RequestStatus::WaitingForAd,
            FeePaymentError::InvalidStatus
        );
        require!(
            clock.unix_timestamp <= request.expires_at,
            FeePaymentError::RequestExpired
        );
        require!(
            request.selected_ad_id == ad.id,
            FeePaymentError::AdMismatch
        );

        // Validate ad viewing time
        let ad_started_at = request.ad_display_started_at.ok_or(FeePaymentError::AdNotStarted)?;
        let actual_view_time = clock.unix_timestamp - ad_started_at;
        require!(
            view_duration >= ad.display_duration && actual_view_time >= ad.display_duration,
            FeePaymentError::InsufficientViewTime
        );

        // Get values before mutable borrowing
        let user_amount = request.amount;
        let gas_fee = request.calculated_fee;
        let state_bump = ctx.accounts.state.bump;
        
        // Validate sufficient program funds for gas fee sponsorship
        require!(
            ctx.accounts.state.total_funds >= gas_fee,
            FeePaymentError::InsufficientProgramFunds
        );
        
        msg!("Executing gas fee sponsorship transaction...");
        msg!("User sends: {} lamports", user_amount);
        msg!("Recipient receives: {} lamports (exact same amount)", user_amount);
        msg!("Program sponsors gas fee: {} lamports", gas_fee);
        // CORRECTED TRANSACTION FLOW:
        // Transfer 1: User → Recipient (exact amount, no gas fee added)
        transfer(
            CpiContext::new(
                ctx.accounts.system_program.to_account_info(),
                Transfer {
                    from: ctx.accounts.user.to_account_info(),
                    to: ctx.accounts.recipient.to_account_info(),
                },
            ),
            user_amount, // Recipient gets exactly what user sent
        )?;

        // Transfer 2: Program → Fee account (gas fee sponsorship)
        // This is where the program pays the gas fee as a separate cost
        let signer_seeds = &[b"state".as_ref(), &[state_bump]];
        
        transfer(
            CpiContext::new_with_signer(
                ctx.accounts.system_program.to_account_info(),
                Transfer {
                    from: ctx.accounts.state.to_account_info(),
                    to: ctx.accounts.fee_account.to_account_info(),
                },
                &[signer_seeds],
            ),
            gas_fee, // Program pays gas fee separately
        )?;

        // Now update program state - only deduct the gas fee we sponsored
        let state = &mut ctx.accounts.state;
        state.total_funds = state.total_funds
            .checked_sub(gas_fee)
            .ok_or(FeePaymentError::MathUnderflow)?;
        
        // Update counters
        ad.view_count = ad.view_count
            .checked_add(1)
            .ok_or(FeePaymentError::MathOverflow)?;
        
        state.total_ads_viewed = state.total_ads_viewed
            .checked_add(1)
            .ok_or(FeePaymentError::MathOverflow)?;
        
        state.total_transactions = state.total_transactions
            .checked_add(1)
            .ok_or(FeePaymentError::MathOverflow)?;

        // Mark request as completed
        request.status = RequestStatus::Completed;
        request.completed_at = Some(clock.unix_timestamp);
        request.ad_view_duration = Some(view_duration);

        emit!(TransactionCompleted {
            user: request.user,
            recipient: request.recipient,
            amount_sent: user_amount,
            amount_received: user_amount, // Same as sent!
            gas_fee_sponsored: gas_fee,
            ad_id: ad.id.clone(),
            view_duration,
            timestamp: clock.unix_timestamp,
        });

        msg!("✅ Transaction completed with gas fee sponsorship!");
        msg!("User sent: {} lamports", user_amount);
        msg!("Recipient received: {} lamports", user_amount);
        msg!("Program sponsored: {} lamports", gas_fee);

        Ok(())
    }

    /// Get a random active ad for popup display
    pub fn get_random_ad(ctx: Context<GetRandomAd>) -> Result<()> {
        let ad = &ctx.accounts.ad;
        require!(ad.is_active, FeePaymentError::AdNotActive);

        emit!(AdRetrieved {
            ad_id: ad.id.clone(),
            ad_content: ad.content.clone(),
            ad_url: ad.url.clone(),
            display_duration: ad.display_duration,
            reward_amount: ad.reward_amount,
        });

        Ok(())
    }

    /// Cancel a pending request
    pub fn cancel_request(ctx: Context<CancelRequest>) -> Result<()> {
        let request = &mut ctx.accounts.request;
        
        require!(
            request.status == RequestStatus::WaitingForAd,
            FeePaymentError::InvalidStatus
        );

        request.status = RequestStatus::Cancelled;
        request.cancelled_at = Some(Clock::get()?.unix_timestamp);

        emit!(RequestCancelled {
            user: request.user,
        });

        Ok(())
    }

    /// Admin function to update base transaction fee
    pub fn update_base_fee(ctx: Context<AdminAction>, new_base_fee: u64) -> Result<()> {
        require!(new_base_fee > 0, FeePaymentError::InvalidAmount);
        
        let state = &mut ctx.accounts.state;
        let old_fee = state.base_transaction_fee;
        state.base_transaction_fee = new_base_fee;
        emit!(BaseFeeUpdated {
            old_fee,
            new_fee: new_base_fee,
            admin: ctx.accounts.admin.key(),
        });

        Ok(())
    }

    /// Admin functions
    pub fn toggle_pause(ctx: Context<AdminAction>) -> Result<()> {
        let state = &mut ctx.accounts.state;
        state.is_paused = !state.is_paused;

        emit!(ProgramToggled {
            is_paused: state.is_paused,
            admin: ctx.accounts.admin.key(),
        });

        Ok(())
    }

    pub fn withdraw_funds(ctx: Context<WithdrawFunds>, amount: u64) -> Result<()> {
        require!(amount > 0, FeePaymentError::InvalidAmount);
        
        // Get values before mutable borrowing
        let state_bump = ctx.accounts.state.bump;
        
        require!(ctx.accounts.state.total_funds >= amount, FeePaymentError::InsufficientProgramFunds);

        let signer_seeds = &[b"state".as_ref(), &[state_bump]];
        
        transfer(
            CpiContext::new_with_signer(
                ctx.accounts.system_program.to_account_info(),
                Transfer {
                    from: ctx.accounts.state.to_account_info(),
                    to: ctx.accounts.admin.to_account_info(),
                },
                &[signer_seeds],
            ),
            amount,
        )?;

        // Now update state
        let state = &mut ctx.accounts.state;
        state.total_funds = state.total_funds
            .checked_sub(amount)
            .ok_or(FeePaymentError::MathUnderflow)?;

        emit!(FundsWithdrawn {
            amount,
            admin: ctx.accounts.admin.key(),
            remaining: state.total_funds,
        });

        Ok(())
    }
}

/// Calculate gas fee for transaction - moved outside the impl block
fn calculate_gas_fee(amount: u64, state: &ProgramState) -> u64 {
    // Simple calculation: base fee + percentage of amount
    let percentage_fee = amount / 1000; // 0.1% of amount
    state.base_transaction_fee + percentage_fee
}

// Account Structures
#[account]
pub struct ProgramState {
    pub admin: Pubkey,                  // 32
    pub total_funds: u64,               // 8
    pub total_ads_viewed: u64,          // 8
    pub total_transactions: u64,        // 8
    pub fee_per_ad: u64,               // 8
    pub base_transaction_fee: u64,      // 8
    pub is_paused: bool,               // 1
    pub bump: u8,                      // 1
}                                      // Total: 74 bytes

#[account]
pub struct Advertisement {
    pub id: String,                 // 4 + 32
    pub url: String,                // 4 + 200
    pub content: String,            // 4 + 500
    pub reward_amount: u64,         // 8
    pub display_duration: i64,      // 8
    pub is_active: bool,           // 1
    pub view_count: u64,           // 8
    pub created_at: i64,           // 8
    pub bump: u8,                  // 1
}                                  // Total: 774 bytes

#[account]
pub struct TransactionRequest {
    pub user: Pubkey,                    // 32
    pub recipient: Pubkey,               // 32
    pub amount: u64,                     // 8
    pub calculated_fee: u64,             // 8
    pub status: RequestStatus,           // 1 + 1
    pub selected_ad_id: String,          // 4 + 32
    pub created_at: i64,                 // 8
    pub expires_at: i64,                 // 8
    pub ad_display_started_at: Option<i64>, // 1 + 8
    pub completed_at: Option<i64>,       // 1 + 8
    pub cancelled_at: Option<i64>,       // 1 + 8
    pub ad_view_duration: Option<i64>,   // 1 + 8
    pub bump: u8,                        // 1
}                                        // Total: 162 bytes

#[derive(AnchorSerialize, AnchorDeserialize, Clone, PartialEq, Eq)]
pub enum RequestStatus {
    WaitingForAd,
    Completed,
    Cancelled,
}

// Events
#[event]
pub struct ProgramInitialized {
    pub admin: Pubkey,
    pub timestamp: i64,
}

#[event]
pub struct FundsDeposited {
    pub admin: Pubkey,
    pub amount: u64,
    pub total_funds: u64,
}
#[event]
pub struct AdCreated {
    pub ad_id: String,
    pub reward_amount: u64,
    pub display_duration: i64,
    pub creator: Pubkey,
    pub timestamp: i64,
}

#[event]
pub struct AdToggled {
    pub ad_id: String,
    pub is_active: bool,
}

#[event]
pub struct TransactionInitiated {
    pub user: Pubkey,
    pub recipient: Pubkey,
    pub amount: u64,
    pub calculated_fee: u64,
    pub ad_id: String,
    pub ad_content: String,
    pub ad_url: String,
    pub display_duration: i64,
    pub request_id: Pubkey,
}

#[event]
pub struct TransactionCompleted {
    pub user: Pubkey,
    pub recipient: Pubkey,
    pub amount_sent: u64,           // What user sent
    pub amount_received: u64,       // What recipient received (same as sent)
    pub gas_fee_sponsored: u64,     // What program paid as sponsorship
    pub ad_id: String,
    pub view_duration: i64,
    pub timestamp: i64,
}

#[event]
pub struct AdRetrieved {
    pub ad_id: String,
    pub ad_content: String,
    pub ad_url: String,
    pub display_duration: i64,
    pub reward_amount: u64,
}

#[event]
pub struct RequestCancelled {
    pub user: Pubkey,
}

#[event]
pub struct BaseFeeUpdated {
    pub old_fee: u64,
    pub new_fee: u64,
    pub admin: Pubkey,
}

#[event]
pub struct ProgramToggled {
    pub is_paused: bool,
    pub admin: Pubkey,
}

#[event]
pub struct FundsWithdrawn {
    pub amount: u64,
    pub admin: Pubkey,
    pub remaining: u64,
}

// Context Definitions
#[derive(Accounts)]
pub struct Initialize<'info> {
    #[account(
        init,
        payer = deployer,
        space = 8 + 74,
        seeds = [b"state"],
        bump
    )]
    pub state: Account<'info, ProgramState>,
    #[account(mut)]
    pub deployer: Signer<'info>,
    pub system_program: Program<'info, System>,
}

#[derive(Accounts)]
pub struct DepositFunds<'info> {
    #[account(
        mut,
        seeds = [b"state"],
        bump = state.bump,
        has_one = admin @ FeePaymentError::Unauthorized
    )]
    pub state: Account<'info, ProgramState>,
    #[account(mut)]
    pub admin: Signer<'info>,
    pub system_program: Program<'info, System>,
}

#[derive(Accounts)]
#[instruction(ad_id: String)]
pub struct CreateAd<'info> {
    #[account(
        seeds = [b"state"],
        bump = state.bump,
        has_one = admin @ FeePaymentError::Unauthorized
    )]
    pub state: Account<'info, ProgramState>,
    #[account(
        init,
        payer = admin,
        space = 8 + 774,
        seeds = [b"ad", ad_id.as_bytes()],
        bump
    )]
    pub ad: Account<'info, Advertisement>,
    #[account(mut)]
    pub admin: Signer<'info>,
    pub system_program: Program<'info, System>,
}

#[derive(Accounts)]
pub struct ToggleAd<'info> {
    #[account(
        seeds = [b"state"],
        bump = state.bump,
        has_one = admin @ FeePaymentError::Unauthorized
    )]
    pub state: Account<'info, ProgramState>,
    #[account(mut)]
    pub ad: Account<'info, Advertisement>,
    pub admin: Signer<'info>,
}

#[derive(Accounts)]
pub struct InitiateSend<'info> {
    #[account(
        seeds = [b"state"],
        bump = state.bump
    )]
    pub state: Account<'info, ProgramState>,
    #[account(
        init,
        payer = user,
        space = 8 + 162,
        seeds = [b"request", user.key().as_ref(), &Clock::get().unwrap().unix_timestamp.to_le_bytes()],
        bump
    )]
    pub request: Account<'info, TransactionRequest>,
    #[account(constraint = selected_ad.is_active @ FeePaymentError::AdNotActive)]
    pub selected_ad: Account<'info, Advertisement>,
    #[account(mut)]
    pub user: Signer<'info>,
    pub system_program: Program<'info, System>,
}
#[derive(Accounts)]
pub struct CompleteTransaction<'info> {
    #[account(
        mut,
        seeds = [b"state"],
        bump = state.bump
    )]
    pub state: Account<'info, ProgramState>,
    #[account(
        mut,
        constraint = ad.id == request.selected_ad_id @ FeePaymentError::AdMismatch
    )]
    pub ad: Account<'info, Advertisement>,
    #[account(
        mut,
        has_one = user @ FeePaymentError::Unauthorized,
        constraint = request.recipient == recipient.key() @ FeePaymentError::RecipientMismatch
    )]
    pub request: Account<'info, TransactionRequest>,
    #[account(mut)]
    pub user: Signer<'info>,
    /// CHECK: Recipient validation through constraint
    #[account(mut)]
    pub recipient: AccountInfo<'info>,
    /// CHECK: Fee account to receive sponsored gas fees (treasury/burn account)
    #[account(mut)]
    pub fee_account: AccountInfo<'info>,
    pub system_program: Program<'info, System>,
}

#[derive(Accounts)]
pub struct GetRandomAd<'info> {
    #[account(constraint = ad.is_active @ FeePaymentError::AdNotActive)]
    pub ad: Account<'info, Advertisement>,
}

#[derive(Accounts)]
pub struct CancelRequest<'info> {
    #[account(
        mut,
        has_one = user @ FeePaymentError::Unauthorized
    )]
    pub request: Account<'info, TransactionRequest>,
    pub user: Signer<'info>,
}

#[derive(Accounts)]
pub struct AdminAction<'info> {
    #[account(
        mut,
        seeds = [b"state"],
        bump = state.bump,
        has_one = admin @ FeePaymentError::Unauthorized
    )]
    pub state: Account<'info, ProgramState>,
    pub admin: Signer<'info>,
}

#[derive(Accounts)]
pub struct WithdrawFunds<'info> {
    #[account(
        mut,
        seeds = [b"state"],
        bump = state.bump,
        has_one = admin @ FeePaymentError::Unauthorized
    )]
    pub state: Account<'info, ProgramState>,
    #[account(mut)]
    pub admin: Signer<'info>,
    pub system_program: Program<'info, System>,
}

// Custom Error Types
#[error_code]
pub enum FeePaymentError {
    #[msg("Unauthorized access")]
    Unauthorized,
    #[msg("Invalid amount")]
    InvalidAmount,
    #[msg("Invalid recipient address")]
    InvalidRecipient,
    #[msg("Invalid fee amount")]
    InvalidFee,
    #[msg("Program has insufficient funds to sponsor gas fees")]
    InsufficientProgramFunds,
    #[msg("Invalid ad ID")]
    InvalidAdId,
    #[msg("Invalid ad URL - must be HTTPS")]
    InvalidAdUrl,
    #[msg("Invalid ad content")]
    InvalidAdContent,
    #[msg("Invalid display time")]
    InvalidDisplayTime,
    #[msg("Reward amount too low")]
    RewardTooLow,
    #[msg("Program is paused")]
    ProgramPaused,
    #[msg("Request has expired")]
    RequestExpired,
    #[msg("Invalid request status")]
    InvalidStatus,
    #[msg("Advertisement not active")]
    AdNotActive,
    #[msg("Recipient address mismatch")]
    RecipientMismatch,
    #[msg("Ad ID mismatch")]
    AdMismatch,
    #[msg("Ad display not started")]
    AdNotStarted,
    #[msg("Insufficient ad viewing time")]
    InsufficientViewTime,
    #[msg("Math overflow")]
    MathOverflow,
    #[msg("Math underflow")]
    MathUnderflow,
}
