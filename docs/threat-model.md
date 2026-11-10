# Threat model — finance-os-contracts v0.1

Scope: `contracts/approval-vault`, `contracts/invoice-registry`, `packages/common`.
Status: **pre-audit, testnet only.** Not upgradeable (no `update_current_contract_wasm` path).

## Assets

| Asset | Where | Worst case |
|---|---|---|
| Treasury tokens | vault's SAC balances | Drained without M-of-N approval |
| Approver policy | vault instance storage | Captured by a minority, or bricked (no reachable threshold) |
| Invoice payment state | registry persistent storage | Payment attributed to the wrong invoice; invoice marked Paid without funds moving |
| Business data | off-chain only | Leaked on-chain |

## Actors

- **Approver**: a member of the current `Policy.approvers` list. Trusted only collectively, up to `threshold`.
- **Business**: registers and voids its own invoices.
- **Payer**: any account paying an invoice.
- **Anyone**: can call `execute`, `update_policy` and the views.
- **Token contract**: the address supplied as `token`. Trusted only as much as the party who chose it.
- **Deployer**: has no privileges after deployment (no admin key).

## Threats and mitigations

### approval-vault

| # | Threat | Mitigation | Test(s) |
|---|---|---|---|
| V1 | A non-approver proposes, approves or cancels | `require_auth` on the acting address, plus a membership check | `*_rejects_non_approver`, `*_rejects_wrong_signer` |
| V2 | One approver counts twice | `approvals` holds unique addresses; `AlreadyApproved` | `approve_rejects_double_approval`, `approve_rejects_proposer_approving_again` |
| V3 | Executing below threshold | `count_valid_approvals >= threshold`, checked in `execute`/`update_policy` | `execute_rejects_threshold_not_met`, `update_policy_rejects_threshold_not_met`, property test `executes_iff_threshold_met` |
| V4 | Replay / double execution | Status is set to Executed before the transfer; `NotPending` | `execute_rejects_double_execution`, `update_policy_rejects_double_apply` |
| V5 | Stale approvals after the approver set changes | Approvals are re-filtered against the *current* policy at execution time and pruned on every approve, so the list stays ≤ `MAX_APPROVERS` | `stale_approvals_do_not_count_after_policy_change`, `competing_policy_proposal_needs_approvals_from_new_set`, `approve_prunes_approvals_from_removed_approvers` |
| V6 | Executing an old proposal long after intent | `expires_at` must be in the future at creation; approve/execute reject once `now >= expires_at` | `*_rejects_expired`, `propose_rejects_expiry_*` |
| V7 | Policy bricked (threshold > approvers, empty set) | `validate_policy` on construction and on every policy proposal | `constructor_rejects_*`, `propose_policy_update_rejects_*`, property test `policy_validation_matches_spec` |
| V8 | Unbounded approver set exhausts resource limits | `MAX_APPROVERS = 20` | `constructor_rejects_too_many_approvers` |
| V9 | Re-entrancy through a malicious token | State is written before the external `transfer`; Soroban also forbids re-entering the same contract | Code review (ordering in `execute`) |
| V10 | Malicious or bogus `token` on a proposal | `propose` rejects addresses that do not answer `decimals()`. Approvers approve the full proposal, including `token`, and only vault-held balances of that token can move | `propose_rejects_unknown_token`, `propose_rejects_non_token_contract` |
| V11 | Proposal archived (TTL) while still needed | Every read/write bumps it to 30 days; archived entries can be restored | `proposal_ttl_is_extended_on_*` |
| V12 | Id overflow | `checked_add` on `NextProposalId` | Code review |
| V13 | Two approved proposals together overspend the vault | Each `execute` re-checks the live balance; the second one fails and stays Pending | `two_proposals_cannot_overspend_vault`, `insufficient_balance_proposal_executes_after_top_up` |
| V14 | Hidden extra or missing `require_auth` | The recorded auth trees are asserted exactly | `propose_requires_exactly_the_proposers_auth`, `deposit_requires_depositor_auth_for_vault_and_token_transfer`, `execute_requires_no_auth` |
| V15 | Fake token emits a `deposited` event that looks like a treasury inflow | `deposit` accepts any token address, so `deposited` is only emitted after that token's `transfer` succeeds. **The indexer must only trust `deposited` for allow-listed token addresses** (same rule as R8) | `deposit_emits_one_deposited_event`, `deposit_rejects_unknown_token` |

### invoice-registry

