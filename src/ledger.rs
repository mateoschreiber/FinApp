use std::collections::BTreeMap;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum AccountKind {
    Cash,
    Bank,
    Wallet,
    CreditCard,
}
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum CategoryKind {
    Income,
    Expense,
}
pub struct Account {
    pub id: String,
    pub kind: AccountKind,
    pub initial_balance: i64,
}
pub struct Category {
    pub id: String,
    pub kind: CategoryKind,
}
pub enum Operation {
    Income {
        account_id: String,
        category_id: String,
        amount: i64,
    },
    Expense {
        account_id: String,
        category_id: String,
        amount: i64,
    },
    CardPayment {
        source_account_id: String,
        card_account_id: String,
        amount: i64,
    },
}
#[derive(Debug, Eq, PartialEq)]
pub enum LedgerError {
    InvalidAmount,
    InvalidPaymentAccounts,
    CategoryMismatch,
    AccountNotFound,
    CategoryNotFound,
    DuplicateAccount,
    DuplicateCategory,
    Overflow,
}

/// Recompute balances from an opening snapshot and the current set of operations.
/// A card balance is an amount owed; an ordinary account balance is money held.
pub fn balances(
    accounts: &[Account],
    categories: &[Category],
    operations: &[Operation],
) -> Result<BTreeMap<String, i64>, LedgerError> {
    let mut result = BTreeMap::new();
    let mut kinds = BTreeMap::new();
    for account in accounts {
        if kinds.insert(account.id.as_str(), account.kind).is_some() {
            return Err(LedgerError::DuplicateAccount);
        }
        result.insert(account.id.clone(), account.initial_balance);
    }
    let mut category_kinds = BTreeMap::new();
    for category in categories {
        if category_kinds
            .insert(category.id.as_str(), category.kind)
            .is_some()
        {
            return Err(LedgerError::DuplicateCategory);
        }
    }
    for operation in operations {
        match operation {
            Operation::Income {
                account_id,
                category_id,
                amount,
            }
            | Operation::Expense {
                account_id,
                category_id,
                amount,
            } => {
                if *amount <= 0 {
                    return Err(LedgerError::InvalidAmount);
                }
                let expected = if matches!(operation, Operation::Income { .. }) {
                    CategoryKind::Income
                } else {
                    CategoryKind::Expense
                };
                let actual = category_kinds
                    .get(category_id.as_str())
                    .ok_or(LedgerError::CategoryNotFound)?;
                if *actual != expected {
                    return Err(LedgerError::CategoryMismatch);
                }
                let kind = kinds
                    .get(account_id.as_str())
                    .ok_or(LedgerError::AccountNotFound)?;
                let increase =
                    (expected == CategoryKind::Income) != (*kind == AccountKind::CreditCard);
                let value = result
                    .get_mut(account_id)
                    .ok_or(LedgerError::AccountNotFound)?;
                *value = if increase {
                    value.checked_add(*amount)
                } else {
                    value.checked_sub(*amount)
                }
                .ok_or(LedgerError::Overflow)?;
            }
            Operation::CardPayment {
                source_account_id,
                card_account_id,
                amount,
            } => {
                if *amount <= 0 {
                    return Err(LedgerError::InvalidAmount);
                }
                let source_kind = kinds
                    .get(source_account_id.as_str())
                    .ok_or(LedgerError::AccountNotFound)?;
                let card_kind = kinds
                    .get(card_account_id.as_str())
                    .ok_or(LedgerError::AccountNotFound)?;
                if *source_kind == AccountKind::CreditCard
                    || *card_kind != AccountKind::CreditCard
                    || source_account_id == card_account_id
                {
                    return Err(LedgerError::InvalidPaymentAccounts);
                }
                let source = result
                    .get(source_account_id)
                    .ok_or(LedgerError::AccountNotFound)?
                    .checked_sub(*amount)
                    .ok_or(LedgerError::Overflow)?;
                let card = result
                    .get(card_account_id)
                    .ok_or(LedgerError::AccountNotFound)?
                    .checked_sub(*amount)
                    .ok_or(LedgerError::Overflow)?;
                result.insert(source_account_id.clone(), source);
                result.insert(card_account_id.clone(), card);
            }
        }
    }
    Ok(result)
}
