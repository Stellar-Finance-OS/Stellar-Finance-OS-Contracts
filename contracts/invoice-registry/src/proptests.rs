//! Property tests: invariants that must hold for any payment sequence.
#![allow(clippy::arithmetic_side_effects, clippy::unwrap_used, clippy::panic)]

extern crate std;

use proptest::prelude::*;
use soroban_sdk::{
    testutils::{Address as _, MockAuth, MockAuthInvoke},
    token::{StellarAssetClient, TokenClient},
    Address, BytesN, Env, IntoVal,
};

use crate::{types::InvoiceStatus, InvoiceRegistry, InvoiceRegistryClient};

fn mock(
    env: &Env,
    signer: &Address,
    contract: &Address,
    f: &str,
    args: soroban_sdk::Vec<soroban_sdk::Val>,
    subs: &[MockAuthInvoke],
) {
    env.mock_auths(&[MockAuth {
        address: signer,
        invoke: &MockAuthInvoke {
            contract,
            fn_name: f,
            args,
            sub_invokes: subs,
        },
    }]);
}

struct World<'a> {
    env: Env,
    registry_id: Address,
    registry: InvoiceRegistryClient<'a>,
    token: TokenClient<'a>,
    payer: Address,
    id: BytesN<32>,
}

/// Registry + SAC token + a payer holding 10 000 tokens.
fn world<'a>() -> World<'a> {
    let env = Env::default();
    let registry_id = env.register(InvoiceRegistry, ());
    let registry = InvoiceRegistryClient::new(&env, &registry_id);
    let admin = Address::generate(&env);
    let sac = env.register_stellar_asset_contract_v2(admin.clone());
    let token = TokenClient::new(&env, &sac.address());
    let payer = Address::generate(&env);
    let mint_args = (payer.clone(), 10_000i128).into_val(&env);
    mock(&env, &admin, &token.address, "mint", mint_args, &[]);
    StellarAssetClient::new(&env, &token.address).mint(&payer, &10_000);
    let id = BytesN::from_array(&env, &[1; 32]);
    World {
        env,
        registry_id,
        registry,
        token,
        payer,
        id,
    }
}

impl World<'_> {
    fn register(&self, business: &Address, amount: i128, payee: &Address) {
        let content = BytesN::from_array(&self.env, &[2; 32]);
        let args = (
            business.clone(),
            self.id.clone(),
            self.token.address.clone(),
            amount,
            content.clone(),
            payee.clone(),
        )
            .into_val(&self.env);
        mock(
            &self.env,
            business,
            &self.registry_id,
            "register",
            args,
            &[],
        );
        self.registry.register(
            business,
            &self.id,
            &self.token.address,
            &amount,
            &content,
            payee,
        );
    }

    /// Attempt a payment; returns whether it succeeded.
    fn pay(&self, business: &Address, payee: &Address, a: i128) -> bool {
        let args = (self.payer.clone(), business.clone(), self.id.clone(), a).into_val(&self.env);
        let sub = [MockAuthInvoke {
            contract: &self.token.address,
            fn_name: "transfer",
            args: (self.payer.clone(), payee.clone(), a).into_val(&self.env),
            sub_invokes: &[],
        }];
        mock(&self.env, &self.payer, &self.registry_id, "pay", args, &sub);
        matches!(
            self.registry.try_pay(&self.payer, business, &self.id, &a),
            Ok(Ok(()))
        )
    }
}

proptest! {
    #![proptest_config(ProptestConfig { cases: 48, ..ProptestConfig::default() })]

    /// For any invoice amount, payee choice and sequence of attempted
    /// payments: `paid` never exceeds `amount`, the payee receives exactly
    /// `paid`, a distinct business receives nothing, the registry's own
    /// balance is always zero, and the status is Paid iff `paid == amount`.
    #[test]
    fn paid_never_exceeds_amount_and_registry_holds_nothing(
        amount in 1i128..1_000,
        attempts in proptest::collection::vec(-5i128..400, 1..12),
        separate_payee in any::<bool>(),
    ) {
        let w = world();
        let business = Address::generate(&w.env);
        let payee = if separate_payee { Address::generate(&w.env) } else { business.clone() };
        w.register(&business, amount, &payee);

        for a in attempts {
            let _ = w.pay(&business, &payee, a);
            let inv = w.registry.get(&business, &w.id);
            prop_assert!(inv.paid >= 0 && inv.paid <= inv.amount);
            prop_assert_eq!(w.token.balance(&payee), inv.paid);
            if separate_payee {
                prop_assert_eq!(w.token.balance(&business), 0);
            }
            prop_assert_eq!(w.token.balance(&w.registry_id), 0);
            prop_assert_eq!(inv.status == InvoiceStatus::Paid, inv.paid == inv.amount);
        }
    }

    /// For any two businesses sharing one invoice id and any payment
    /// sequence aimed at `honest`: only `honest` is ever credited, and the
    /// other business's invoice and balance never change.
    #[test]
    fn payments_only_credit_the_named_business(
        amount_honest in 1i128..1_000,
        amount_squat in 1i128..1_000,
        attempts in proptest::collection::vec(-5i128..400, 1..12),
        squat_first in any::<bool>(),
    ) {
        let w = world();
        let honest = Address::generate(&w.env);
        let squatter = Address::generate(&w.env);
        if squat_first {
            w.register(&squatter, amount_squat, &squatter);
            w.register(&honest, amount_honest, &honest);
        } else {
            w.register(&honest, amount_honest, &honest);
            w.register(&squatter, amount_squat, &squatter);
        }

        for a in attempts {
            let _ = w.pay(&honest, &honest, a);
            let h = w.registry.get(&honest, &w.id);
            let sq = w.registry.get(&squatter, &w.id);
            prop_assert_eq!(w.token.balance(&honest), h.paid);
            prop_assert_eq!(w.token.balance(&squatter), 0);
            prop_assert_eq!(sq.paid, 0);
            prop_assert_eq!(sq.status, InvoiceStatus::Open);
        }
    }

    /// After `close`, no payment is accepted and `paid` is frozen.
    #[test]
    fn no_payment_accepted_after_close(
        amount in 2i128..1_000,
        first in 1i128..1_000,
        later in proptest::collection::vec(-5i128..400, 1..8),
    ) {
        let first = first % (amount - 1) + 1; // 1..amount-1: partial
        let w = world();
        let business = Address::generate(&w.env);
        w.register(&business, amount, &business);
        prop_assert!(w.pay(&business, &business, first));

        let close_args = (business.clone(), w.id.clone()).into_val(&w.env);
        mock(&w.env, &business, &w.registry_id, "close", close_args, &[]);
        w.registry.close(&business, &w.id);

        for a in later {
            prop_assert!(!w.pay(&business, &business, a));
            let inv = w.registry.get(&business, &w.id);
            prop_assert_eq!(inv.status, InvoiceStatus::Closed);
            prop_assert_eq!(inv.paid, first);
            prop_assert_eq!(w.token.balance(&business), first);
        }
    }
}
