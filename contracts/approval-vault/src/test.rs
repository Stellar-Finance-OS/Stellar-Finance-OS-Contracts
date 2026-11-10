// Tests may use plain arithmetic and unwrap; the workspace lints target
// contract code (README §5).
#![allow(clippy::arithmetic_side_effects, clippy::unwrap_used, clippy::panic)]

extern crate std;

use soroban_sdk::{
    testutils::{
        storage::Persistent as _, Address as _, Events as _, Ledger as _, MockAuth, MockAuthInvoke,
    },
    token::{StellarAssetClient, TokenClient},
    vec, Address, BytesN, Env, Event as _, IntoVal, Val,
};

use crate::{
    events,
    types::{Error, ProposalStatus},
    ApprovalVault, ApprovalVaultClient,
};

/// Ledger timestamp every fixture starts at.
const NOW: u64 = 1_000_000;
/// Default proposal lifetime used by the fixture.
const TTL: u64 = 3_600;

/// Test fixture: a vault with 3 approvers, threshold 2, and a SAC token.
///
/// Auth is mocked explicitly per call (AGENTS.md rule 2) via [`auth`] and
/// [`token_transfer_auth`], never with `mock_all_auths`.
#[allow(dead_code)]
struct Setup<'a> {
    env: Env,
    vault_id: Address,
    vault: ApprovalVaultClient<'a>,
    approvers: [Address; 3],
    token: TokenClient<'a>,
    token_admin: Address,
}

impl Setup<'_> {
    fn new() -> Self {
        let env = Env::default();
        env.ledger().set_timestamp(NOW);
        let approvers = [
            Address::generate(&env),
            Address::generate(&env),
            Address::generate(&env),
        ];
        let vault_id = env.register(
            ApprovalVault,
            (
                vec![
                    &env,
                    approvers[0].clone(),
                    approvers[1].clone(),
                    approvers[2].clone(),
                ],
                2u32,
            ),
        );
        let vault = ApprovalVaultClient::new(&env, &vault_id);
        let token_admin = Address::generate(&env);
        let sac = env.register_stellar_asset_contract_v2(token_admin.clone());
        let token = TokenClient::new(&env, &sac.address());
        Setup {
            env,
            vault_id,
            vault,
            approvers,
            token,
            token_admin,
        }
    }

    /// Mint `amount` of the test token to `to`, authorising only the admin.
    fn mint(&self, to: &Address, amount: i128) {
        let args: soroban_sdk::Vec<Val> = (to.clone(), amount).into_val(&self.env);
        self.env.mock_auths(&[MockAuth {
            address: &self.token_admin,
            invoke: &MockAuthInvoke {
                contract: &self.token.address,
                fn_name: "mint",
                args,
                sub_invokes: &[],
            },
        }]);
        StellarAssetClient::new(&self.env, &self.token.address).mint(to, &amount);
    }

    /// Mint to a fresh account and deposit it into the vault.
    fn fund_vault(&self, amount: i128) {
        let from = Address::generate(&self.env);
        self.mint(&from, amount);
        let args = (from.clone(), self.token.address.clone(), amount).into_val(&self.env);
        let sub = [token_transfer_auth(
            &self.env,
            &self.token.address,
            &from,
            &self.vault_id,
            amount,
        )];
        auth(&self.env, &from, &self.vault_id, "deposit", args, &sub);
        self.vault.deposit(&from, &self.token.address, &amount);
    }
}

impl Setup<'_> {
    fn memo(&self, n: u8) -> BytesN<32> {
        BytesN::from_array(&self.env, &[n; 32])
    }

    /// `proposer` proposes paying `amount` to `to`, expiring at `NOW + TTL`.
    fn propose(&self, proposer: &Address, to: &Address, amount: i128) -> u64 {
        self.try_propose(proposer, proposer, to, amount, NOW + TTL)
            .expect("propose ok")
    }

    /// Propose with full control: `signer` is who actually authorises.
    fn try_propose(
        &self,
        signer: &Address,
        proposer: &Address,
        to: &Address,
        amount: i128,
        expires_at: u64,
    ) -> Result<u64, Option<Error>> {
        self.try_propose_token(
            signer,
            proposer,
            &self.token.address,
            to,
            amount,
            expires_at,
        )
    }

    /// Like [`Setup::try_propose`], but paying out `token`.
    fn try_propose_token(
        &self,
        signer: &Address,
        proposer: &Address,
        token: &Address,
        to: &Address,
        amount: i128,
        expires_at: u64,
    ) -> Result<u64, Option<Error>> {
        let memo = self.memo(1);
        let args = (
            proposer.clone(),
            token.clone(),
            to.clone(),
            amount,
            memo.clone(),
            expires_at,
        )
            .into_val(&self.env);
        auth(&self.env, signer, &self.vault_id, "propose", args, &[]);
        match self
            .vault
            .try_propose(proposer, token, to, &amount, &memo, &expires_at)
        {
            Ok(Ok(id)) => Ok(id),
            Err(Ok(e)) => Err(Some(e)),
            _ => Err(None),
        }
    }
}

impl Setup<'_> {
    fn approve(&self, approver: &Address, id: u64) {
        self.try_approve(approver, approver, id)
            .expect("approve ok");
    }

    fn try_approve(
        &self,
        signer: &Address,
        approver: &Address,
        id: u64,
    ) -> Result<(), Option<Error>> {
        let args = (approver.clone(), id).into_val(&self.env);
        auth(&self.env, signer, &self.vault_id, "approve", args, &[]);
        match self.vault.try_approve(approver, &id) {
            Ok(Ok(())) => Ok(()),
            Err(Ok(e)) => Err(Some(e)),
            _ => Err(None),
        }
    }
}

impl Setup<'_> {
    /// Execute needs no auth: clear any mocked auths first so the call
    /// proves that.
    fn try_execute(&self, id: u64) -> Result<(), Option<Error>> {
        self.env.mock_auths(&[]);
        match self.vault.try_execute(&id) {
            Ok(Ok(())) => Ok(()),
            Err(Ok(e)) => Err(Some(e)),
            _ => Err(None),
        }
    }
}

impl Setup<'_> {
    fn try_cancel(&self, signer: &Address, caller: &Address, id: u64) -> Result<(), Option<Error>> {
        let args = (caller.clone(), id).into_val(&self.env);
        auth(&self.env, signer, &self.vault_id, "cancel", args, &[]);
        match self.vault.try_cancel(caller, &id) {
            Ok(Ok(())) => Ok(()),
            Err(Ok(e)) => Err(Some(e)),
            _ => Err(None),
        }
    }
}

impl Setup<'_> {
    fn new_approvers(&self, n: u32) -> soroban_sdk::Vec<Address> {
        approvers(&self.env, n)
    }

    fn try_propose_policy(
        &self,
        signer: &Address,
        proposer: &Address,
        new_approvers: &soroban_sdk::Vec<Address>,
        new_threshold: u32,
        expires_at: u64,
    ) -> Result<u64, Option<Error>> {
        let args = (
            proposer.clone(),
            new_approvers.clone(),
            new_threshold,
            expires_at,
        )
            .into_val(&self.env);
        auth(
            &self.env,
            signer,
            &self.vault_id,
            "propose_policy_update",
            args,
            &[],
        );
        match self.vault.try_propose_policy_update(
            proposer,
            new_approvers,
            &new_threshold,
            &expires_at,
        ) {
            Ok(Ok(id)) => Ok(id),
            Err(Ok(e)) => Err(Some(e)),
            _ => Err(None),
        }
    }

    fn propose_policy(
        &self,
        proposer: &Address,
        new_approvers: &soroban_sdk::Vec<Address>,
        t: u32,
    ) -> u64 {
        self.try_propose_policy(proposer, proposer, new_approvers, t, NOW + TTL)
            .expect("propose_policy_update ok")
    }
}

