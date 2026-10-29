//! Hunter currency foundations.

use serde::{Deserialize, Serialize};

/// Starting balance granted to every new hunter.
pub const STARTING_WON: u64 = 0;

/// A hunter's balance in South Korean won.
///
/// This is kept as a typed value instead of a bare integer so future reward,
/// shop, and trading systems cannot accidentally mix it with experience or
/// item counts.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(transparent)]
pub struct Won(u64);

impl Won {
    /// Creates an empty balance, used by shared treasuries.
    #[must_use]
    pub const fn zero() -> Self {
        Self(0)
    }

    /// Creates a balance with the configured starting amount.
    #[must_use]
    pub const fn starting_balance() -> Self {
        Self(STARTING_WON)
    }

    /// Returns the numeric amount of won.
    #[must_use]
    pub const fn amount(self) -> u64 {
        self.0
    }

    /// Adds won, saturating at the largest representable balance.
    pub const fn credit(&mut self, amount: u64) {
        self.0 = self.0.saturating_add(amount);
    }

    /// Adds a whole payment/reward, rejecting overflow without changing funds.
    pub const fn try_credit(&mut self, amount: u64) -> bool {
        let Some(balance) = self.0.checked_add(amount) else {
            return false;
        };
        self.0 = balance;
        true
    }

    /// Removes `amount` when sufficient funds are available.
    pub const fn debit(&mut self, amount: u64) -> bool {
        if self.0 < amount {
            return false;
        }
        self.0 -= amount;
        true
    }
}

impl Default for Won {
    fn default() -> Self {
        Self::starting_balance()
    }
}

/// Personal System currency, distinct from Won, XP and stat points.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(transparent)]
pub struct SystemPoints(u64);

impl SystemPoints {
    /// Creates an empty personal balance.
    #[must_use]
    pub const fn zero() -> Self {
        Self(0)
    }

    /// Returns the spendable balance.
    #[must_use]
    pub const fn amount(self) -> u64 {
        self.0
    }

    /// Credits the whole reward, rejecting overflow without changing the balance.
    pub const fn credit(&mut self, amount: u64) -> bool {
        let Some(balance) = self.0.checked_add(amount) else {
            return false;
        };
        self.0 = balance;
        true
    }

    /// Spends the whole price, or leaves the balance unchanged.
    pub const fn debit(&mut self, amount: u64) -> bool {
        let Some(balance) = self.0.checked_sub(amount) else {
            return false;
        };
        self.0 = balance;
        true
    }
}

/// Payment failures, including an explicitly indeterminate commit result.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PaymentError {
    /// Both participants must have live hunter sessions.
    Unavailable,
    /// Payments cannot target the sender.
    SelfPayment,
    /// Amounts must be positive.
    ZeroAmount,
    /// The sender cannot afford this payment.
    InsufficientFunds,
    /// The recipient cannot hold the entire payment.
    Overflow,
    /// Persistence failed before the payment committed.
    Storage,
    /// The redo record was installed but its durability could not be confirmed.
    CommitUncertain,
}

impl Won {
    /// Calculates both resulting balances without mutating either account.
    pub const fn payment(self, recipient: Self, amount: u64) -> Result<(Self, Self), PaymentError> {
        if amount == 0 {
            return Err(PaymentError::ZeroAmount);
        }
        let Some(sender) = self.0.checked_sub(amount) else {
            return Err(PaymentError::InsufficientFunds);
        };
        let Some(recipient) = recipient.0.checked_add(amount) else {
            return Err(PaymentError::Overflow);
        };
        Ok((Self(sender), Self(recipient)))
    }
}

#[cfg(test)]
mod tests {
    use super::{PaymentError, SystemPoints, Won};

    #[test]
    fn payment_preserves_total_and_rejects_partial_transfers() {
        assert_eq!(Won(100).payment(Won(20), 30), Ok((Won(70), Won(50))));
        assert_eq!(
            Won(100).payment(Won(20), 101),
            Err(PaymentError::InsufficientFunds)
        );
        assert_eq!(
            Won(100).payment(Won(u64::MAX), 1),
            Err(PaymentError::Overflow)
        );
        assert_eq!(Won(100).payment(Won(20), 0), Err(PaymentError::ZeroAmount));
    }

    #[test]
    fn system_currency_never_wraps_or_spends_partial_prices() {
        let mut points = SystemPoints::default();
        assert!(!points.debit(1));
        assert!(points.credit(u64::MAX));
        assert!(!points.credit(1));
        assert_eq!(points.amount(), u64::MAX);
        assert!(points.debit(u64::MAX));
        assert_eq!(points.amount(), 0);
    }
}
