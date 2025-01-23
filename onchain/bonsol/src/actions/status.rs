use crate::{
    assertions::*,
    error::ChannelError,
    proof_handling::{output_digest_v1_0_1, prepare_inputs_v1_0_1, verify_risc0_v1_0_1},
    utilities::*,
};

use bonsol_interface::{
    bonsol_schema::{
        root_as_execution_request_v1, ChannelInstruction, ExecutionRequestV1, ExitCode, StatusV1,
    },
    prover_version::{ProverVersion, VERSION_V1_0_1},
    util::execution_address_seeds,
};

use solana_program::{
    account_info::AccountInfo,
    clock::Clock,
    instruction::{AccountMeta, Instruction},
    msg,
    program::invoke_signed,
    program_error::ProgramError,
    program_memory::sol_memcmp,
    sysvar::Sysvar,
};

// process_status_v1 handles 4+ accounts - their indexes are the following:
//  all extra accounts are for use within solana
//
// 0: requester
// 1: executor
// 2. prover
// 3. callback_program
// 4. extra_accounts

pub fn process_status_v1(
    accounts: &[AccountInfo],
    ix: ChannelInstruction,
) -> Result<(), ProgramError> {
    let st = ix.status_v1_nested_flatbuffer();
    if st.is_none() {
        return Err(ChannelError::InvalidInstruction.into());
    }
    let st = st.unwrap();

    let eid = st.execution_id();

    // todo: check this
    let exec_bmp = Some(check_pda(
        &execution_address_seeds(&accounts[0].key, &eid.unwrap().as_bytes()),
        accounts[0].key,
        ChannelError::InvalidExecutionAccount,
    )?);

    let er_ref = accounts[1].try_borrow_data()?;
    let er = root_as_execution_request_v1(&*er_ref)
        .map_err(|_| ChannelError::InvalidExecutionAccount)?;
    let pr_v = st.proof().filter(|x| x.len() == 256);
    if er.max_block_height() < Clock::get()?.slot {
        return Err(ChannelError::ExecutionExpired.into());
    }
    let execution_digest_v = st.execution_digest().map(|x| x.bytes());
    let input_digest_v = st.input_digest().map(|x| x.bytes());
    let assumption_digest_v = st.assumption_digest().map(|x| x.bytes());
    let committed_outputs_v = st.committed_outputs().map(|x| x.bytes());

    if let (Some(proof), Some(exed), Some(asud), Some(input_digest), Some(co)) = (
        pr_v,
        execution_digest_v,
        assumption_digest_v,
        input_digest_v,
        committed_outputs_v,
    ) {
        let proof: &[u8; 256] = proof
            .bytes()
            .try_into()
            .map_err(|_| ChannelError::InvalidInstruction)?;

        if er.verify_input_hash() {
            er.input_digest()
                .map(|x| check_bytes_match(x.bytes(), input_digest, ChannelError::InputsDontMatch));
        }

        let verified = verify_with_prover(input_digest, co, asud, er, exed, st, proof)?;
        let tip = er.tip();

        if verified {
            let callback_program_set =
                sol_memcmp(accounts[3].key.as_ref(), crate::ID.as_ref(), 32) != 0;
            let ix_prefix_set = er.callback_instruction_prefix().is_some();
            if callback_program_set && ix_prefix_set {
                let cbp = er
                    .callback_program_id()
                    .map(|b| b.bytes())
                    .unwrap_or(crate::ID.as_ref());

                check_bytes_match(
                    cbp,
                    accounts[3].key.as_ref(),
                    ChannelError::InvalidCallbackProgram,
                )?;

                let b = [exec_bmp.unwrap()];

                let mut seeds = execution_address_seeds(accounts[0].key, eid.unwrap().as_bytes());

                seeds.push(&b);

                let extra_accounts = accounts[4..].to_vec();

                let mut callback_ix_accounts =
                    vec![AccountMeta::new_readonly(*accounts[1].key, true)];
                if let Some(extra_accounts_callback) = er.callback_extra_accounts() {
                    if extra_accounts.len() != extra_accounts_callback.len() {
                        return Err(ChannelError::InvalidCallbackExtraAccounts.into());
                    }
                    for (i, a) in extra_accounts.iter().enumerate() {
                        let stored_a = extra_accounts_callback.get(i);
                        let key: [u8; 32] = stored_a.pubkey().into();

                        if sol_memcmp(a.key.as_ref(), &key, 32) != 0 {
                            return Err(ChannelError::InvalidCallbackExtraAccounts.into());
                        }
                        // dont cary feepayer signature through to callback we set all signer to false except the ER
                        if a.is_writable {
                            if !stored_a.writable() == 0 {
                                return Err(ChannelError::InvalidCallbackExtraAccounts.into());
                            }
                            callback_ix_accounts.push(AccountMeta::new(*a.key, false));
                        } else {
                            if stored_a.writable() == 1 {
                                //maybe relax this for devs?
                                return Err(ChannelError::InvalidCallbackExtraAccounts.into());
                            }
                            callback_ix_accounts.push(AccountMeta::new_readonly(*a.key, false));
                        }
                    }
                }

                let payload = if er.forward_output() && st.committed_outputs().is_some() {
                    [
                        er.callback_instruction_prefix().unwrap().bytes(),
                        input_digest,
                        st.committed_outputs().unwrap().bytes(),
                    ]
                    .concat()
                } else {
                    er.callback_instruction_prefix().unwrap().bytes().to_vec()
                };

                // pass in the executor account and the program account making this instruction to solana
                let mut ainfos = vec![accounts[1].clone(), accounts[3].clone()];
                ainfos.extend(extra_accounts);

                let callback_ix =
                    Instruction::new_with_bytes(*accounts[1].key, &payload, callback_ix_accounts);
                drop(er_ref);
                let res = invoke_signed(&callback_ix, &ainfos, &[&seeds]);
                match res {
                    Ok(_) => {}
                    Err(e) => {
                        msg!("{} Callback Failed: {:?}", eid.unwrap(), e);
                    }
                }
            }
            // add curve reduction here
            payout_tip(&accounts[1], &accounts[2], tip)?;
            cleanup_execution_account(&accounts[1], &accounts[0], ExitCode::Success as u8)?;
        } else {
            msg!("{} Verifying Failed Cleaning up", eid.unwrap());
            cleanup_execution_account(&accounts[1], &accounts[0], ExitCode::VerifyError as u8)?;
        }
    } else {
        msg!("{} Proving Failed Cleaning up", eid.unwrap());
        cleanup_execution_account(&accounts[1], &accounts[0], ExitCode::ProvingError as u8)?;
    }
    Ok(())
}

fn verify_with_prover(
    input_digest: &[u8],
    co: &[u8],
    asud: &[u8],
    er: ExecutionRequestV1,
    exed: &[u8],
    st: StatusV1,
    proof: &[u8; 256],
) -> Result<bool, ProgramError> {
    let prover_version =
        ProverVersion::try_from(er.prover_version()).unwrap_or(ProverVersion::default());

    let verified = match prover_version {
        VERSION_V1_0_1 => {
            let output_digest = output_digest_v1_0_1(input_digest, co, asud);
            let proof_inputs = prepare_inputs_v1_0_1(
                er.image_id().unwrap(),
                exed,
                output_digest.as_ref(),
                st.exit_code_system(),
                st.exit_code_user(),
            )?;
            verify_risc0_v1_0_1(proof, &proof_inputs)?
        }
        _ => false,
    };
    Ok(verified)
}
