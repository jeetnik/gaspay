#![allow(deprecated)]
use anchor_lang::prelude::*;
use anchor_lang::system_program::{transfer, Transfer};

declare_id!("368SCgsps98BfdQfgcZvmhexXXijABFWZVj5PDjUWtyi");

#[program]
pub mod fee_payment_dapp {
    use super::*;

    // Initialize the program state
    pub fn initialize(ctx: Context<Initialize>, admin: Pubkey) -> Result<()> {
        let state = &mut ctx.accounts.state;
        state.admin = admin;
        state.total_funds = 0;
        state.total_ads_viewed = 0;
        state.fee_per_ad = 5000; // 0.005 SOL in lamports
        state.bump = ctx.bumps.state;
        
        msg!("Program initialized with admin: {}", admin);
        Ok(())
    }

    // Admin adds funds to the program
    pub fn add_funds(ctx: Context<AddFunds>, amount: u64) -> Result<()> {
        // Transfer SOL from admin to program account using CPI
        let cpi_context = CpiContext::new(
            ctx.accounts.system_program.to_account_info(),
            Transfer {
                from: ctx.accounts.admin.to_account_info(),
                to: ctx.accounts.state.to_account_info(),
            },
        );
        transfer(cpi_context, amount)?;

        let state = &mut ctx.accounts.state;
        state.total_funds += amount;
        
        msg!("Admin added {} lamports. Total funds: {}", amount, state.total_funds);
        Ok(())
    }

    // Admin adds or updates an ad
    pub fn add_ad(ctx: Context<AddAd>, ad_id: String, ad_url: String, reward_amount: u64) -> Result<()> {
        let ad_account = &mut ctx.accounts.ad_account;
        ad_account.ad_id = ad_id.clone();
        ad_account.ad_url = ad_url;
        ad_account.reward_amount = reward_amount;
        ad_account.is_active = true;
        ad_account.total_views = 0;
        ad_account.bump = ctx.bumps.ad_account;
        
        msg!("Ad added: {} with reward: {}", ad_id, reward_amount);
        Ok(())
    }

    // User requests to send SOL with fee coverage
    pub fn request_transaction(
        ctx: Context<RequestTransaction>,
        recipient: Pubkey,
        amount: u64,
        estimated_fee: u64,
        nonce: u64, // Add nonce parameter for uniqueness
    ) -> Result<()> {
        let user_request = &mut ctx.accounts.user_request;
        let state = &ctx.accounts.state;
        
        // Check if program has enough funds to cover the fee
        require!(
            state.total_funds >= estimated_fee,
            ErrorCode::InsufficientProgramFunds
        );
        
        user_request.user = ctx.accounts.user.key();
        user_request.recipient = recipient;
        user_request.amount = amount;
        user_request.estimated_fee = estimated_fee;
        user_request.status = TransactionStatus::Pending;
        user_request.ad_viewed = false;
        user_request.timestamp = Clock::get()?.unix_timestamp;
        user_request.nonce = nonce;
        user_request.bump = ctx.bumps.user_request;
        
        msg!(
            "Transaction request created: {} SOL to {} with fee {}",
            amount,
            recipient,
            estimated_fee
        );
        Ok(())
    }

    // User views ad and gets fee coverage
    pub fn view_ad(ctx: Context<ViewAd>, ad_id: String) -> Result<()> {
        let user_request = &mut ctx.accounts.user_request;
        let ad_account = &mut ctx.accounts.ad_account;
        let state = &mut ctx.accounts.state;
        
        // Verify the request is pending and ad not viewed
        require!(
            user_request.status == TransactionStatus::Pending,
            ErrorCode::InvalidTransactionStatus
        );
        require!(!user_request.ad_viewed, ErrorCode::AdAlreadyViewed);
        require!(ad_account.is_active, ErrorCode::AdNotActive);
        
        // Mark ad as viewed
        user_request.ad_viewed = true;
        ad_account.total_views += 1;
        state.total_ads_viewed += 1;
        
        msg!("User {} viewed ad: {}", user_request.user, ad_id);
        Ok(())
    }

