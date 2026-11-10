//! Approval vault: treasury funds leave only after M-of-N approvers sign off.
//!
//! Spec: README §3. Build order: tasks 2–4 in AGENTS.md.
#![no_std]

pub mod events;
mod storage;
pub mod types;

#[cfg(test)]
mod proptests;
#[cfg(test)]
mod test;

use finance_os_common::amount;
use soroban_sdk::{contract, contractimpl, token, vec, Address, BytesN, Env, Vec};

use crate::events::{
    Approved, Cancelled, Deposited, Executed, PolicyProposed, PolicyUpdated, Proposed,
};
use crate::types::{Error, Policy, PolicyProposal, Proposal, ProposalStatus, MAX_APPROVERS};

#[contract]
pub struct ApprovalVault;

#[contractimpl]
impl ApprovalVault {
    /// Sets the initial policy. Rejects `threshold == 0`,
    /// `threshold > approvers.len()`, duplicate approvers, and more than
    /// [`MAX_APPROVERS`] approvers.
    pub fn __constructor(env: Env, approvers: Vec<Address>, threshold: u32) -> Result<(), Error> {
        let policy = validate_policy(approvers, threshold)?;
        storage::set_policy(&env, &policy);
        storage::bump_instance(&env);
        Ok(())
    }

    /// Transfer `amount` of `token` from `from` into the vault.
    ///
    /// Anyone may deposit. `token` must be a contract implementing the standard
    /// token interface (e.g. a Stellar Asset Contract).
    pub fn deposit(env: Env, from: Address, token: Address, amount: i128) -> Result<(), Error> {
        from.require_auth();
        if !amount::is_positive(amount) {
            return Err(Error::InvalidAmount);
        }
        storage::bump_instance(&env);
        token::TokenClient::new(&env, &token).transfer(
            &from,
            env.current_contract_address(),
            &amount,
        );
        Deposited {
            token,
            from,
            amount,
        }
        .publish(&env);
        Ok(())
    }

    /// Create a Pending payment proposal. The proposer must be an approver,
    /// and their approval counts towards the threshold.
    ///
    /// `memo_ref` is a hash of the backend `PaymentRequest` id.
    /// `expires_at` is a ledger timestamp (seconds) that must be in the future.
    /// `token` must implement the standard token interface. This is checked
    /// here so a bad token fails at proposal time, not after approvals.
    pub fn propose(
        env: Env,
        proposer: Address,
        token: Address,
        to: Address,
        amount: i128,
        memo_ref: BytesN<32>,
        expires_at: u64,
    ) -> Result<u64, Error> {
        proposer.require_auth();
        storage::bump_instance(&env);
        require_approver(&storage::get_policy(&env), &proposer)?;
        if !amount::is_positive(amount) {
            return Err(Error::InvalidAmount);
        }
        require_future(&env, expires_at)?;
        require_token(&env, &token)?;

        let id = storage::next_proposal_id(&env)?;
        storage::set_proposal(
            &env,
            &Proposal {
                id,
                token: token.clone(),
                to: to.clone(),
                amount,
                memo_ref: memo_ref.clone(),
                created_by: proposer.clone(),
                approvals: vec![&env, proposer.clone()],
                status: ProposalStatus::Pending,
                expires_at,
            },
        );
        Proposed {
            proposal_id: id,
            proposer,
            token,
            to,
            amount,
            memo_ref,
            expires_at,
        }
        .publish(&env);
        Ok(id)
    }

    /// Add `approver`'s approval to a Pending, unexpired proposal.
    pub fn approve(env: Env, approver: Address, id: u64) -> Result<(), Error> {
        approver.require_auth();
        storage::bump_instance(&env);
        let policy = storage::get_policy(&env);
        require_approver(&policy, &approver)?;

        let mut proposal = storage::get_proposal(&env, id)?;
        require_open(&env, proposal.status, proposal.expires_at)?;
        if proposal.approvals.contains(&approver) {
            return Err(Error::AlreadyApproved);
        }
        proposal.approvals = current_approvals(&env, &policy, &proposal.approvals);
        proposal.approvals.push_back(approver.clone());
        storage::set_proposal(&env, &proposal);

        Approved {
            proposal_id: id,
            approver,
            approvals: proposal.approvals.len(),
        }
        .publish(&env);
        Ok(())
    }