impl Setup<'_> {
    fn try_approve_policy(
        &self,
        signer: &Address,
        approver: &Address,
        id: u64,
    ) -> Result<(), Option<Error>> {
        let args = (approver.clone(), id).into_val(&self.env);
        auth(
            &self.env,
            signer,
            &self.vault_id,
            "approve_policy_update",
            args,
            &[],
        );
        match self.vault.try_approve_policy_update(approver, &id) {
            Ok(Ok(())) => Ok(()),
            Err(Ok(e)) => Err(Some(e)),
            _ => Err(None),
        }
    }
}

impl Setup<'_> {
    fn try_update_policy(&self, id: u64) -> Result<(), Option<Error>> {
        self.env.mock_auths(&[]);
        match self.vault.try_update_policy(&id) {
            Ok(Ok(())) => Ok(()),
            Err(Ok(e)) => Err(Some(e)),
            _ => Err(None),
        }
    }
}

impl Setup<'_> {
    fn try_cancel_policy(
        &self,
        signer: &Address,
        caller: &Address,
        id: u64,
    ) -> Result<(), Option<Error>> {
        let args = (caller.clone(), id).into_val(&self.env);
        auth(
            &self.env,
            signer,
            &self.vault_id,
            "cancel_policy_update",
            args,
            &[],
        );
        match self.vault.try_cancel_policy_update(caller, &id) {
            Ok(Ok(())) => Ok(()),
            Err(Ok(e)) => Err(Some(e)),
            _ => Err(None),
        }
    }
}

/// Mock exactly one authorisation: `signer` authorises `contract.fn_name(args)`.
fn auth(
    env: &Env,
    signer: &Address,
    contract: &Address,
    fn_name: &str,
    args: soroban_sdk::Vec<Val>,
    sub_invokes: &[MockAuthInvoke],
) {
    env.mock_auths(&[MockAuth {
        address: signer,
        invoke: &MockAuthInvoke {
            contract,
            fn_name,
            args,
            sub_invokes,
        },
    }]);
}

/// The nested `token.transfer(from, to, amount)` authorisation.
fn token_transfer_auth<'a>(
    env: &Env,
    token: &'a Address,
    from: &Address,
    to: &Address,
    amount: i128,
) -> MockAuthInvoke<'a> {
    MockAuthInvoke {
        contract: token,
        fn_name: "transfer",
        args: (from.clone(), to.clone(), amount).into_val(env),
        sub_invokes: &[],
    }
}

fn approvers(env: &Env, n: u32) -> soroban_sdk::Vec<Address> {
    let mut v = soroban_sdk::Vec::new(env);
    for _ in 0..n {
        v.push_back(Address::generate(env));
    }
    v
}

#[test]
fn constructor_sets_policy() {
    let env = Env::default();
    let a = approvers(&env, 3);
    let id = env.register(ApprovalVault, (a.clone(), 2u32));
    let client = ApprovalVaultClient::new(&env, &id);

    let policy = client.get_policy();
    assert_eq!(policy.approvers, a);
    assert_eq!(policy.threshold, 2);
}

// Constructor failures abort registration, so they surface as panics carrying
// the contract error code (see `types::Error`).

#[test]
#[should_panic(expected = "Error(Contract, #1)")] // InvalidThreshold
fn constructor_rejects_zero_threshold() {
    let env = Env::default();
    let a = approvers(&env, 2);
    env.register(ApprovalVault, (a, 0u32));
}

#[test]
#[should_panic(expected = "Error(Contract, #1)")] // InvalidThreshold
fn constructor_rejects_threshold_above_approver_count() {
    let env = Env::default();
    let a = approvers(&env, 2);
    env.register(ApprovalVault, (a, 3u32));
}

#[test]
#[should_panic(expected = "Error(Contract, #2)")] // DuplicateApprover
fn constructor_rejects_duplicate_approvers() {
    let env = Env::default();
    let a = Address::generate(&env);
    env.register(ApprovalVault, (vec![&env, a.clone(), a], 1u32));
}

#[test]
#[should_panic(expected = "Error(Contract, #12)")] // TooManyApprovers
fn constructor_rejects_too_many_approvers() {
    let env = Env::default();
    let a = approvers(&env, crate::types::MAX_APPROVERS + 1);
    env.register(ApprovalVault, (a, 1u32));
}

#[test]
fn constructor_accepts_max_approvers() {
    let env = Env::default();
    let a = approvers(&env, crate::types::MAX_APPROVERS);
    let id = env.register(ApprovalVault, (a, crate::types::MAX_APPROVERS));
    let client = ApprovalVaultClient::new(&env, &id);
    assert_eq!(client.get_policy().threshold, crate::types::MAX_APPROVERS);
}

// ---------------------------------------------------------------- deposit

#[test]
fn deposit_moves_tokens_into_vault() {
    let s = Setup::new();
    s.fund_vault(1_000);
    assert_eq!(s.token.balance(&s.vault_id), 1_000);
}

#[test]
fn deposit_emits_one_deposited_event() {
    let s = Setup::new();
    let from = Address::generate(&s.env);
    s.mint(&from, 300);
    let args = (from.clone(), s.token.address.clone(), 300i128).into_val(&s.env);
    let sub = [token_transfer_auth(
        &s.env,
        &s.token.address,
        &from,
        &s.vault_id,
        300,
    )];
    auth(&s.env, &from, &s.vault_id, "deposit", args, &sub);
    s.vault.deposit(&from, &s.token.address, &300);
    let expected = events::Deposited {
        token: s.token.address.clone(),
        from,
        amount: 300,
    };
    assert_eq!(
        s.env
            .events()
            .all()
            .filter_by_contract(&s.vault_id)
            .events(),
        [expected.to_xdr(&s.env, &s.vault_id)]
    );
}

#[test]
fn deposit_rejects_zero_amount() {
    let s = Setup::new();
    let from = Address::generate(&s.env);
    let args = (from.clone(), s.token.address.clone(), 0i128).into_val(&s.env);
    auth(&s.env, &from, &s.vault_id, "deposit", args, &[]);
    let res = s.vault.try_deposit(&from, &s.token.address, &0);
    assert_eq!(res, Err(Ok(Error::InvalidAmount)));
}

#[test]
fn deposit_rejects_negative_amount() {
    let s = Setup::new();
    let from = Address::generate(&s.env);
    let args = (from.clone(), s.token.address.clone(), -5i128).into_val(&s.env);
    auth(&s.env, &from, &s.vault_id, "deposit", args, &[]);
    let res = s.vault.try_deposit(&from, &s.token.address, &-5);
    assert_eq!(res, Err(Ok(Error::InvalidAmount)));
}

#[test]
fn deposit_rejects_wrong_signer() {
    let s = Setup::new();
    let from = Address::generate(&s.env);
    let attacker = Address::generate(&s.env);
    s.mint(&from, 100);
    let args = (from.clone(), s.token.address.clone(), 100i128).into_val(&s.env);
    auth(&s.env, &attacker, &s.vault_id, "deposit", args, &[]);
    assert!(s.vault.try_deposit(&from, &s.token.address, &100).is_err());
    assert_eq!(s.token.balance(&from), 100);
    assert_eq!(s.token.balance(&s.vault_id), 0);
}

#[test]
fn deposit_rejects_unknown_token() {
    let s = Setup::new();
    let from = Address::generate(&s.env);
    let not_a_token = Address::generate(&s.env);
    let args = (from.clone(), not_a_token.clone(), 100i128).into_val(&s.env);
    auth(&s.env, &from, &s.vault_id, "deposit", args, &[]);
    assert!(s.vault.try_deposit(&from, &not_a_token, &100).is_err());
}

