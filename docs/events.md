# Events: guide for the backend indexer

The backend indexer mirrors contract state by reading events from Stellar RPC
`getEvents`. This page describes exactly what each event looks like on the wire.
The README tables (§3, §4) are the canonical list. **Any change here is a
breaking change and needs a major version bump.**

## Shape

Every event is a Soroban contract event with:

- **topics** (2 entries): `[Symbol(<event name>), <id>]`
  - vault: `<id>` is `U64(proposal_id)`, except `deposited`, where it is
    `Address(token)` (deposits have no proposal id)
  - registry: `<id>` is `Bytes(32)` (the invoice id hash)
- **data**: an `ScMap` keyed by `Symbol(<field name>)`, holding the remaining
  fields in alphabetical key order. `voided` has no data fields and carries an
  empty map.

Every event is **self-sufficient**: the indexer can update its mirror from the
event alone, with no follow-up `get_*` read (which would race with later state
changes and fail once the entry is archived).

## Topic filters

`getEvents` topic filters take base64 XDR `ScVal`s. Use `"*"` for the id
position to match all ids.

| Contract | Event | Topic 0 (base64 XDR) |
|---|---|---|
| approval-vault | `deposited` | `AAAADwAAAAlkZXBvc2l0ZWQAAAA=` |
| approval-vault | `proposed` | `AAAADwAAAAhwcm9wb3NlZA==` |
| approval-vault | `policy_proposed` | `AAAADwAAAA9wb2xpY3lfcHJvcG9zZWQA` |
| approval-vault | `approved` | `AAAADwAAAAhhcHByb3ZlZA==` |
| approval-vault | `executed` | `AAAADwAAAAhleGVjdXRlZA==` |
| approval-vault | `cancelled` | `AAAADwAAAAljYW5jZWxsZWQAAAA=` |
| approval-vault | `policy_updated` | `AAAADwAAAA5wb2xpY3lfdXBkYXRlZAAA` |
| invoice-registry | `registered` | `AAAADwAAAApyZWdpc3RlcmVkAAA=` |
| invoice-registry | `paid` | `AAAADwAAAARwYWlk` |
| invoice-registry | `voided` | `AAAADwAAAAZ2b2lkZWQAAA==` |
| invoice-registry | `closed` | `AAAADwAAAAZjbG9zZWQAAA==` |

These values are checked by tests (`event_topic_xdr_matches_docs` in each
contract), so this table can't silently drift from the code.

### Example: all `paid` events from the registry

```json
{
  "jsonrpc": "2.0",
  "id": 1,
  "method": "getEvents",
  "params": {
    "startLedger": 123456,
    "filters": [
      {
        "type": "contract",
        "contractIds": ["<invoiceRegistry from deployments/testnet.json>"],
        "topics": [["AAAADwAAAARwYWlk", "*"]]
      }
    ]
  }
}
```

## Data fields

### approval-vault

| Event | Fields (ScVal type) |
|---|---|
| `deposited` | `amount` (I128), `from` (Address). Topic 1 is `token` (Address) |
| `proposed` | `amount` (I128), `expires_at` (U64), `memo_ref` (Bytes(32)), `proposer` (Address), `to` (Address), `token` (Address) |
| `policy_proposed` | `approvers` (Vec\<Address\>), `expires_at` (U64), `proposer` (Address), `threshold` (U32) |
| `approved` | `approvals` (U32), `approver` (Address) |
| `executed` | `amount` (I128), `to` (Address), `token` (Address) |
| `cancelled` | `cancelled_by` (Address) |
| `policy_updated` | `approvers` (Vec\<Address\>), `threshold` (U32) |

### invoice-registry

| Event | Fields (ScVal type) |
|---|---|
| `registered` | `amount` (I128), `business` (Address), `content_hash` (Bytes(32)), `token` (Address) |
| `registered` | `amount` (I128), `business` (Address), `token` (Address) |
| `registered` | `amount` (I128), `business` (Address), `payee` (Address), `token` (Address) |
| `paid` | `amount` (I128), `business` (Address), `payer` (Address), `total_paid` (I128) |
| `voided` | `business` (Address) |
| `closed` | `business` (Address), `outstanding` (I128), `paid` (I128) |

