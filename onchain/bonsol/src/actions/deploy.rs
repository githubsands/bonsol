use crate::{assertions::*, error::ChannelError, utilities::*};

use bonsol_interface::{
    bonsol_schema::{ChannelInstruction, DeployV1},
    util::{deployment_address_seeds, img_id_hash},
};

use solana_program::{account_info::AccountInfo, msg, system_program};

// deploy process leverages the following accounts:
//
// 0: deployer
// 1: payer
// 2: deployment
// 3: system_program
// 4++: extra_accountss

#[inline(always)]
pub fn check_owner_instruction(data: DeployV1) -> Result<&[u8], ChannelError> {
    let owner = data
        .owner()
        .map(|b| b.bytes())
        .ok_or(ChannelError::InvalidInstructionNoOwnerGiven)?;
    Ok(owner)
}

#[inline(always)]
pub fn check_accounts_deployment(
    accounts: &[AccountInfo],
    owner: &[u8],
) -> Result<(), ChannelError> {
    check_writable_signer(&accounts[0], ChannelError::InvalidDeployerAccount)?;
    check_writable_signer(&accounts[1], ChannelError::InvalidPayerAccount)?;
    check_bytes_match(
        &accounts[0].key.as_ref(),
        owner,
        ChannelError::InvalidDeployerAccount,
    )?;
    check_writeable(&accounts[0], ChannelError::InvalidDeploymentAccount)?;
    ensure_0(&accounts[0], ChannelError::DeploymentAlreadyExists)?;
    check_owner(
        &accounts[0],
        &system_program::ID,
        ChannelError::DeploymentAlreadyExists,
    )?;
    check_key_match(
        &accounts[3],
        &system_program::ID,
        ChannelError::InvalidInstruction,
    )?;
    Ok(())
}

pub fn process_deploy_v1(
    accounts: &[AccountInfo],
    ix: ChannelInstruction,
) -> Result<(), ChannelError> {
    msg!("deploy");
    let dp = ix.deploy_v1_nested_flatbuffer();
    if dp.is_none() {
        return Err(ChannelError::InvalidInstruction);
    }
    let dp = dp.unwrap();

    let owner = dp
        .owner()
        .map(|b| b.bytes())
        .ok_or(ChannelError::InvalidInstructionNoOwnerGiven)?;

    check_accounts_deployment(&accounts[..=4], owner)?;

    if let Some(imageid) = dp.image_id() {
        let imghash = img_id_hash(imageid);
        let mut seeds = deployment_address_seeds(&imghash);
        let b = &[check_pda(
            &deployment_address_seeds(&img_id_hash(imageid)),
            accounts[0].key,
            ChannelError::InvalidDeploymentAccountPDA,
        )?];
        seeds.push(b);
        let dp_bytes = ix.deploy_v1().unwrap().bytes();

        save_structure(
            &accounts[0],
            &seeds,
            dp_bytes,
            &accounts[1],
            &accounts[3],
            None,
        )?;
        return Ok(());
    }
    Err(ChannelError::InvalidInstructionNoImageIDGiven)
}