#[test]
fn deposit_fails_with_insufficient_funds() {
    let s = Setup::new();
    let from = Address::generate(&s.env);
    s.mint(&from, 50);
    let args = (from.clone(), s.token.address.clone(), 100i128).into_val(&s.env);
    let sub = [token_transfer_auth(
        &s.env,
        &s.token.address,
        &from,
        &s.vault_id,
        100,
    )];
    auth(&s.env, &from, &s.vault_id, "deposit", args, &sub);
    assert!(s.vault.try_deposit(&from, &s.token.address, &100).is_err());
    assert_eq!(s.token.balance(&from), 50);
}

// ---------------------------------------------------------------- views

#[test]
fn balance_reports_vault_holdings() {
    let s = Setup::new();
    assert_eq!(s.vault.balance(&s.token.address), 0);
    s.fund_vault(250);
    s.fund_vault(750);
    assert_eq!(s.vault.balance(&s.token.address), 1_000);
}

// ---------------------------------------------------------------- propose

#[test]
fn propose_creates_pending_proposal_with_proposer_approval() {
    let s = Setup::new();
    let to = Address::generate(&s.env);
    let id = s.propose(&s.approvers[0], &to, 400);

    let p = s.vault.get_proposal(&id);
    assert_eq!(p.id, 0);
    assert_eq!(p.token, s.token.address);
    assert_eq!(p.to, to);
    assert_eq!(p.amount, 400);
    assert_eq!(p.memo_ref, s.memo(1));
    assert_eq!(p.created_by, s.approvers[0]);
    assert_eq!(p.approvals, vec![&s.env, s.approvers[0].clone()]);
    assert_eq!(p.status, ProposalStatus::Pending);
    assert_eq!(p.expires_at, NOW + TTL);
}

#[test]
fn propose_emits_proposed_event() {
    let s = Setup::new();
    let to = Address::generate(&s.env);
    let id = s.propose(&s.approvers[0], &to, 400);
    let expected = events::Proposed {
        proposal_id: id,
        proposer: s.approvers[0].clone(),
        token: s.token.address.clone(),
        to,
        amount: 400,
        memo_ref: s.memo(1),
        expires_at: NOW + TTL,
    };
    assert_eq!(
        s.env
            .events()
            .all()
            .filter_by_contract(&s.vault_id)
            .events(),
        [expected.to_xdr(&s.env, &s.vault_id)]
    );
}

#[test]
fn propose_assigns_sequential_ids() {
    let s = Setup::new();
    let to = Address::generate(&s.env);
    assert_eq!(s.propose(&s.approvers[0], &to, 1), 0);
    assert_eq!(s.propose(&s.approvers[1], &to, 1), 1);
    assert_eq!(s.propose(&s.approvers[2], &to, 1), 2);
}

#[test]
fn propose_rejects_non_approver() {
    let s = Setup::new();
    let outsider = Address::generate(&s.env);
    let to = Address::generate(&s.env);
    let res = s.try_propose(&outsider, &outsider, &to, 10, NOW + TTL);
    assert_eq!(res, Err(Some(Error::NotApprover)));
}

#[test]
fn propose_rejects_wrong_signer() {
    let s = Setup::new();
    let to = Address::generate(&s.env);
    // approvers[1] signs a proposal claiming to be from approvers[0].
    let res = s.try_propose(&s.approvers[1], &s.approvers[0], &to, 10, NOW + TTL);
    assert_eq!(res, Err(None));
}

#[test]
fn propose_rejects_zero_amount() {
    let s = Setup::new();
    let to = Address::generate(&s.env);
    let res = s.try_propose(&s.approvers[0], &s.approvers[0], &to, 0, NOW + TTL);
    assert_eq!(res, Err(Some(Error::InvalidAmount)));
}

#[test]
fn propose_rejects_negative_amount() {
    let s = Setup::new();
    let to = Address::generate(&s.env);
    let res = s.try_propose(&s.approvers[0], &s.approvers[0], &to, -1, NOW + TTL);
    assert_eq!(res, Err(Some(Error::InvalidAmount)));
}

#[test]
fn propose_rejects_expiry_in_past() {
    let s = Setup::new();
    let to = Address::generate(&s.env);
    let res = s.try_propose(&s.approvers[0], &s.approvers[0], &to, 10, NOW - 1);
    assert_eq!(res, Err(Some(Error::InvalidExpiry)));
}

#[test]
fn propose_rejects_expiry_equal_to_now() {
    let s = Setup::new();
    let to = Address::generate(&s.env);
    let res = s.try_propose(&s.approvers[0], &s.approvers[0], &to, 10, NOW);
    assert_eq!(res, Err(Some(Error::InvalidExpiry)));
}

#[test]
fn propose_rejects_unknown_token() {
    let s = Setup::new();
    let to = Address::generate(&s.env);
    let not_a_token = Address::generate(&s.env);
    let p = &s.approvers[0];
    let res = s.try_propose_token(p, p, &not_a_token, &to, 10, NOW + TTL);
    assert_eq!(res, Err(Some(Error::InvalidToken)));
}

#[test]
fn propose_rejects_non_token_contract() {
    let s = Setup::new();
    let to = Address::generate(&s.env);
    // A real contract that does not implement the token interface.
    let other_vault = s
        .env
        .register(ApprovalVault, (vec![&s.env, to.clone()], 1u32));
    let p = &s.approvers[0];
    let res = s.try_propose_token(p, p, &other_vault, &to, 10, NOW + TTL);
    assert_eq!(res, Err(Some(Error::InvalidToken)));
}

#[test]
fn get_proposal_rejects_unknown_id() {
    let s = Setup::new();
    assert_eq!(
        s.vault.try_get_proposal(&99),
        Err(Ok(Error::ProposalNotFound))
    );
}

// ---------------------------------------------------------------- approve

#[test]
fn approve_records_approval() {
    let s = Setup::new();
    let to = Address::generate(&s.env);
    let id = s.propose(&s.approvers[0], &to, 10);
    s.approve(&s.approvers[1], id);
    assert_eq!(
        s.vault.get_proposal(&id).approvals,
        vec![&s.env, s.approvers[0].clone(), s.approvers[1].clone()]
    );
}

#[test]
fn approve_emits_approved_event() {
    let s = Setup::new();
    let to = Address::generate(&s.env);
    let id = s.propose(&s.approvers[0], &to, 10);
    s.approve(&s.approvers[1], id);
    let expected = events::Approved {
        proposal_id: id,
        approver: s.approvers[1].clone(),
        approvals: 2,
    };
    assert_eq!(
        s.env
            .events()
            .all()
            .filter_by_contract(&s.vault_id)
            .events(),
        [expected.to_xdr(&s.env, &s.vault_id)]
    );
}

#[test]
fn approve_rejects_double_approval() {
    let s = Setup::new();
    let to = Address::generate(&s.env);
    let id = s.propose(&s.approvers[0], &to, 10);
    s.approve(&s.approvers[1], id);
    assert_eq!(
        s.try_approve(&s.approvers[1], &s.approvers[1], id),
        Err(Some(Error::AlreadyApproved))
    );
}

#[test]
fn approve_rejects_proposer_approving_again() {
    let s = Setup::new();
    let to = Address::generate(&s.env);
    let id = s.propose(&s.approvers[0], &to, 10);
    assert_eq!(
        s.try_approve(&s.approvers[0], &s.approvers[0], id),
        Err(Some(Error::AlreadyApproved))
    );
}

#[test]
fn approve_rejects_non_approver() {
    let s = Setup::new();
    let to = Address::generate(&s.env);
    let outsider = Address::generate(&s.env);
    let id = s.propose(&s.approvers[0], &to, 10);
    assert_eq!(
        s.try_approve(&outsider, &outsider, id),
        Err(Some(Error::NotApprover))
    );
}