## Mirroring rules

**Vault proposals**

- `deposited`: record a treasury inflow of `amount` of `token` from `from`.
  The vault's balance of `token` increases by `amount`.
- `proposed`: create the record (Pending, 1 approval, `created_by = proposer`)
  with `token`, `to`, `amount`, `memo_ref` and `expires_at`. Match it to the
  backend `PaymentRequest` via `memo_ref`.
- `policy_proposed`: create the record (Pending, 1 approval) with the new
  `approvers`, `threshold` and `expires_at`.
- `approved`: set the approval count to `approvals`. It's the authoritative count *after* stale approvers were pruned, so don't increment a local counter.
- `executed`: mark Executed. `token`, `to`, `amount` describe the payout; the
  vault's balance of `token` decreases by `amount`.
- `cancelled`: mark Cancelled. Look up the id to tell whether it was a payment or a policy proposal: ids are unique across both kinds.
- `policy_updated`: replace the cached approver set and threshold.

**Invoices**

- An invoice is identified by `(business, id)`; the same `id` can exist under several businesses. Key your records on both.
- `registered`: create the record (Open, `paid = 0`) with `business`, `token`,
  `amount` and `content_hash`. Compare `content_hash` with the backend
  document to verify the on-chain invoice.
- `paid`: look up `(business, id)` and set `paid = total_paid`. Mark Paid when `total_paid` equals the registered `amount`.
- `registered`: create the record (Open) and store its `payee`: every payment for this invoice lands there (may differ from `business`, e.g. an approval vault).
- `paid`: look up `(business, id)` and set `paid = total_paid`. Mark Paid when `total_paid` equals the registered `amount`. The funds went to the stored `payee`.
- `voided`: look up `(business, id)` and mark Void.
- `closed`: look up `(business, id)` and mark Closed (terminal). `paid` is final and `outstanding == amount - paid`; no further `paid` events will occur.

**Trust**

- Only act on events whose contract id matches `deployments/<network>.json`.
- Only treat `paid` as a real payment when the invoice's `token` is on your
  allow-list (e.g. the USDC SAC). See the threat model, R8.

**Expiry has no event.** A Pending proposal whose `expires_at` has passed is
effectively dead. Derive that from `expires_at` and the ledger close time.

## Migration notes

### 1.0.0 (from 0.1.x)

Breaking: data maps gained fields and the vault gained an event. Topics of
existing events are unchanged, so existing topic filters keep working; decoders
that reject unknown map keys must be updated.

| Contract | Event | Old data fields | New data fields |
|---|---|---|---|
| approval-vault | `deposited` | (event did not exist; deposits were only visible as a SAC `transfer`) | `amount`, `from`; topic 1 is `token` |
| approval-vault | `proposed` | `amount`, `proposer`, `to`, `token` | + `expires_at`, `memo_ref` |
| approval-vault | `executed` | (none) | `amount`, `to`, `token` |
| approval-vault | `policy_proposed` | `approvers`, `proposer`, `threshold` | + `expires_at` |
| invoice-registry | `registered` | `amount`, `business`, `token` | + `content_hash` |
| invoice-registry | `paid` | `amount`, `payer`, `total_paid` | + `business` (invoices are keyed by `(business, id)`) |
| invoice-registry | `voided` | (none) | `business` |
| invoice-registry | `closed` | (event did not exist) | `business`, `outstanding`, `paid` |

Indexer changes:

- Subscribe to `deposited` and stop inferring deposits from SAC transfers to
  the vault.
- Drop the follow-up `get_proposal` / `get_policy_proposal` / `get` reads after
  `proposed`, `policy_proposed`, `executed` and `registered`; read the new
  fields from the event instead.