    // Execute the transaction after ad is viewed
    pub fn execute_transaction(ctx: Context<ExecuteTransaction>) -> Result<()> {
        let user_request = &mut ctx.accounts.user_request;
        
        // Verify ad was viewed and request is pending
        require!(user_request.ad_viewed, ErrorCode::AdNotViewed);
        require!(
            user_request.status == TransactionStatus::Pending,
            ErrorCode::InvalidTransactionStatus
        );
        
        // Transfer the main amount from user to recipient
        let user_cpi_context = CpiContext::new(
            ctx.accounts.system_program.to_account_info(),
            Transfer {
                from: ctx.accounts.user.to_account_info(),
                to: ctx.accounts.recipient.to_account_info(),
            },
        );
        transfer(user_cpi_context, user_request.amount)?;
        
        // Get state bump before creating signer seeds
        let state_bump = ctx.accounts.state.bump;
        let estimated_fee = user_request.estimated_fee;
        
        // Pay the network fee from program funds to user
        let seeds = &[b"state".as_ref(), &[state_bump]];
        let signer_seeds = &[&seeds[..]];
        
        let fee_cpi_context = CpiContext::new_with_signer(
            ctx.accounts.system_program.to_account_info(),
            Transfer {
                from: ctx.accounts.state.to_account_info(),
                to: ctx.accounts.user.to_account_info(),
            },
            signer_seeds,
        );
        transfer(fee_cpi_context, estimated_fee)?;
        
        // Update state after transfers are complete
        let state = &mut ctx.accounts.state;
        state.total_funds -= estimated_fee;
        user_request.status = TransactionStatus::Completed;
        
        msg!(
            "Transaction executed: {} SOL sent to {}, fee {} paid by program",
            user_request.amount,
            user_request.recipient,
            estimated_fee
        );
        Ok(())
    }

    // Cancel a pending transaction
    pub fn cancel_transaction(ctx: Context<CancelTransaction>) -> Result<()> {
        let user_request = &mut ctx.accounts.user_request;
        
        require!(
            user_request.status == TransactionStatus::Pending,
            ErrorCode::InvalidTransactionStatus
        );
        
        user_request.status = TransactionStatus::Cancelled;
        
        msg!("Transaction cancelled for user: {}", user_request.user);
        Ok(())
    }

    // Get program statistics
    pub fn get_stats(ctx: Context<GetStats>) -> Result<()> {
        let state = &ctx.accounts.state;
        
        msg!(
            "Program Stats - Total Funds: {}, Total Ads Viewed: {}, Fee per Ad: {}",
            state.total_funds,
            state.total_ads_viewed,
            state.fee_per_ad
        );
        Ok(())
    }
}

// Account structures
#[account]
pub struct ProgramState {
    pub admin: Pubkey,
    pub total_funds: u64,
    pub total_ads_viewed: u64,
    pub fee_per_ad: u64,
    pub bump: u8,
}

#[account]
pub struct AdAccount {
    pub ad_id: String,
    pub ad_url: String,
    pub reward_amount: u64,
    pub is_active: bool,
    pub total_views: u64,
    pub bump: u8,
}

#[account]
pub struct UserTransactionRequest {
    pub user: Pubkey,
    pub recipient: Pubkey,
    pub amount: u64,
    pub estimated_fee: u64,
    pub status: TransactionStatus,
    pub ad_viewed: bool,
    pub timestamp: i64,
    pub nonce: u64, // Add nonce field
    pub bump: u8,
}

#[derive(AnchorSerialize, AnchorDeserialize, Clone, PartialEq, Eq)]
pub enum TransactionStatus {
    Pending,
    Completed,
    Cancelled,
}

// Context structures
#[derive(Accounts)]
pub struct Initialize<'info> {
    #[account(
        init,
        payer = admin,
        space = 8 + 32 + 8 + 8 + 8 + 1, // discriminator + pubkey + 3*u64 + bump
        seeds = [b"state"],
        bump
    )]
    pub state: Account<'info, ProgramState>,
    #[account(mut)]
    pub admin: Signer<'info>,
    pub system_program: Program<'info, System>,
}