#[test]
fn approve_rejects_wrong_signer() {
    let s = Setup::new();
    let to = Address::generate(&s.env);
    let id = s.propose(&s.approvers[0], &to, 10);
    // approvers[2] tries to cast approvers[1]'s vote.
    assert_eq!(
        s.try_approve(&s.approvers[2], &s.approvers[1], id),
        Err(None)
    );
    assert_eq!(s.vault.get_proposal(&id).approvals.len(), 1);
}

#[test]
fn approve_rejects_expired_proposal() {
    let s = Setup::new();
    let to = Address::generate(&s.env);
    let id = s.propose(&s.approvers[0], &to, 10);
    s.env.ledger().set_timestamp(NOW + TTL);
    assert_eq!(
        s.try_approve(&s.approvers[1], &s.approvers[1], id),
        Err(Some(Error::Expired))
    );
}

#[test]
fn approve_accepts_just_before_expiry() {
    let s = Setup::new();
    let to = Address::generate(&s.env);
    let id = s.propose(&s.approvers[0], &to, 10);
    s.env.ledger().set_timestamp(NOW + TTL - 1);
    s.approve(&s.approvers[1], id);
}

#[test]
fn approve_rejects_unknown_proposal() {
    let s = Setup::new();
    assert_eq!(
        s.try_approve(&s.approvers[1], &s.approvers[1], 42),
        Err(Some(Error::ProposalNotFound))
    );
}

// ---------------------------------------------------------------- execute

#[test]
fn execute_transfers_and_marks_executed() {
    let s = Setup::new();
    s.fund_vault(1_000);
    let to = Address::generate(&s.env);
    let id = s.propose(&s.approvers[0], &to, 400);
    s.approve(&s.approvers[1], id);

    assert_eq!(s.try_execute(id), Ok(()));
    assert_eq!(s.token.balance(&to), 400);
    assert_eq!(s.token.balance(&s.vault_id), 600);
    assert_eq!(s.vault.get_proposal(&id).status, ProposalStatus::Executed);
}

#[test]
fn execute_emits_executed_event() {
    let s = Setup::new();
    s.fund_vault(1_000);
    let to = Address::generate(&s.env);
    let id = s.propose(&s.approvers[0], &to, 400);
    s.approve(&s.approvers[1], id);
    s.try_execute(id).unwrap();
    let expected = events::Executed {
        proposal_id: id,
        token: s.token.address.clone(),
        to,
        amount: 400,
    };
    assert_eq!(
        s.env
            .events()
            .all()
            .filter_by_contract(&s.vault_id)
            .events(),
        [expected.to_xdr(&s.env, &s.vault_id)]
    );
}

#[test]
fn execute_rejects_threshold_not_met() {
    let s = Setup::new();
    s.fund_vault(1_000);
    let to = Address::generate(&s.env);
    let id = s.propose(&s.approvers[0], &to, 400);
    assert_eq!(s.try_execute(id), Err(Some(Error::ThresholdNotMet)));
    assert_eq!(s.token.balance(&s.vault_id), 1_000);
}

#[test]
fn execute_rejects_expired_proposal() {
    let s = Setup::new();
    s.fund_vault(1_000);
    let to = Address::generate(&s.env);
    let id = s.propose(&s.approvers[0], &to, 400);
    s.approve(&s.approvers[1], id);
    s.env.ledger().set_timestamp(NOW + TTL);
    assert_eq!(s.try_execute(id), Err(Some(Error::Expired)));
    assert_eq!(s.token.balance(&s.vault_id), 1_000);
}

#[test]
fn execute_rejects_double_execution() {
    let s = Setup::new();
    s.fund_vault(1_000);
    let to = Address::generate(&s.env);
    let id = s.propose(&s.approvers[0], &to, 400);
    s.approve(&s.approvers[1], id);
    s.try_execute(id).unwrap();
    assert_eq!(s.try_execute(id), Err(Some(Error::NotPending)));
    assert_eq!(s.token.balance(&to), 400);
}

#[test]
fn execute_rejects_insufficient_vault_balance() {
    let s = Setup::new();
    s.fund_vault(100);
    let to = Address::generate(&s.env);
    let id = s.propose(&s.approvers[0], &to, 400);
    s.approve(&s.approvers[1], id);
    assert_eq!(s.try_execute(id), Err(Some(Error::InsufficientBalance)));
    assert_eq!(s.vault.get_proposal(&id).status, ProposalStatus::Pending);
}

#[test]
fn execute_rejects_unknown_proposal() {
    let s = Setup::new();
    assert_eq!(s.try_execute(7), Err(Some(Error::ProposalNotFound)));
}

#[test]
fn execute_with_all_approvals_succeeds() {
    let s = Setup::new();
    s.fund_vault(1_000);
    let to = Address::generate(&s.env);
    let id = s.propose(&s.approvers[0], &to, 1_000);
    s.approve(&s.approvers[1], id);
    s.approve(&s.approvers[2], id);
    s.try_execute(id).unwrap();
    assert_eq!(s.token.balance(&s.vault_id), 0);
}

// ---------------------------------------------------------------- cancel

#[test]
fn cancel_marks_cancelled_and_emits_event() {
    let s = Setup::new();
    let to = Address::generate(&s.env);
    let id = s.propose(&s.approvers[0], &to, 10);
    assert_eq!(s.try_cancel(&s.approvers[2], &s.approvers[2], id), Ok(()));
    // `events().all()` only holds the latest invocation's events: check first.
    let expected = events::Cancelled {
        proposal_id: id,
        cancelled_by: s.approvers[2].clone(),
    };
    assert_eq!(
        s.env
            .events()
            .all()
            .filter_by_contract(&s.vault_id)
            .events(),
        [expected.to_xdr(&s.env, &s.vault_id)]
    );
    assert_eq!(s.vault.get_proposal(&id).status, ProposalStatus::Cancelled);
}

#[test]
fn cancel_blocks_later_approve_and_execute() {
    let s = Setup::new();
    s.fund_vault(1_000);
    let to = Address::generate(&s.env);
    let id = s.propose(&s.approvers[0], &to, 10);
    s.try_cancel(&s.approvers[0], &s.approvers[0], id).unwrap();
    assert_eq!(
        s.try_approve(&s.approvers[1], &s.approvers[1], id),
        Err(Some(Error::NotPending))
    );
    assert_eq!(s.try_execute(id), Err(Some(Error::NotPending)));
}

#[test]
fn cancel_allowed_after_expiry() {
    let s = Setup::new();
    let to = Address::generate(&s.env);
    let id = s.propose(&s.approvers[0], &to, 10);
    s.env.ledger().set_timestamp(NOW + TTL + 1);
    assert_eq!(s.try_cancel(&s.approvers[1], &s.approvers[1], id), Ok(()));
}

#[test]
fn cancel_rejects_non_approver() {
    let s = Setup::new();
    let to = Address::generate(&s.env);
    let outsider = Address::generate(&s.env);
    let id = s.propose(&s.approvers[0], &to, 10);
    assert_eq!(
        s.try_cancel(&outsider, &outsider, id),
        Err(Some(Error::NotApprover))
    );
}

#[test]
fn cancel_rejects_wrong_signer() {
    let s = Setup::new();
    let to = Address::generate(&s.env);
    let outsider = Address::generate(&s.env);
    let id = s.propose(&s.approvers[0], &to, 10);
    assert_eq!(s.try_cancel(&outsider, &s.approvers[1], id), Err(None));
    assert_eq!(s.vault.get_proposal(&id).status, ProposalStatus::Pending);
}

#[test]
fn cancel_rejects_executed_proposal() {
    let s = Setup::new();
    s.fund_vault(1_000);
    let to = Address::generate(&s.env);
    let id = s.propose(&s.approvers[0], &to, 10);
    s.approve(&s.approvers[1], id);
    s.try_execute(id).unwrap();
    assert_eq!(
        s.try_cancel(&s.approvers[0], &s.approvers[0], id),
        Err(Some(Error::NotPending))
    );
}

