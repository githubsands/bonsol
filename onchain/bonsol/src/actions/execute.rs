use crate::{assertions::*, error::ChannelError, utilities::*};

use bonsol_interface::{
    bonsol_schema::{
        root_as_deploy_v1, root_as_input_set, ChannelInstruction, ExecutionRequestV1, InputType,
    },
    util::execution_address_seeds,
};

use solana_program::{account_info::AccountInfo, bpf_loader_upgradeable, system_program};

// execute process leverages the following accounts:
//
// 0: requester
// 1: payer
// 2: executor
// 3. deployment
// 4. callback_program
// 5. system_program
// 6. extra_accounts

#[inline(always)]
fn check_execution_accounts(accounts: &[AccountInfo]) -> Result<(), ChannelError> {
    check_writable_signer(&accounts[0], ChannelError::InvalidRequesterAccount)?;
    check_writable_signer(&accounts[1], ChannelError::InvalidPayerAccount)?;
    check_writeable(&accounts[2], ChannelError::InvalidExecutionAccount)?;
    check_owner(
        &accounts[2],
        &system_program::ID,
        ChannelError::InvalidExecutionAccount,
    )?;
    ensure_0(&accounts[2], ChannelError::InvalidExecutionAccount)?;
    check_owner(
        &accounts[3],
        &crate::ID,
        ChannelError::InvalidDeploymentAccount,
    )?;
    check_key_match(
        &accounts[5],
        &system_program::ID,
        ChannelError::InvalidInstruction,
    )?;
    or(
        &[
            check_key_match(
                &accounts[5],
                &crate::ID,
                ChannelError::InvalidCallbackAccount,
            ),
            check_owner(
                &accounts[5],
                &bpf_loader_upgradeable::ID,
                ChannelError::InvalidCallbackAccount,
            ),
        ],
        ChannelError::InvalidCallbackAccount,
    )?;

    Ok(())
}

fn validate_er(er: &ExecutionRequestV1) -> Result<(), ChannelError> {
    if er.max_block_height() == 0 {
        return Err(ChannelError::MaxBlockHeightRequired);
    }

    if er.verify_input_hash() && er.input_digest().is_none() {
        return Err(ChannelError::InputDigestRequired);
    }
    Ok(())
}

#[inline(always)]
fn validate_inputs(
    er: &ExecutionRequestV1,
    required_input_size: usize,
    extra_accounts: &[AccountInfo],
) -> Result<(), ChannelError> {
    let inputs = er.input().ok_or(ChannelError::InvalidInputs)?;
    let invalid_input_type_count = inputs
        .iter()
        .filter(|i| i.input_type() == InputType::PrivateLocal)
        .count();
    if invalid_input_type_count > 0 {
        return Err(ChannelError::InvalidInputType.into());
    }

    let mut num_sets = 0;
    let input_set: usize = inputs
        .iter()
        .filter(|i| {
            // these must be changed on client to reference account index, they will be 1 byte
            i.data().is_some() && i.input_type() == InputType::InputSet
        })
        .flat_map(|i| {
            num_sets += 1;
            // can panic here
            let index = i.data().map(|x| x.bytes().get(0)).flatten().unwrap();
            let rel_index = index - 6;
            let accounts = extra_accounts
                .get(rel_index as usize)
                .ok_or(ChannelError::InvalidInputs)
                .unwrap();
            let data = accounts.data.borrow();

            let input_set = root_as_input_set(&*data).map_err(|_| ChannelError::InvalidInputs)?;
            input_set
                .inputs()
                .map(|x| x.len())
                .ok_or(ChannelError::InvalidInputs)
        })
        .fold(0, |acc, x| acc + x);

    if inputs.len() - num_sets + input_set != required_input_size {
        return Err(ChannelError::InvalidInputs);
    }

    Ok(())
}

pub fn process_execute_v1(
    accounts: &[AccountInfo],
    ix: ChannelInstruction,
) -> Result<(), ChannelError> {
    check_execution_accounts(accounts)?;

    let er = ix.execute_v1_nested_flatbuffer();
    if er.is_none() {
        return Err(ChannelError::InvalidInstruction);
    }
    let er = er.unwrap();
    let eid = er
        .execution_id()
        .ok_or(ChannelError::InvalidExecutionId)?
        .as_bytes();

    let deploy_data = &*accounts[3]
        .try_borrow_data()
        .map_err(|_| ChannelError::InvalidDeploymentAccount)?;

    let deploy =
        root_as_deploy_v1(&*&deploy_data).map_err(|_| ChannelError::InvalidDeploymentAccount)?;

    let required_input_size = deploy.inputs().map(|x| x.len()).unwrap_or(1);

    validate_inputs(&er, required_input_size, &accounts[6..])?;

    let exec_bump = [check_pda(
        &execution_address_seeds(accounts[0].key, eid),
        &accounts[3].key,
        ChannelError::InvalidExecutionAccount,
    )?];

    let mut seeds = execution_address_seeds(accounts[0].key, eid);
    seeds.push(&exec_bump);

    let bytes = ix.execute_v1().unwrap().bytes();
    save_structure(
        &accounts[2],
        &seeds,
        bytes,
        &accounts[1],
        &accounts[5],
        None,
    )?;

    Ok(())
}
