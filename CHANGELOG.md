# Changelog

All notable changes to the contracts' public surface (functions, events,
error codes) and tooling. Format: [Keep a Changelog](https://keepachangelog.com/en/1.1.0/).
Versioning: [SemVer](https://semver.org/). A change to a function signature,
event shape or error code is a **major** bump.

## [Unreleased]

### Breaking
- `invoice-registry`: invoices are keyed by `(business, id)` instead of `id`
  alone, closing invoice-id squatting (a third party registering an `id` first
  could receive payments meant for the real business).
  - `pay(payer, id, amount)` → `pay(payer, business, id, amount)`.
  - `get(id)` → `get(business, id)`.
  - `register` and `void` keep their signatures; `register` returns
    `InvoiceExists` only when the same business re-registers the same `id`,
    and `void` on another business's invoice now returns `InvoiceNotFound`.
  - `paid` and `voided` events gain a `business` data field (topics unchanged).
  - Error 9 `NotBusiness` is reserved and no longer returned.
  - The indexer must key invoices on `(business, id)`. Generated bindings
    need regenerating (`./scripts/bindings.sh`).
- Events are now self-sufficient for the backend indexer (#6): the indexer
  can update its mirror from each event alone, with no follow-up `get_*` read.
  See the migration note in `docs/events.md`.
  - `approval-vault`: new `deposited { #[topic] token, from, amount }` event,
    emitted once by `deposit`.
  - `approval-vault`: `proposed` gains `memo_ref`, `expires_at`.
  - `approval-vault`: `executed` gains `token`, `to`, `amount`.
  - `approval-vault`: `policy_proposed` gains `expires_at`.
  - `invoice-registry`: `registered` gains `content_hash`.
  - Workspace version bumped to 1.0.0.

- `invoice-registry`: invoices have a `payee` (issue #4), so revenue can be
  paid straight into an `approval-vault`.
  - `register(business, id, token, amount, content_hash)` →
    `register(business, id, token, amount, content_hash, payee)`.
  - `Invoice` gains `payee`; `pay` transfers payer → `payee`. `business` keeps
    sole control of `void` and `close`.
  - `registered` event gains a `payee` data field; `closed` gains `business`
    (topics unchanged).
  - New error 11 `InvalidPayee`: `payee` is the registry itself.

### Added
- `invoice-registry`: `close(business, id)`, `InvoiceStatus::Closed`, event `closed { business, paid, outstanding }`, error 10 `NothingPaid`. Lets a business finalise a partially paid invoice. Like `void`, `close` looks the invoice up by `(business, id)`, so another business gets `InvoiceNotFound`.
- `invoice-registry`: `close` entrypoint, `InvoiceStatus::Closed`, `closed`
  event and error 10 `NothingPaid`, so a partially paid invoice can reach a
  terminal state.
- `approval-vault` error 14 `InvalidToken`: `propose` now rejects a `token`
  that does not implement the token interface, instead of letting the
  proposal collect approvals and fail in `execute` with a host error.
- `approval-vault`: constructor, `deposit`, `propose`, `approve`, `execute`,
  `cancel`, `propose_policy_update`, `approve_policy_update`, `update_policy`,
  `cancel_policy_update`, and views `get_policy`, `get_proposal`,
  `get_policy_proposal`, `balance`.
- `approval-vault` events: `proposed`, `policy_proposed`, `approved`,
  `executed`, `cancelled`, `policy_updated`.
- `approval-vault` errors 1–13 (see README §3).
- `invoice-registry`: `register`, `pay`, `void`, `get`.
- `invoice-registry` events: `registered`, `paid`, `voided`.
- `invoice-registry` errors 1–9 (see README §4).
- Shared TTL policy (30-day bump, 1-day threshold) in `packages/common`.
- Scripts: `build`, `check`, `coverage`, `deploy`, `bindings`.
- CI: fmt/clippy/test gate, Wasm build with size limit, coverage, cargo-audit.
- Workflows: manual testnet deploy, tag-triggered release with checksums.
- Docs: threat model, coverage report, security policy, contributing guide,
  event decoding guide for the indexer, testnet runbook.
- Tests: property tests for M-of-N execution, policy validation and invoice
  payment invariants; exact auth-tree assertions; event XDR pinned to docs.
- Build: contract meta records `source_repo` and `source_rev`; deployments
  record `sourceRev` and `cliVersion`.

### Reproducibility
- Rust toolchain pinned to 1.99.1 and CI runners to ubuntu-24.04.
- `scripts/verify-deployment.sh` compares on-chain Wasm hashes with the
  deployment record and the local build; the testnet deploy workflow runs it.
- `docs/verifying-builds.md` explains how to verify a deployment from source.

### Deviations from the original design
- Policy changes use a dedicated proposal flow (`propose_policy_update` →
  `approve_policy_update` → `update_policy(id)`) instead of
  `update_policy(new_approvers, new_threshold)`.
- `pay` rejects overpayment instead of accepting `paid > amount`.