#[test]
fn cancel_rejects_already_cancelled() {
    let s = Setup::new();
    let to = Address::generate(&s.env);
    let id = s.propose(&s.approvers[0], &to, 10);
    s.try_cancel(&s.approvers[0], &s.approvers[0], id).unwrap();
    assert_eq!(
        s.try_cancel(&s.approvers[1], &s.approvers[1], id),
        Err(Some(Error::NotPending))
    );
}

#[test]
fn cancel_rejects_unknown_proposal() {
    let s = Setup::new();
    assert_eq!(
        s.try_cancel(&s.approvers[0], &s.approvers[0], 5),
        Err(Some(Error::ProposalNotFound))
    );
}

// ---------------------------------------------------------------- propose_policy_update

#[test]
fn propose_policy_update_creates_pending_policy_proposal() {
    let s = Setup::new();
    let new = s.new_approvers(4);
    let id = s.propose_policy(&s.approvers[0], &new, 3);
    let p = s.vault.get_policy_proposal(&id);
    assert_eq!(p.id, id);
    assert_eq!(p.new_policy.approvers, new);
    assert_eq!(p.new_policy.threshold, 3);
    assert_eq!(p.created_by, s.approvers[0]);
    assert_eq!(p.approvals, vec![&s.env, s.approvers[0].clone()]);
    assert_eq!(p.status, ProposalStatus::Pending);
    assert_eq!(p.expires_at, NOW + TTL);
    // The live policy is untouched until executed.
    assert_eq!(s.vault.get_policy().threshold, 2);
}

#[test]
fn propose_policy_update_emits_policy_proposed_event() {
    let s = Setup::new();
    let new = s.new_approvers(2);
    let id = s.propose_policy(&s.approvers[1], &new, 1);
    let expected = events::PolicyProposed {
        proposal_id: id,
        proposer: s.approvers[1].clone(),
        approvers: new,
        threshold: 1,
        expires_at: NOW + TTL,
    };
    assert_eq!(
        s.env
            .events()
            .all()
            .filter_by_contract(&s.vault_id)
            .events(),
        [expected.to_xdr(&s.env, &s.vault_id)]
    );
}

#[test]
fn policy_and_payment_proposals_share_id_sequence() {
    let s = Setup::new();
    let to = Address::generate(&s.env);
    let new = s.new_approvers(2);
    assert_eq!(s.propose(&s.approvers[0], &to, 1), 0);
    assert_eq!(s.propose_policy(&s.approvers[0], &new, 1), 1);
    assert_eq!(s.propose(&s.approvers[0], &to, 1), 2);
    assert_eq!(
        s.vault.try_get_proposal(&1),
        Err(Ok(Error::ProposalNotFound))
    );
    assert_eq!(
        s.vault.try_get_policy_proposal(&0),
        Err(Ok(Error::ProposalNotFound))
    );
}

#[test]
fn propose_policy_update_rejects_non_approver() {
    let s = Setup::new();
    let outsider = Address::generate(&s.env);
    let new = s.new_approvers(2);
    assert_eq!(
        s.try_propose_policy(&outsider, &outsider, &new, 1, NOW + TTL),
        Err(Some(Error::NotApprover))
    );
}

#[test]
fn propose_policy_update_rejects_wrong_signer() {
    let s = Setup::new();
    let new = s.new_approvers(2);
    assert_eq!(
        s.try_propose_policy(&s.approvers[1], &s.approvers[0], &new, 1, NOW + TTL),
        Err(None)
    );
}

#[test]
fn propose_policy_update_rejects_zero_threshold() {
    let s = Setup::new();
    let new = s.new_approvers(2);
    assert_eq!(
        s.try_propose_policy(&s.approvers[0], &s.approvers[0], &new, 0, NOW + TTL),
        Err(Some(Error::InvalidThreshold))
    );
}

#[test]
fn propose_policy_update_rejects_threshold_above_approvers() {
    let s = Setup::new();
    let new = s.new_approvers(2);
    assert_eq!(
        s.try_propose_policy(&s.approvers[0], &s.approvers[0], &new, 3, NOW + TTL),
        Err(Some(Error::InvalidThreshold))
    );
}

#[test]
fn propose_policy_update_rejects_empty_approver_set() {
    let s = Setup::new();
    let new = s.new_approvers(0);
    assert_eq!(
        s.try_propose_policy(&s.approvers[0], &s.approvers[0], &new, 1, NOW + TTL),
        Err(Some(Error::InvalidThreshold))
    );
}

#[test]
fn propose_policy_update_rejects_duplicate_approvers() {
    let s = Setup::new();
    let a = Address::generate(&s.env);
    let new = vec![&s.env, a.clone(), a];
    assert_eq!(
        s.try_propose_policy(&s.approvers[0], &s.approvers[0], &new, 1, NOW + TTL),
        Err(Some(Error::DuplicateApprover))
    );
}

#[test]
fn propose_policy_update_rejects_too_many_approvers() {
    let s = Setup::new();
    let new = s.new_approvers(crate::types::MAX_APPROVERS + 1);
    assert_eq!(
        s.try_propose_policy(&s.approvers[0], &s.approvers[0], &new, 1, NOW + TTL),
        Err(Some(Error::TooManyApprovers))
    );
}

#[test]
fn propose_policy_update_rejects_past_expiry() {
    let s = Setup::new();
    let new = s.new_approvers(2);
    assert_eq!(
        s.try_propose_policy(&s.approvers[0], &s.approvers[0], &new, 1, NOW),
        Err(Some(Error::InvalidExpiry))
    );
}

// ---------------------------------------------------------------- approve_policy_update

#[test]
fn approve_policy_update_records_approval_and_emits_event() {
    let s = Setup::new();
    let new = s.new_approvers(2);
    let id = s.propose_policy(&s.approvers[0], &new, 1);
    assert_eq!(
        s.try_approve_policy(&s.approvers[2], &s.approvers[2], id),
        Ok(())
    );
    let expected = events::Approved {
        proposal_id: id,
        approver: s.approvers[2].clone(),
        approvals: 2,
    };
    assert_eq!(
        s.env
            .events()
            .all()
            .filter_by_contract(&s.vault_id)
            .events(),
        [expected.to_xdr(&s.env, &s.vault_id)]
    );
    assert_eq!(
        s.vault.get_policy_proposal(&id).approvals,
        vec![&s.env, s.approvers[0].clone(), s.approvers[2].clone()]
    );
}

#[test]
fn approve_policy_update_rejects_double_approval() {
    let s = Setup::new();
    let new = s.new_approvers(2);
    let id = s.propose_policy(&s.approvers[0], &new, 1);
    assert_eq!(
        s.try_approve_policy(&s.approvers[0], &s.approvers[0], id),
        Err(Some(Error::AlreadyApproved))
    );
}

#[test]
fn approve_policy_update_rejects_non_approver() {
    let s = Setup::new();
    let new = s.new_approvers(2);
    let id = s.propose_policy(&s.approvers[0], &new, 1);
    // Being in the *proposed* set does not let you approve.
    let incoming = new.get(0).unwrap();
    assert_eq!(
        s.try_approve_policy(&incoming, &incoming, id),
        Err(Some(Error::NotApprover))
    );
}

#[test]
fn approve_policy_update_rejects_wrong_signer() {
    let s = Setup::new();
    let new = s.new_approvers(2);
    let id = s.propose_policy(&s.approvers[0], &new, 1);
    assert_eq!(
        s.try_approve_policy(&s.approvers[2], &s.approvers[1], id),
        Err(None)
    );
}

#[test]
fn approve_policy_update_rejects_expired() {
    let s = Setup::new();
    let new = s.new_approvers(2);
    let id = s.propose_policy(&s.approvers[0], &new, 1);
    s.env.ledger().set_timestamp(NOW + TTL);
    assert_eq!(
        s.try_approve_policy(&s.approvers[1], &s.approvers[1], id),
        Err(Some(Error::Expired))
    );
}

