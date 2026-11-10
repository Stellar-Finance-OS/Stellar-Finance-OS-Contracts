//! Events emitted by the approval vault. Consumed by the backend indexer.
//!
//! **Changing any of these shapes is a breaking change**: update README §3
//! and bump the major version.
//!
//! Topic layout: `[<event_name>, proposal_id]`. The event name is the struct
//! name in snake_case. `proposal_id` is a topic so the indexer can filter on it.
//! Exception: `deposited` has no proposal, so its second topic is the token.
//!
//! Every event carries enough data for the indexer to update its mirror
//! without a follow-up `get_*` read.

use soroban_sdk::{contractevent, Address, BytesN, Vec};

#[contractevent]
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Proposed {
    #[topic]
    pub proposal_id: u64,
    pub proposer: Address,
    pub token: Address,
    pub to: Address,
    pub amount: i128,
    pub memo_ref: BytesN<32>,
    pub expires_at: u64,
}

/// `amount` of `token` was deposited into the vault by `from`.
#[contractevent]
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Deposited {
    #[topic]
    pub token: Address,
    pub from: Address,
    pub amount: i128,
}

/// A policy-update proposal was created. `approved` and `cancelled` are shared
/// with payment proposals (ids are unique across both kinds).
#[contractevent]
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct PolicyProposed {
    #[topic]
    pub proposal_id: u64,
    pub proposer: Address,
    pub approvers: Vec<Address>,
    pub threshold: u32,
    pub expires_at: u64,
}

#[contractevent]
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Approved {
    #[topic]
    pub proposal_id: u64,
    pub approver: Address,
    pub approvals: u32,
}

#[contractevent]
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Executed {
    #[topic]
    pub proposal_id: u64,
    pub token: Address,
    pub to: Address,
    pub amount: i128,
}

#[contractevent]
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Cancelled {
    #[topic]
    pub proposal_id: u64,
    pub cancelled_by: Address,
}

#[contractevent]
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct PolicyUpdated {
    #[topic]
    pub proposal_id: u64,
    pub approvers: Vec<Address>,
    pub threshold: u32,
}