| # | Threat | Mitigation | Test(s) |
|---|---|---|---|
| R1 | Registering an invoice for someone else's business | `business.require_auth()` | `register_rejects_wrong_signer` |
| R2 | Invoice-id squatting redirects client payments | Invoices are keyed by `(business, id)` and `pay` names the business. A squatter registering the same `id` creates a different invoice; `pay(client, acme, id, x)` can only credit `acme`. `InvoiceExists` is returned only when the same business re-registers an id | `squatter_cannot_receive_payment_meant_for_real_business`, `paying_a_business_that_never_registered_the_id_fails`, `two_businesses_can_register_the_same_id_independently`, `register_duplicate_error_only_for_same_business`, `register_rejects_duplicate_id`, property test `payments_only_credit_the_named_business` |
| R3 | Invoice marked Paid without funds moving | State and transfer happen in one transaction, and a failed transfer reverts everything | `pay_fails_atomically_with_insufficient_funds` |
| R4 | Overpayment locks extra funds / skews accounting | `Overpayment` once `paid + amount > amount` | `pay_rejects_overpayment_*`, property test `paid_never_exceeds_amount` |
| R5 | Paying from someone else's account | `payer.require_auth()`; the transfer is from `payer` | `pay_rejects_wrong_signer` |
| R6 | Voiding a partly paid invoice (hides received funds) | `AlreadyPaid` when `paid != 0` | `void_rejects_after_partial_payment` |
| R7 | Another business voids my invoice | `void` looks up `(business, id)` under the signing business, so another business's invoice is `InvoiceNotFound` | `void_cannot_reach_other_business_invoice` |
| R8 | Fake token registered so the "paid" events are worthless | `register` checks `decimals()`, but a malicious token contract still passes. **The indexer must only trust `paid` events for allow-listed token addresses** | `register_rejects_unknown_token`, `register_rejects_non_token_contract` |
| R9 | i128 overflow on `paid` | `checked_add` → `ArithmeticOverflow` | `pay_rejects_overflowing_total` |
| R10 | PII on-chain | Only hashes and amounts are stored (`id`, `content_hash`) | Design / backend responsibility |
| R12 | Partially paid invoice stays payable forever | `close` makes it terminal; `pay` then fails `NotOpen` | `close_blocks_payment`, `no_payment_accepted_after_close` |
| R12 | Payee (e.g. a vault) controls or redirects an invoice it is paid into | Only `business` can `void`/`close`; lookups are keyed by `(business, id)` so the payee reaches nothing. `payee` is fixed at `register` | `payee_cannot_void_or_close`, `pay_routes_funds_to_payee_not_business` |
| R13 | Registry accumulates funds (`payee` = registry) | `register` rejects `payee == current_contract_address()` with `InvalidPayee`; `pay` always transfers payer → payee | `register_rejects_registry_as_payee`, property test `paid_never_exceeds_amount_and_registry_holds_nothing` |
| R11 | Reusing a voided id makes events ambiguous | Ids are never freed per business; `InvoiceExists` even after `void` | `void_does_not_free_the_id_for_reuse` |

## Residual risks and open items

1. **Token trust (R8, V10).** The contracts accept any token address. Downstream systems must allow-list token contract IDs, e.g. testnet/mainnet USDC SAC.
2. **Invoice-id squatting (downgraded).** Fixed for payment safety by the `(business, id)` key (R2). Residual: anyone can register invoices under their own address, which is only clutter. The indexer and clients must always carry the intended `business`; a client who pays a *different* business address than the one on their invoice pays that address. Backend salting remains good hygiene.
3. **Wrong payee (resolved: vault as payee).** Invoices carry a `payee`, so revenue can flow straight into an approval vault (`invoice_paid_into_approval_vault_then_paid_out_by_m_of_n`). Residual: the contract cannot check that `payee` is the business's *intended* vault; a compromised business key can register invoices paying anywhere. The backend should verify `payee` against `deployments/<network>.json` (or the business's known accounts) from the `registered` event before showing an invoice to a client.
4. **Approver key loss.** If enough approvers lose keys that `threshold` can't be reached, funds are stuck. This is operational: keep `threshold < approvers.len()` to leave headroom.
5. **No upgrade path.** This is intentional for v1 (README §5). Bugs require redeploying and migrating funds through a normal proposal.
6. **TTL archival.** Entries untouched for about 30 days can be archived. They are restorable with `stellar contract restore`; the backend should restore before acting on long-idle proposals or invoices.
