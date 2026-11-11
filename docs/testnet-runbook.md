# Testnet runbook

A start-to-finish walkthrough: deploy both contracts to testnet, then exercise
every main flow by hand. It takes about 15 minutes with the Stellar CLI installed.

> Testnet resets periodically. After a reset, redeploy and merge the new
> `deployments/testnet.json`.

## 0. Identities

Create and fund a few testnet accounts. These live in the CLI's local config
and must **never** be committed.

```bash
for who in deployer alice bob carol acme client; do
  stellar keys generate $who --network testnet --fund
done
```

## 1. Deploy

Either run **Actions → Deploy (testnet)** with approvers
`["$(stellar keys address alice)","$(stellar keys address bob)","$(stellar keys address carol)"]`
and threshold `2`, then merge the PR it opens. Or deploy locally:

```bash
STELLAR_ACCOUNT=deployer \
VAULT_APPROVERS="[\"$(stellar keys address alice)\",\"$(stellar keys address bob)\",\"$(stellar keys address carol)\"]" \
VAULT_THRESHOLD=2 \
./scripts/deploy.sh testnet
```

Then confirm the policy:

```bash
stellar contract invoke --id approval-vault --source-account alice --network testnet --send=no -- get_policy
```

## 2. A test token

Use the native XLM asset contract, so no issuer setup is needed:

```bash
TOKEN=$(stellar contract id asset --asset native --network testnet)
```

## 3. Vault: deposit → propose → approve → execute

```bash
VAULT=approval-vault
INV() { stellar contract invoke --id "$1" --source-account "$2" --network testnet -- "${@:3}"; }

# acme deposits 100 XLM (7 decimals)
INV $VAULT acme deposit --from acme --token $TOKEN --amount 1000000000
INV $VAULT acme balance --token $TOKEN

# alice proposes paying client 10 XLM, valid for one hour
MEMO=$(printf 'payment-request-1' | sha256sum | cut -d' ' -f1)
INV $VAULT alice propose --proposer alice --token $TOKEN --to client \
  --amount 100000000 --memo_ref $MEMO --expires_at $(( $(date +%s) + 3600 ))
# → prints the proposal id, e.g. 0

INV $VAULT alice execute --id 0      # expected: Error(Contract, #10) ThresholdNotMet
INV $VAULT bob approve --approver bob --id 0
INV $VAULT carol execute --id 0      # succeeds, client receives 10 XLM
INV $VAULT carol get_proposal --id 0 # status: Executed
```

Failure checks worth doing by hand:

| Try | Expected |
|---|---|
| `client` calls `propose` | `#3 NotApprover` |
| `bob` approves proposal 0 twice | `#7 AlreadyApproved` |
| propose with `--expires_at` in the past | `#9 InvalidExpiry` |
| propose more than the vault holds, approve, execute | `#13 InsufficientBalance` |
| propose with `--token` set to an account address | `#14 InvalidToken` |

## 4. Vault: change the approver set

```bash
INV $VAULT alice propose_policy_update --proposer alice \
  --new_approvers "[\"$(stellar keys address alice)\",\"$(stellar keys address bob)\"]" \
  --new_threshold 2 --expires_at $(( $(date +%s) + 3600 ))
INV $VAULT bob approve_policy_update --approver bob --id 1
INV $VAULT bob update_policy --id 1
INV $VAULT bob get_policy            # carol is gone; threshold 2
```

## 5. Invoices: register → partial pay → pay → (void)

```bash
REG=invoice-registry
ID=$(printf 'invoice-0001:%s' "$INVOICE_ID_SALT" | sha256sum | cut -d' ' -f1)
DOC=$(printf 'invoice-0001-pdf-bytes' | sha256sum | cut -d' ' -f1)

INV $REG acme register --business acme --id $ID --token $TOKEN --amount 50000000 --content_hash $DOC --payee acme
INV $REG client pay --payer client --business acme --id $ID --amount 20000000
INV $REG client get --business acme --id $ID         # paid 20000000, status Open
INV $REG client pay --payer client --business acme --id $ID --amount 40000000   # #5 Overpayment
INV $REG client pay --payer client --business acme --id $ID --amount 30000000   # status Paid
INV $REG acme void --business acme --id $ID                      # #4 NotOpen
```

## 6. Check events

```bash
stellar events --network testnet --start-ledger <ledger from step 1> \
  --id $(jq -r .invoiceRegistry deployments/testnet.json)
```

You should see `registered`, two `paid` events (`total_paid` 20000000, then
50000000), with topics matching [`docs/events.md`](events.md).

## 7. Clean up

Nothing to clean up on-chain. Testnet resets on its own. Keep the
`stellar keys` identities out of git; `.gitignore` already excludes `.stellar/`.
