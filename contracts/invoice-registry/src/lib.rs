//! Invoice registry: anchors invoice hashes on-chain and lets clients pay
//! through the contract so payment → invoice matching is exact.
//!
//! Spec: README §4. Build order: task 5 in AGENTS.md.
#![no_std]

pub mod events;
mod storage;
pub mod types;

#[cfg(test)]
mod proptests;
#[cfg(test)]
mod test;

use finance_os_common::amount;
use soroban_sdk::{contract, contractimpl, token, Address, BytesN, Env};

use crate::events::{Closed, Paid, Registered, Voided};
use crate::types::{Error, Invoice, InvoiceStatus};

#[contract]
pub struct InvoiceRegistry;

#[contractimpl]
impl InvoiceRegistry {
    /// Register an Open invoice controlled by `business` and payable to
    /// `payee` in `token`. `payee` may equal `business` (the common case) or
    /// be any other address or contract, e.g. an approval vault. It may not be
    /// the registry itself: the registry never holds funds.
    ///
    /// `id` and `content_hash` are backend-generated hashes. `token` must be a
    /// contract implementing the standard token interface. This is checked
    /// here so a bad token fails at registration, not when a client pays.
    pub fn register(
        env: Env,
        business: Address,
        id: BytesN<32>,
        token: Address,
        amount: i128,
        content_hash: BytesN<32>,
        payee: Address,
    ) -> Result<(), Error> {
        business.require_auth();
        storage::bump_instance(&env);
        if !amount::is_positive(amount) {
            return Err(Error::InvalidAmount);
        }
        if payee == env.current_contract_address() {
            return Err(Error::InvalidPayee);
        }
        if storage::has_invoice(&env, &business, &id) {
            return Err(Error::InvoiceExists);
        }
        require_token(&env, &token)?;

        storage::set_invoice(
            &env,
            &Invoice {
                id: id.clone(),
                business: business.clone(),
                token: token.clone(),
                amount,
                paid: 0,
                status: InvoiceStatus::Open,
                content_hash: content_hash.clone(),
                content_hash,
                payee: payee.clone(),
            },
        );
        Registered {
            id,
            business,
            token,
            amount,
            content_hash,
            payee,
        }
        .publish(&env);
        Ok(())
    }

    /// Pay `amount` towards `business`'s Open invoice `id`. Tokens move
    /// straight from `payer` to the invoice's `payee`. The invoice is looked up by
    /// `(business, id)`, so an invoice another address registered under the
    /// same `id` can never receive this payment. Partial payments are
    /// allowed. The invoice becomes Paid once `paid == amount`.
    ///
    /// Overpayment is rejected (README §5 lists it as a failure path), so
    /// `paid` can never exceed `amount`.
    pub fn pay(
        env: Env,
        payer: Address,
        business: Address,
        id: BytesN<32>,
        amount: i128,
    ) -> Result<(), Error> {
        payer.require_auth();
        storage::bump_instance(&env);
        if !amount::is_positive(amount) {
            return Err(Error::InvalidAmount);
        }
        let mut invoice =
            storage::get_invoice(&env, &business, &id).ok_or(Error::InvoiceNotFound)?;
        if invoice.status != InvoiceStatus::Open {
            return Err(Error::NotOpen);
        }
        let total_paid = invoice
            .paid
            .checked_add(amount)
            .ok_or(Error::ArithmeticOverflow)?;
        if total_paid > invoice.amount {
            return Err(Error::Overpayment);
        }

        // Update state before calling the token contract.
        invoice.paid = total_paid;
        if total_paid == invoice.amount {
            invoice.status = InvoiceStatus::Paid;
        }
        storage::set_invoice(&env, &invoice);

        token::TokenClient::new(&env, &invoice.token).transfer(&payer, &invoice.payee, &amount);

        Paid {
            id,
            business,
            payer,
            amount,
            total_paid,
        }
        .publish(&env);
        Ok(())
    }

    /// Void an Open invoice that has received no payments. `business` can
    /// only reach its own invoices: the lookup is by `(business, id)`. The
    /// payee has no control over the invoice.
    pub fn void(env: Env, business: Address, id: BytesN<32>) -> Result<(), Error> {
        business.require_auth();
        storage::bump_instance(&env);
        let mut invoice =
            storage::get_invoice(&env, &business, &id).ok_or(Error::InvoiceNotFound)?;
        if invoice.status != InvoiceStatus::Open {
            return Err(Error::NotOpen);
        }
        if invoice.paid != 0 {
            return Err(Error::AlreadyPaid);
        }
        invoice.status = InvoiceStatus::Void;
        storage::set_invoice(&env, &invoice);

        Voided { id, business }.publish(&env);
        Ok(())
    }

    /// Close a partially paid Open invoice (discount, write-off, or remainder
    /// settled off-chain). `business` can only reach its own invoices: the
    /// lookup is by `(business, id)`. Zero-paid invoices must use `void`.
    /// Close a partially paid Open invoice (discount, dispute, write-off or
    /// remainder settled off-chain). Only the invoice's business may close it.
    /// Invoices with nothing paid must use `void` instead.
    pub fn close(env: Env, business: Address, id: BytesN<32>) -> Result<(), Error> {
        business.require_auth();
        storage::bump_instance(&env);
        let mut invoice =
            storage::get_invoice(&env, &business, &id).ok_or(Error::InvoiceNotFound)?;
        if invoice.status != InvoiceStatus::Open {
            return Err(Error::NotOpen);
        }
        if invoice.paid == 0 {
            return Err(Error::NothingPaid);
        }
        let outstanding = invoice
            .amount
            .checked_sub(invoice.paid)
            .ok_or(Error::ArithmeticOverflow)?;
        invoice.status = InvoiceStatus::Closed;
        storage::set_invoice(&env, &invoice);

        Closed {
            id,
            business,
            paid: invoice.paid,
            outstanding,
        }
        .publish(&env);
        Ok(())
    }

    pub fn get(env: Env, business: Address, id: BytesN<32>) -> Result<Invoice, Error> {
        storage::bump_instance(&env);
        storage::get_invoice(&env, &business, &id).ok_or(Error::InvoiceNotFound)
    }
}

/// `token` must answer the standard token interface's `decimals()`.
fn require_token(env: &Env, token: &Address) -> Result<(), Error> {
    match token::TokenClient::new(env, token).try_decimals() {
        Ok(Ok(_)) => Ok(()),
        _ => Err(Error::InvalidToken),
    }
}