    /// Execute a Pending, unexpired proposal once it has at least `threshold`
    /// approvals from the *current* approver set. Callable by anyone.
    ///
    /// State is marked Executed before the token transfer (no re-entrancy
    /// assumptions). If the transfer fails the whole transaction reverts.
    pub fn execute(env: Env, id: u64) -> Result<(), Error> {
        storage::bump_instance(&env);
        let policy = storage::get_policy(&env);
        let mut proposal = storage::get_proposal(&env, id)?;
        require_open(&env, proposal.status, proposal.expires_at)?;
        if count_valid_approvals(&policy, &proposal.approvals) < policy.threshold {
            return Err(Error::ThresholdNotMet);
        }

        let vault = env.current_contract_address();
        let token = token::TokenClient::new(&env, &proposal.token);
        if token.balance(&vault) < proposal.amount {
            return Err(Error::InsufficientBalance);
        }

        proposal.status = ProposalStatus::Executed;
        storage::set_proposal(&env, &proposal);
        token.transfer(&vault, &proposal.to, &proposal.amount);

        Executed {
            proposal_id: id,
            token: proposal.token,
            to: proposal.to,
            amount: proposal.amount,
        }
        .publish(&env);
        Ok(())
    }

    /// Cancel a Pending proposal. Any approver may cancel, including after
    /// expiry (so stale proposals can be closed out explicitly).
    pub fn cancel(env: Env, caller: Address, id: u64) -> Result<(), Error> {
        caller.require_auth();
        storage::bump_instance(&env);
        require_approver(&storage::get_policy(&env), &caller)?;

        let mut proposal = storage::get_proposal(&env, id)?;
        if proposal.status != ProposalStatus::Pending {
            return Err(Error::NotPending);
        }
        proposal.status = ProposalStatus::Cancelled;
        storage::set_proposal(&env, &proposal);

        Cancelled {
            proposal_id: id,
            cancelled_by: caller,
        }
        .publish(&env);
        Ok(())
    }

    /// Propose replacing the policy. The proposer must be a current approver,
    /// and their approval counts. The new policy is validated up front
    /// with the same rules as the constructor.
    pub fn propose_policy_update(
        env: Env,
        proposer: Address,
        new_approvers: Vec<Address>,
        new_threshold: u32,
        expires_at: u64,
    ) -> Result<u64, Error> {
        proposer.require_auth();
        storage::bump_instance(&env);
        require_approver(&storage::get_policy(&env), &proposer)?;
        let new_policy = validate_policy(new_approvers, new_threshold)?;
        require_future(&env, expires_at)?;

        let id = storage::next_proposal_id(&env)?;
        storage::set_policy_proposal(
            &env,
            &PolicyProposal {
                id,
                new_policy: new_policy.clone(),
                created_by: proposer.clone(),
                approvals: vec![&env, proposer.clone()],
                status: ProposalStatus::Pending,
                expires_at,
            },
        );
        PolicyProposed {
            proposal_id: id,
            proposer,
            approvers: new_policy.approvers,
            threshold: new_policy.threshold,
            expires_at,
        }
        .publish(&env);
        Ok(id)
    }

    /// Approve a Pending, unexpired policy-update proposal.
    pub fn approve_policy_update(env: Env, approver: Address, id: u64) -> Result<(), Error> {
        approver.require_auth();
        storage::bump_instance(&env);
        let policy = storage::get_policy(&env);
        require_approver(&policy, &approver)?;

        let mut proposal = storage::get_policy_proposal(&env, id)?;
        require_open(&env, proposal.status, proposal.expires_at)?;
        if proposal.approvals.contains(&approver) {
            return Err(Error::AlreadyApproved);
        }
        proposal.approvals = current_approvals(&env, &policy, &proposal.approvals);
        proposal.approvals.push_back(approver.clone());
        storage::set_policy_proposal(&env, &proposal);

        Approved {
            proposal_id: id,
            approver,
            approvals: proposal.approvals.len(),
        }
        .publish(&env);
        Ok(())
    }

    /// Apply a policy-update proposal once it has at least `threshold`
    /// approvals from the *current* approver set. Callable by anyone.
    ///
    /// Deviation from README §3: the README sketches
    /// `update_policy(new_approvers, new_threshold)`. Here the new policy comes
    /// from an approved `PolicyProposal`, so the call takes that proposal's id.
    pub fn update_policy(env: Env, id: u64) -> Result<(), Error> {
        storage::bump_instance(&env);
        let policy = storage::get_policy(&env);
        let mut proposal = storage::get_policy_proposal(&env, id)?;
        require_open(&env, proposal.status, proposal.expires_at)?;
        if count_valid_approvals(&policy, &proposal.approvals) < policy.threshold {
            return Err(Error::ThresholdNotMet);
        }

        proposal.status = ProposalStatus::Executed;
        storage::set_policy_proposal(&env, &proposal);
        storage::set_policy(&env, &proposal.new_policy);

        PolicyUpdated {
            proposal_id: id,
            approvers: proposal.new_policy.approvers,
            threshold: proposal.new_policy.threshold,
        }
        .publish(&env);
        Ok(())
    }