#[test]
fn approve_policy_update_rejects_payment_proposal_id() {
    let s = Setup::new();
    let to = Address::generate(&s.env);
    let id = s.propose(&s.approvers[0], &to, 1);
    assert_eq!(
        s.try_approve_policy(&s.approvers[1], &s.approvers[1], id),
        Err(Some(Error::ProposalNotFound))
    );
}

// ---------------------------------------------------------------- update_policy

#[test]
fn update_policy_applies_new_policy_and_emits_event() {
    let s = Setup::new();
    let new = s.new_approvers(4);
    let id = s.propose_policy(&s.approvers[0], &new, 3);
    s.try_approve_policy(&s.approvers[1], &s.approvers[1], id)
        .unwrap();

    assert_eq!(s.try_update_policy(id), Ok(()));
    let expected = events::PolicyUpdated {
        proposal_id: id,
        approvers: new.clone(),
        threshold: 3,
    };
    assert_eq!(
        s.env
            .events()
            .all()
            .filter_by_contract(&s.vault_id)
            .events(),
        [expected.to_xdr(&s.env, &s.vault_id)]
    );
    let policy = s.vault.get_policy();
    assert_eq!(policy.approvers, new);
    assert_eq!(policy.threshold, 3);
    assert_eq!(
        s.vault.get_policy_proposal(&id).status,
        ProposalStatus::Executed
    );
}

#[test]
fn update_policy_rejects_threshold_not_met() {
    let s = Setup::new();
    let new = s.new_approvers(2);
    let id = s.propose_policy(&s.approvers[0], &new, 1);
    assert_eq!(s.try_update_policy(id), Err(Some(Error::ThresholdNotMet)));
    assert_eq!(s.vault.get_policy().threshold, 2);
}

#[test]
fn update_policy_rejects_expired() {
    let s = Setup::new();
    let new = s.new_approvers(2);
    let id = s.propose_policy(&s.approvers[0], &new, 1);
    s.try_approve_policy(&s.approvers[1], &s.approvers[1], id)
        .unwrap();
    s.env.ledger().set_timestamp(NOW + TTL);
    assert_eq!(s.try_update_policy(id), Err(Some(Error::Expired)));
}

#[test]
fn update_policy_rejects_double_apply() {
    let s = Setup::new();
    let new = s.new_approvers(2);
    let id = s.propose_policy(&s.approvers[0], &new, 1);
    s.try_approve_policy(&s.approvers[1], &s.approvers[1], id)
        .unwrap();
    s.try_update_policy(id).unwrap();
    assert_eq!(s.try_update_policy(id), Err(Some(Error::NotPending)));
}

#[test]
fn update_policy_rejects_unknown_id() {
    let s = Setup::new();
    assert_eq!(s.try_update_policy(3), Err(Some(Error::ProposalNotFound)));
}

#[test]
fn removed_approvers_lose_power_after_update() {
    let s = Setup::new();
    let new = s.new_approvers(2);
    let id = s.propose_policy(&s.approvers[0], &new, 2);
    s.try_approve_policy(&s.approvers[1], &s.approvers[1], id)
        .unwrap();
    s.try_update_policy(id).unwrap();

    let to = Address::generate(&s.env);
    assert_eq!(
        s.try_propose(&s.approvers[0], &s.approvers[0], &to, 1, NOW + TTL),
        Err(Some(Error::NotApprover))
    );
    // New approvers can act.
    let n0 = new.get(0).unwrap();
    s.propose(&n0, &to, 1);
}

#[test]
fn stale_approvals_do_not_count_after_policy_change() {
    let s = Setup::new();
    s.fund_vault(1_000);
    let to = Address::generate(&s.env);
    // Payment proposal approved by two old approvers (meets old threshold 2).
    let pay = s.propose(&s.approvers[0], &to, 500);
    s.approve(&s.approvers[1], pay);

    // Replace the approver set entirely before executing.
    let new = s.new_approvers(2);
    let pol = s.propose_policy(&s.approvers[0], &new, 1);
    s.try_approve_policy(&s.approvers[1], &s.approvers[1], pol)
        .unwrap();
    s.try_update_policy(pol).unwrap();

    // The old approvals no longer count, so execute fails.
    assert_eq!(s.try_execute(pay), Err(Some(Error::ThresholdNotMet)));
    assert_eq!(s.token.balance(&s.vault_id), 1_000);
}

#[test]
fn competing_policy_proposal_needs_approvals_from_new_set() {
    let s = Setup::new();
    let set_a = s.new_approvers(2);
    let set_b = s.new_approvers(2);
    let a = s.propose_policy(&s.approvers[0], &set_a, 1);
    let b = s.propose_policy(&s.approvers[1], &set_b, 1);
    s.try_approve_policy(&s.approvers[1], &s.approvers[1], a)
        .unwrap();
    s.try_approve_policy(&s.approvers[2], &s.approvers[2], b)
        .unwrap();
    s.try_update_policy(a).unwrap();
    // b was approved by the old set only; under set_a (threshold 1) it has 0.
    assert_eq!(s.try_update_policy(b), Err(Some(Error::ThresholdNotMet)));
}

// ---------------------------------------------------------------- cancel_policy_update

#[test]
fn cancel_policy_update_marks_cancelled_and_emits_event() {
    let s = Setup::new();
    let new = s.new_approvers(2);
    let id = s.propose_policy(&s.approvers[0], &new, 1);
    assert_eq!(
        s.try_cancel_policy(&s.approvers[1], &s.approvers[1], id),
        Ok(())
    );
    let expected = events::Cancelled {
        proposal_id: id,
        cancelled_by: s.approvers[1].clone(),
    };
    assert_eq!(
        s.env
            .events()
            .all()
            .filter_by_contract(&s.vault_id)
            .events(),
        [expected.to_xdr(&s.env, &s.vault_id)]
    );
    assert_eq!(
        s.vault.get_policy_proposal(&id).status,
        ProposalStatus::Cancelled
    );
}

#[test]
fn cancel_policy_update_blocks_apply() {
    let s = Setup::new();
    let new = s.new_approvers(2);
    let id = s.propose_policy(&s.approvers[0], &new, 1);
    s.try_approve_policy(&s.approvers[1], &s.approvers[1], id)
        .unwrap();
    s.try_cancel_policy(&s.approvers[2], &s.approvers[2], id)
        .unwrap();
    assert_eq!(s.try_update_policy(id), Err(Some(Error::NotPending)));
}

#[test]
fn cancel_policy_update_rejects_non_approver() {
    let s = Setup::new();
    let outsider = Address::generate(&s.env);
    let new = s.new_approvers(2);
    let id = s.propose_policy(&s.approvers[0], &new, 1);
    assert_eq!(
        s.try_cancel_policy(&outsider, &outsider, id),
        Err(Some(Error::NotApprover))
    );
}

#[test]
fn cancel_policy_update_rejects_wrong_signer() {
    let s = Setup::new();
    let outsider = Address::generate(&s.env);
    let new = s.new_approvers(2);
    let id = s.propose_policy(&s.approvers[0], &new, 1);
    assert_eq!(
        s.try_cancel_policy(&outsider, &s.approvers[0], id),
        Err(None)
    );
}

#[test]
fn cancel_policy_update_rejects_applied_proposal() {
    let s = Setup::new();
    let new = s.new_approvers(2);
    let id = s.propose_policy(&s.approvers[0], &new, 1);
    s.try_approve_policy(&s.approvers[1], &s.approvers[1], id)
        .unwrap();
    s.try_update_policy(id).unwrap();
    let n0 = new.get(0).unwrap();
    assert_eq!(
        s.try_cancel_policy(&n0, &n0, id),
        Err(Some(Error::NotPending))
    );
}

