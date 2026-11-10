//! Events emitted by the invoice registry. Consumed by the backend indexer.
//! The `paid` event is the backend's `contract_event` payment-match method.
//!
//! **Changing any of these shapes is a breaking change**: update README §4
//! and bump the major version.
//!
//! Topic layout: `[<event_name>, id]`. The event name is the struct name in snake_case.
//! An invoice is identified by `(business, id)`: every event carries `business`
//! in its data, so the indexer must key on both.

use soroban_sdk::{contractevent, Address, BytesN};

#[contractevent]
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Registered {
    #[topic]
    pub id: BytesN<32>,
    pub business: Address,
    pub token: Address,
    pub amount: i128,
    pub content_hash: BytesN<32>,
    pub payee: Address,
}

/// Funds went to the invoice's `payee` (see `Registered`), not necessarily
/// to `business`.
#[contractevent]
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Paid {
    #[topic]
    pub id: BytesN<32>,
    pub business: Address,
    pub payer: Address,
    pub amount: i128,
    pub total_paid: i128,
}

#[contractevent]
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Voided {
    #[topic]
    pub id: BytesN<32>,
    pub business: Address,
}

#[contractevent]
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Closed {
    #[topic]
    pub id: BytesN<32>,
    pub business: Address,
    pub paid: i128,
    pub outstanding: i128,
}