    /// Cancel a Pending policy-update proposal. Any current approver may cancel.
    pub fn cancel_policy_update(env: Env, caller: Address, id: u64) -> Result<(), Error> {
        caller.require_auth();
        storage::bump_instance(&env);
        require_approver(&storage::get_policy(&env), &caller)?;

        let mut proposal = storage::get_policy_proposal(&env, id)?;
        if proposal.status != ProposalStatus::Pending {
            return Err(Error::NotPending);
        }
        proposal.status = ProposalStatus::Cancelled;
        storage::set_policy_proposal(&env, &proposal);

        Cancelled {
            proposal_id: id,
            cancelled_by: caller,
        }
        .publish(&env);
        Ok(())
    }

    /// The vault's balance of `token`.
    pub fn balance(env: Env, token: Address) -> i128 {
        storage::bump_instance(&env);
        token::TokenClient::new(&env, &token).balance(&env.current_contract_address())
    }

    pub fn get_proposal(env: Env, id: u64) -> Result<Proposal, Error> {
        storage::bump_instance(&env);
        storage::get_proposal(&env, id)
    }

    pub fn get_policy_proposal(env: Env, id: u64) -> Result<PolicyProposal, Error> {
        storage::bump_instance(&env);
        storage::get_policy_proposal(&env, id)
    }

    pub fn get_policy(env: Env) -> Policy {
        storage::bump_instance(&env);
        storage::get_policy(&env)
    }
}

fn validate_policy(approvers: Vec<Address>, threshold: u32) -> Result<Policy, Error> {
    if approvers.len() > MAX_APPROVERS {
        return Err(Error::TooManyApprovers);
    }
    if threshold == 0 || threshold > approvers.len() {
        return Err(Error::InvalidThreshold);
    }
    let mut seen = Vec::new(approvers.env());
    for a in approvers.iter() {
        if seen.contains(&a) {
            return Err(Error::DuplicateApprover);
        }
        seen.push_back(a);
    }
    Ok(Policy {
        approvers,
        threshold,
    })
}

fn require_approver(policy: &Policy, who: &Address) -> Result<(), Error> {
    if policy.approvers.contains(who) {
        Ok(())
    } else {
        Err(Error::NotApprover)
    }
}

/// A proposal can be acted on only while Pending and before `expires_at`.
fn require_open(env: &Env, status: ProposalStatus, expires_at: u64) -> Result<(), Error> {
    if status != ProposalStatus::Pending {
        return Err(Error::NotPending);
    }
    if env.ledger().timestamp() >= expires_at {
        return Err(Error::Expired);
    }
    Ok(())
}

/// `expires_at` must be strictly in the future.
fn require_future(env: &Env, expires_at: u64) -> Result<(), Error> {
    if expires_at <= env.ledger().timestamp() {
        return Err(Error::InvalidExpiry);
    }
    Ok(())
}

/// `token` must answer the standard token interface's `decimals()`.
fn require_token(env: &Env, token: &Address) -> Result<(), Error> {
    match token::TokenClient::new(env, token).try_decimals() {
        Ok(Ok(_)) => Ok(()),
        _ => Err(Error::InvalidToken),
    }
}

/// `approvals` minus anyone no longer in the policy. Applied on every approve
/// so stale entries are dropped and the list stays bounded by `MAX_APPROVERS`
/// across policy changes.
fn current_approvals(env: &Env, policy: &Policy, approvals: &Vec<Address>) -> Vec<Address> {
    let mut kept = Vec::new(env);
    for a in approvals.iter() {
        if policy.approvers.contains(&a) {
            kept.push_back(a);
        }
    }
    kept
}

/// Approvals that still count: only addresses in the current policy. If the
/// policy changes while a proposal is pending, removed approvers' votes drop out.
fn count_valid_approvals(policy: &Policy, approvals: &Vec<Address>) -> u32 {
    // Bounded by MAX_APPROVERS, so saturation can never actually kick in.
    approvals
        .iter()
        .filter(|a| policy.approvers.contains(a))
        .fold(0u32, |n, _| n.saturating_add(1))
}