#[derive(Accounts)]
pub struct AddFunds<'info> {
    #[account(
        mut,
        seeds = [b"state"],
        bump = state.bump,
        constraint = state.admin == admin.key() @ ErrorCode::UnauthorizedAdmin
    )]
    pub state: Account<'info, ProgramState>,
    #[account(mut)]
    pub admin: Signer<'info>,
    pub system_program: Program<'info, System>,
}

#[derive(Accounts)]
#[instruction(ad_id: String)]
pub struct AddAd<'info> {
    #[account(
        seeds = [b"state"],
        bump = state.bump,
        constraint = state.admin == admin.key() @ ErrorCode::UnauthorizedAdmin
    )]
    pub state: Account<'info, ProgramState>,
    #[account(
        init,
        payer = admin,
        space = 8 + 4 + ad_id.len() + 4 + 200 + 8 + 1 + 8 + 1, // discriminator + string + url + u64 + bool + u64 + bump
        seeds = [b"ad", ad_id.as_bytes()],
        bump
    )]
    pub ad_account: Account<'info, AdAccount>,
    #[account(mut)]
    pub admin: Signer<'info>,
    pub system_program: Program<'info, System>,
}

#[derive(Accounts)]
#[instruction(recipient: Pubkey, amount: u64, estimated_fee: u64, nonce: u64)]
pub struct RequestTransaction<'info> {
    #[account(
        seeds = [b"state"],
        bump = state.bump
    )]
    pub state: Account<'info, ProgramState>,
    #[account(
        init,
        payer = user,
        space = 8 + 32 + 32 + 8 + 8 + 1 + 1 + 8 + 8 + 1, // discriminator + 2*pubkey + 3*u64 + status + bool + i64 + nonce + bump
        seeds = [b"request", user.key().as_ref(), &nonce.to_le_bytes()],
        bump
    )]
    pub user_request: Account<'info, UserTransactionRequest>,
    #[account(mut)]
    pub user: Signer<'info>,
    pub system_program: Program<'info, System>,
}

#[derive(Accounts)]
#[instruction(ad_id: String)]
pub struct ViewAd<'info> {
    #[account(mut)]
    pub state: Account<'info, ProgramState>,
    #[account(
        mut,
        seeds = [b"ad", ad_id.as_bytes()],
        bump = ad_account.bump
    )]
    pub ad_account: Account<'info, AdAccount>,
    #[account(
        mut,
        constraint = user_request.user == user.key() @ ErrorCode::UnauthorizedUser
    )]
    pub user_request: Account<'info, UserTransactionRequest>,
    pub user: Signer<'info>,
}

#[derive(Accounts)]
pub struct ExecuteTransaction<'info> {
    #[account(
        mut,
        seeds = [b"state"],
        bump = state.bump
    )]
    pub state: Account<'info, ProgramState>,
    #[account(
        mut,
        constraint = user_request.user == user.key() @ ErrorCode::UnauthorizedUser
    )]
    pub user_request: Account<'info, UserTransactionRequest>,
    #[account(mut)]
    pub user: Signer<'info>,
    /// CHECK: This is the recipient account, validated by the user request
    #[account(mut)]
    pub recipient: AccountInfo<'info>,
    pub system_program: Program<'info, System>,
}

#[derive(Accounts)]
pub struct CancelTransaction<'info> {
    #[account(
        mut,
        constraint = user_request.user == user.key() @ ErrorCode::UnauthorizedUser
    )]
    pub user_request: Account<'info, UserTransactionRequest>,
    pub user: Signer<'info>,
}

#[derive(Accounts)]
pub struct GetStats<'info> {
    pub state: Account<'info, ProgramState>,
}

// Error codes
#[error_code]
pub enum ErrorCode {
    #[msg("Unauthorized admin access")]
    UnauthorizedAdmin,
    #[msg("Unauthorized user access")]
    UnauthorizedUser,
    #[msg("Insufficient program funds to cover fee")]
    InsufficientProgramFunds,
    #[msg("Invalid transaction status")]
    InvalidTransactionStatus,
    #[msg("Ad already viewed for this transaction")]
    AdAlreadyViewed,
    #[msg("Ad is not active")]
    AdNotActive,
    #[msg("Ad must be viewed before executing transaction")]
    AdNotViewed,
}