#[test]
fn cancel_policy_update_rejects_unknown_id() {
    let s = Setup::new();
    assert_eq!(
        s.try_cancel_policy(&s.approvers[0], &s.approvers[0], 11),
        Err(Some(Error::ProposalNotFound))
    );
}

// ---------------------------------------------------------------- storage TTL

fn proposal_ttl(s: &Setup, id: u64) -> u32 {
    s.env.as_contract(&s.vault_id, || {
        s.env
            .storage()
            .persistent()
            .get_ttl(&crate::storage::DataKey::Proposal(id))
    })
}

#[test]
fn proposal_ttl_is_extended_on_write() {
    let s = Setup::new();
    let to = Address::generate(&s.env);
    let id = s.propose(&s.approvers[0], &to, 1);
    assert!(proposal_ttl(&s, id) >= finance_os_common::ttl::PERSISTENT_LIFETIME_THRESHOLD);
}

#[test]
fn proposal_ttl_is_extended_on_access() {
    let s = Setup::new();
    let to = Address::generate(&s.env);
    let id = s.propose(&s.approvers[0], &to, 1);
    let start = proposal_ttl(&s, id);

    // Advance far enough that the remaining TTL drops under the threshold.
    let step = 2 * finance_os_common::ttl::DAY_IN_LEDGERS;
    s.env.ledger().with_mut(|l| l.sequence_number += step);
    assert!(proposal_ttl(&s, id) < finance_os_common::ttl::PERSISTENT_LIFETIME_THRESHOLD);

    s.vault.get_proposal(&id);
    assert_eq!(proposal_ttl(&s, id), start);
}

#[test]
fn approve_prunes_approvals_from_removed_approvers() {
    let s = Setup::new();
    s.fund_vault(1_000);
    let to = Address::generate(&s.env);
    let pay = s.propose(&s.approvers[0], &to, 500);
    s.approve(&s.approvers[1], pay);

    // New policy keeps approvers[1] and approvers[2], drops approvers[0].
    let new = vec![&s.env, s.approvers[1].clone(), s.approvers[2].clone()];
    let pol = s.propose_policy(&s.approvers[0], &new, 2);
    s.try_approve_policy(&s.approvers[1], &s.approvers[1], pol)
        .unwrap();
    s.try_update_policy(pol).unwrap();

    s.approve(&s.approvers[2], pay);
    let expected = events::Approved {
        proposal_id: pay,
        approver: s.approvers[2].clone(),
        approvals: 2,
    };
    assert_eq!(
        s.env
            .events()
            .all()
            .filter_by_contract(&s.vault_id)
            .events(),
        [expected.to_xdr(&s.env, &s.vault_id)]
    );
    assert_eq!(
        s.vault.get_proposal(&pay).approvals,
        vec![&s.env, s.approvers[1].clone(), s.approvers[2].clone()]
    );
    // 2 valid approvals meet the new threshold of 2.
    assert_eq!(s.try_execute(pay), Ok(()));
}

// ---------------------------------------------------------------- edge cases

#[test]
fn single_approver_vault_executes_on_proposal_alone() {
    let env = Env::default();
    env.ledger().set_timestamp(NOW);
    let solo = Address::generate(&env);
    let vault_id = env.register(ApprovalVault, (vec![&env, solo.clone()], 1u32));
    let vault = ApprovalVaultClient::new(&env, &vault_id);
    let token_admin = Address::generate(&env);
    let sac = env.register_stellar_asset_contract_v2(token_admin.clone());
    let token = TokenClient::new(&env, &sac.address());

    let args: soroban_sdk::Vec<Val> = (vault_id.clone(), 100i128).into_val(&env);
    auth(&env, &token_admin, &token.address, "mint", args, &[]);
    StellarAssetClient::new(&env, &token.address).mint(&vault_id, &100);

    let to = Address::generate(&env);
    let memo = BytesN::from_array(&env, &[1; 32]);
    let args = (
        solo.clone(),
        token.address.clone(),
        to.clone(),
        100i128,
        memo.clone(),
        NOW + TTL,
    )
        .into_val(&env);
    auth(&env, &solo, &vault_id, "propose", args, &[]);
    let id = vault.propose(&solo, &token.address, &to, &100, &memo, &(NOW + TTL));

    env.mock_auths(&[]);
    vault.execute(&id);
    assert_eq!(token.balance(&to), 100);
}

#[test]
fn approve_after_execute_is_rejected() {
    let s = Setup::new();
    s.fund_vault(100);
    let to = Address::generate(&s.env);
    let id = s.propose(&s.approvers[0], &to, 100);
    s.approve(&s.approvers[1], id);
    s.try_execute(id).unwrap();
    assert_eq!(
        s.try_approve(&s.approvers[2], &s.approvers[2], id),
        Err(Some(Error::NotPending))
    );
}

#[test]
fn balances_are_tracked_per_token() {
    let s = Setup::new();
    let other_admin = Address::generate(&s.env);
    let other = s
        .env
        .register_stellar_asset_contract_v2(other_admin.clone());
    let args: soroban_sdk::Vec<Val> = (s.vault_id.clone(), 7i128).into_val(&s.env);
    auth(&s.env, &other_admin, &other.address(), "mint", args, &[]);
    StellarAssetClient::new(&s.env, &other.address()).mint(&s.vault_id, &7);

    s.fund_vault(50);
    assert_eq!(s.vault.balance(&s.token.address), 50);
    assert_eq!(s.vault.balance(&other.address()), 7);
}

#[test]
fn execute_pays_only_the_proposed_token() {
    let s = Setup::new();
    let other_admin = Address::generate(&s.env);
    let other = s
        .env
        .register_stellar_asset_contract_v2(other_admin.clone());
    let args: soroban_sdk::Vec<Val> = (s.vault_id.clone(), 500i128).into_val(&s.env);
    auth(&s.env, &other_admin, &other.address(), "mint", args, &[]);
    StellarAssetClient::new(&s.env, &other.address()).mint(&s.vault_id, &500);
    s.fund_vault(500);

    let to = Address::generate(&s.env);
    let id = s.propose(&s.approvers[0], &to, 200);
    s.approve(&s.approvers[1], id);
    s.try_execute(id).unwrap();

    let other_token = TokenClient::new(&s.env, &other.address());
    assert_eq!(s.token.balance(&to), 200);
    assert_eq!(other_token.balance(&to), 0);
    assert_eq!(other_token.balance(&s.vault_id), 500);
}

#[test]
fn execute_drains_vault_exactly() {
    let s = Setup::new();
    s.fund_vault(300);
    let to = Address::generate(&s.env);
    let id = s.propose(&s.approvers[0], &to, 300);
    s.approve(&s.approvers[2], id);
    assert_eq!(s.try_execute(id), Ok(()));
    assert_eq!(s.token.balance(&s.vault_id), 0);
}

#[test]
fn two_proposals_cannot_overspend_vault() {
    let s = Setup::new();
    s.fund_vault(500);
    let to = Address::generate(&s.env);
    let a = s.propose(&s.approvers[0], &to, 400);
    let b = s.propose(&s.approvers[1], &to, 400);
    s.approve(&s.approvers[1], a);
    s.approve(&s.approvers[0], b);
    s.try_execute(a).unwrap();
    assert_eq!(s.try_execute(b), Err(Some(Error::InsufficientBalance)));
    assert_eq!(s.vault.get_proposal(&b).status, ProposalStatus::Pending);
}

#[test]
fn insufficient_balance_proposal_executes_after_top_up() {
    let s = Setup::new();
    s.fund_vault(100);
    let to = Address::generate(&s.env);
    let id = s.propose(&s.approvers[0], &to, 250);
    s.approve(&s.approvers[1], id);
    assert_eq!(s.try_execute(id), Err(Some(Error::InsufficientBalance)));
    s.fund_vault(150);
    assert_eq!(s.try_execute(id), Ok(()));
    assert_eq!(s.token.balance(&to), 250);
}

