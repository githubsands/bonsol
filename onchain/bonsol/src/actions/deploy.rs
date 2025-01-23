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

pub struct DeploymentInstance<'a> {
    pub imageid: &'a str,
    pub image_checksum: &'a [u8],
    pub deployment_bump: &'a [u8; 1],
}

pub fn check_owner_instruction(data: DeployV1) -> Result<&[u8], ChannelError> {
    let owner = data
        .owner()
        .map(|b| b.bytes())
        .ok_or(ChannelError::InvalidInstructionNoOwnerGiven)?;
    Ok(owner)
}

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
        return Err(ChannelError::InvalidInstruction.into());
    }
    let dp = dp.unwrap();

    let data = check_owner_instruction(dp)?;
    check_accounts_deployment(&accounts[..=4], data)?;

    if let Some(imageid) = dp.image_id() {
        let deployment_bump = Some(check_pda(
            &deployment_address_seeds(&img_id_hash(imageid)),
            &accounts[0].key, // deployer account key
            ChannelError::InvalidDeploymentAccountPDA,
        )?);
    } else {
        return Err(ChannelError::InvalidInstructionNoImageIDGiven);
    }
    if let Some(imageid) = dp.image_id() {
        let imghash = img_id_hash(imageid);
        let mut seeds = deployment_address_seeds(&imghash);

        let b = check_pda(
            &deployment_address_seeds(&img_id_hash(imageid)),
            accounts[0].key,
            ChannelError::InvalidDeploymentAccountPDA,
        )?;
        let bs = &[b];
        seeds.push(bs);
        let dp_bytes = ix.deploy_v1().unwrap().bytes();

        let space = dp_bytes.len() as u64;

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
    return Err(ChannelError::InvalidInstructionNoImageIDGiven);
}
