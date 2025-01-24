use bonsol_interface::{
    bonsol_schema::{root_as_execution_request_v1, ChannelInstruction, ClaimV1},
    claim_state::ClaimStateV1,
    util::{execution_address_seeds, execution_claim_address_seeds},
};

use solana_program::{
    account_info::AccountInfo, msg, program_error::ProgramError, system_program, sysvar::Sysvar,
};

use crate::{assertions::*, error::ChannelError, utilities::*};

// claim processe leverages the following accounts:
//
// 0: executor
// 1: requester
// 2: executive_claim
// 3. claimer (or prover)
// 4. payer
// 5. system_program

#[inline(always)]
fn check_accounts_claim(accounts: &[AccountInfo]) -> Result<(), ChannelError> {
    check_writable_signer(&accounts[4], ChannelError::InvalidPayerAccount)?;
    check_writable_signer(&accounts[3], ChannelError::InvalidClaimerAccount)?;
    check_writeable(&accounts[2], ChannelError::InvalidClaimAccount)?;
    check_writeable(&accounts[0], ChannelError::InvalidExecutionAccount)?;
    check_owner(
        &accounts[0],
        &crate::ID,
        ChannelError::InvalidExecutionAccountOwner,
    )?;
    Ok(())
}

// todo: use enum for claim state rather then bools - expired, existing, etc.
pub struct Claim<'a> {
    execution_id: &'a str,
    block_commitment: u64,
    existing_claim: bool,
    stake: u64,
    expired: bool,
}

#[inline(always)]
pub fn build_claim<'a>(
    accounts: &[AccountInfo],
    data: &ClaimV1<'a>,
    current_block: u64,
) -> Result<Claim<'a>, ChannelError> {
    let mut claim = Claim {
        execution_id: data.execution_id().unwrap(),
        block_commitment: data.block_commitment(),
        existing_claim: false,
        stake: 0,
        expired: false,
    };

    let eid = data.execution_id().unwrap();
    let exec_seeds = execution_address_seeds(&accounts[1].key, eid.as_bytes());
    check_pda(
        &exec_seeds,
        &accounts[0].key,
        ChannelError::InvalidExecutionAccount,
    )?;
    let exec_data = accounts[0]
        .try_borrow_data()
        .map_err(|_| ChannelError::CannotBorrowData)?;

    let execution_request = root_as_execution_request_v1(&*exec_data)
        .map_err(|_| ChannelError::InvalidExecutionAccountData)?;
    let expected_eid = execution_request.execution_id();
    if expected_eid != data.execution_id() {
        return Err(ChannelError::InvalidExecutionId);
    }
    let tip = execution_request.tip();

    if accounts[3].lamports() < tip {
        return Err(ChannelError::InsufficientStake.into());
    }
    if execution_request.max_block_height() < current_block {
        claim.expired = true;
    }
    // make this more dynamic
    claim.stake = tip / 2;

    let mut exec_claim_seeds = execution_claim_address_seeds(accounts[0].key.as_ref());
    let bump = [check_pda(
        &exec_claim_seeds,
        accounts[2].key,
        ChannelError::InvalidClaimAccount,
    )?];
    exec_claim_seeds.push(&bump);
    if accounts[2].data_len() == 0 && accounts[2].owner == &system_program::ID {
        create_program_account(
            &accounts[2],
            &exec_claim_seeds,
            std::mem::size_of::<ClaimStateV1>() as u64,
            &accounts[4],
            &accounts[5],
            None,
        )?;
    } else {
        check_owner(&accounts[2], &crate::ID, ChannelError::InvalidClaimAccount)?;
        claim.existing_claim = true;
    }
    return Ok(claim);
}

pub fn process_claim_v1(
    accounts: &[AccountInfo],
    ix: ChannelInstruction,
) -> Result<(), ProgramError> {
    check_accounts_claim(accounts)?;

    let cl = ix.claim_v1_nested_flatbuffer();
    if cl.is_none() {
        return Err(ChannelError::InvalidInstruction.into());
    }
    let cl = cl.unwrap();

    if cl.execution_id().is_none() {
        return Err(ChannelError::InvalidInstruction.into());
    }

    // todo: get the current block
    let current_block = solana_program::clock::Clock::get()?.slot;

    let claim_meta = build_claim(accounts, &cl, current_block)?;

    if claim_meta.expired {
        cleanup_execution_account(
            &accounts[0],
            &accounts[3],
            ChannelError::ExecutionExpired as u8,
        )?;
        msg!("Execution expired");
        return Ok(());
    }
    if claim_meta.existing_claim {
        let mut data = accounts[2].try_borrow_mut_data()?;
        let current_claim =
            ClaimStateV1::load_claim(*data).map_err(|_| ChannelError::InvalidClaimAccount)?;
        transfer_owned(&accounts[2], &accounts[3], claim_meta.stake)?;
        if current_block > current_claim.block_commitment {
            let claim = ClaimStateV1::from_claim_ix(
                &accounts[3].key,
                current_block,
                claim_meta.block_commitment,
            );
            drop(data);
            ClaimStateV1::save_claim(&claim, &accounts[2]);
            transfer_unowned(&accounts[3], &accounts[2], claim_meta.stake)
        } else {
            Err(ChannelError::ActiveClaimExists.into())
        }
    } else {
        let claim = ClaimStateV1::from_claim_ix(
            &accounts[3].key,
            current_block,
            claim_meta.block_commitment,
        );
        transfer_unowned(&accounts[3], &accounts[2], claim_meta.stake)?;
        ClaimStateV1::save_claim(&claim, &accounts[2]);
        Ok(())
    }
}