#[test]
fn retained_approvers_keep_pending_approvals_after_policy_update() {
    let s = Setup::new();
    s.fund_vault(100);
    let to = Address::generate(&s.env);
    let pay = s.propose(&s.approvers[1], &to, 100);
    s.approve(&s.approvers[2], pay);

    // Drop approvers[0] only; [1] and [2] stay, threshold stays 2.
    let new = vec![&s.env, s.approvers[1].clone(), s.approvers[2].clone()];
    let pol = s.propose_policy(&s.approvers[0], &new, 2);
    s.try_approve_policy(&s.approvers[1], &s.approvers[1], pol)
        .unwrap();
    s.try_update_policy(pol).unwrap();

    assert_eq!(s.try_execute(pay), Ok(()));
}

#[test]
fn raising_threshold_blocks_previously_sufficient_proposal() {
    let s = Setup::new();
    s.fund_vault(100);
    let to = Address::generate(&s.env);
    let pay = s.propose(&s.approvers[0], &to, 100);
    s.approve(&s.approvers[1], pay);

    let same = vec![
        &s.env,
        s.approvers[0].clone(),
        s.approvers[1].clone(),
        s.approvers[2].clone(),
    ];
    let pol = s.propose_policy(&s.approvers[0], &same, 3);
    s.try_approve_policy(&s.approvers[1], &s.approvers[1], pol)
        .unwrap();
    s.try_update_policy(pol).unwrap();

    assert_eq!(s.try_execute(pay), Err(Some(Error::ThresholdNotMet)));
    s.approve(&s.approvers[2], pay);
    assert_eq!(s.try_execute(pay), Ok(()));
}

#[test]
fn approve_policy_update_rejects_cancelled() {
    let s = Setup::new();
    let new = s.new_approvers(2);
    let id = s.propose_policy(&s.approvers[0], &new, 1);
    s.try_cancel_policy(&s.approvers[0], &s.approvers[0], id)
        .unwrap();
    assert_eq!(
        s.try_approve_policy(&s.approvers[1], &s.approvers[1], id),
        Err(Some(Error::NotPending))
    );
}

#[test]
fn get_policy_proposal_rejects_unknown_id() {
    let s = Setup::new();
    assert_eq!(
        s.vault.try_get_policy_proposal(&0),
        Err(Ok(Error::ProposalNotFound))
    );
}

// ---------------------------------------------------------------- recorded auth trees

fn contract_call(
    env: &Env,
    contract: &Address,
    fn_name: &str,
    args: soroban_sdk::Vec<Val>,
    subs: std::vec::Vec<soroban_sdk::testutils::AuthorizedInvocation>,
) -> soroban_sdk::testutils::AuthorizedInvocation {
    soroban_sdk::testutils::AuthorizedInvocation {
        function: soroban_sdk::testutils::AuthorizedFunction::Contract((
            contract.clone(),
            soroban_sdk::Symbol::new(env, fn_name),
            args,
        )),
        sub_invocations: subs,
    }
}

#[test]
fn propose_requires_exactly_the_proposers_auth() {
    let s = Setup::new();
    let to = Address::generate(&s.env);
    let id = s.propose(&s.approvers[0], &to, 10);
    let args = (
        s.approvers[0].clone(),
        s.token.address.clone(),
        to,
        10i128,
        s.memo(1),
        NOW + TTL,
    )
        .into_val(&s.env);
    assert_eq!(id, 0);
    assert_eq!(
        s.env.auths(),
        std::vec![(
            s.approvers[0].clone(),
            contract_call(&s.env, &s.vault_id, "propose", args, std::vec![])
        )]
    );
}

#[test]
fn deposit_requires_depositor_auth_for_vault_and_token_transfer() {
    let s = Setup::new();
    s.fund_vault(25);
    let auths = s.env.auths();
    assert_eq!(auths.len(), 1);
    let (signer, invocation) = &auths[0];
    let transfer_args = (signer.clone(), s.vault_id.clone(), 25i128).into_val(&s.env);
    let deposit_args = (signer.clone(), s.token.address.clone(), 25i128).into_val(&s.env);
    assert_eq!(
        invocation,
        &contract_call(
            &s.env,
            &s.vault_id,
            "deposit",
            deposit_args,
            std::vec![contract_call(
                &s.env,
                &s.token.address,
                "transfer",
                transfer_args,
                std::vec![]
            )]
        )
    );
}

#[test]
fn execute_requires_no_auth() {
    let s = Setup::new();
    s.fund_vault(10);
    let to = Address::generate(&s.env);
    let id = s.propose(&s.approvers[0], &to, 10);
    s.approve(&s.approvers[1], id);
    s.try_execute(id).unwrap();
    assert_eq!(s.env.auths(), std::vec![]);
}

// ---------------------------------------------------------------- docs/events.md

#[test]
fn event_topic_xdr_matches_docs() {
    use soroban_sdk::xdr::{ContractEventBody, Limits, ScVal, WriteXdr};
    let env = Env::default();
    let id = Address::generate(&env);
    let a = Address::generate(&env);
    let topic0 = |ev: soroban_sdk::xdr::ContractEvent| -> (std::string::String, ScVal) {
        let ContractEventBody::V0(body) = ev.body;
        (
            body.topics[0].to_xdr_base64(Limits::none()).unwrap(),
            body.data,
        )
    };
    let cases: std::vec::Vec<(soroban_sdk::xdr::ContractEvent, &str)> = std::vec![
        (
            events::Proposed {
                proposal_id: 0,
                proposer: a.clone(),
                token: a.clone(),
                to: a.clone(),
                amount: 1,
                memo_ref: BytesN::from_array(&env, &[0; 32]),
                expires_at: 1
            }
            .to_xdr(&env, &id),
            "AAAADwAAAAhwcm9wb3NlZA==",
        ),
        (
            events::Deposited {
                token: a.clone(),
                from: a.clone(),
                amount: 1
            }
            .to_xdr(&env, &id),
            "AAAADwAAAAlkZXBvc2l0ZWQAAAA=",
        ),
        (
            events::PolicyProposed {
                proposal_id: 0,
                proposer: a.clone(),
                approvers: vec![&env],
                threshold: 1,
                expires_at: 1
            }
            .to_xdr(&env, &id),
            "AAAADwAAAA9wb2xpY3lfcHJvcG9zZWQA",
        ),
        (
            events::Approved {
                proposal_id: 0,
                approver: a.clone(),
                approvals: 1
            }
            .to_xdr(&env, &id),
            "AAAADwAAAAhhcHByb3ZlZA==",
        ),
        (
            events::Executed {
                proposal_id: 0,
                token: a.clone(),
                to: a.clone(),
                amount: 1
            }
            .to_xdr(&env, &id),
            "AAAADwAAAAhleGVjdXRlZA=="
        ),
        (
            events::Cancelled {
                proposal_id: 0,
                cancelled_by: a.clone()
            }
            .to_xdr(&env, &id),
            "AAAADwAAAAljYW5jZWxsZWQAAAA=",
        ),
        (
            events::PolicyUpdated {
                proposal_id: 0,
                approvers: vec![&env],
                threshold: 1
            }
            .to_xdr(&env, &id),
            "AAAADwAAAA5wb2xpY3lfdXBkYXRlZAAA",
        ),
    ];
    for (ev, expected) in cases {
        let (got, data) = topic0(ev);
        assert_eq!(got, expected);
        // Data is always a map, with keys in sorted order.
        let ScVal::Map(Some(map)) = data else {
            panic!("data is not a map: {data:?}")
        };
        let keys: std::vec::Vec<_> = map.iter().map(|e| e.key.clone()).collect();
        let mut sorted = keys.clone();
        sorted.sort();
        assert_eq!(keys, sorted);
    }
}
