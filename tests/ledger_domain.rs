#[path = "../src/ledger.rs"]
mod ledger;

use ledger::{Account, AccountKind, Category, CategoryKind, LedgerError, Operation, balances};

fn accounts() -> Vec<Account> {
    vec![
        Account {
            id: "bank".into(),
            kind: AccountKind::Bank,
            initial_balance: 1_000,
        },
        Account {
            id: "card".into(),
            kind: AccountKind::CreditCard,
            initial_balance: 100,
        },
    ]
}

fn categories() -> Vec<Category> {
    vec![
        Category {
            id: "salary".into(),
            kind: CategoryKind::Income,
        },
        Category {
            id: "food".into(),
            kind: CategoryKind::Expense,
        },
    ]
}

#[test]
fn income_expense_and_card_payment_change_the_correct_balances() {
    let operations = vec![
        Operation::Income {
            account_id: "bank".into(),
            category_id: "salary".into(),
            amount: 500,
        },
        Operation::Expense {
            account_id: "bank".into(),
            category_id: "food".into(),
            amount: 100,
        },
        Operation::Expense {
            account_id: "card".into(),
            category_id: "food".into(),
            amount: 300,
        },
        Operation::CardPayment {
            source_account_id: "bank".into(),
            card_account_id: "card".into(),
            amount: 200,
        },
    ];
    let actual = balances(&accounts(), &categories(), &operations).unwrap();
    assert_eq!(actual["bank"], 1_200);
    assert_eq!(actual["card"], 200);
}

#[test]
fn payment_cannot_use_a_card_as_source() {
    let op = Operation::CardPayment {
        source_account_id: "card".into(),
        card_account_id: "card".into(),
        amount: 50,
    };
    assert_eq!(
        balances(&accounts(), &categories(), &[op]),
        Err(LedgerError::InvalidPaymentAccounts)
    );
}

#[test]
fn categories_must_match_movement_type() {
    let op = Operation::Expense {
        account_id: "bank".into(),
        category_id: "salary".into(),
        amount: 1,
    };
    assert_eq!(
        balances(&accounts(), &categories(), &[op]),
        Err(LedgerError::CategoryMismatch)
    );
}

#[test]
fn amounts_must_be_positive() {
    let op = Operation::Income {
        account_id: "bank".into(),
        category_id: "salary".into(),
        amount: 0,
    };
    assert_eq!(
        balances(&accounts(), &categories(), &[op]),
        Err(LedgerError::InvalidAmount)
    );
}

#[test]
fn rejects_arithmetic_overflow() {
    let account = Account {
        id: "bank".into(),
        kind: AccountKind::Bank,
        initial_balance: i64::MAX,
    };
    let op = Operation::Income {
        account_id: "bank".into(),
        category_id: "salary".into(),
        amount: 1,
    };
    assert_eq!(
        balances(&[account], &categories(), &[op]),
        Err(LedgerError::Overflow)
    );
}

#[test]
fn unknown_account_is_rejected() {
    let op = Operation::Expense {
        account_id: "missing".into(),
        category_id: "food".into(),
        amount: 1,
    };
    assert_eq!(
        balances(&accounts(), &categories(), &[op]),
        Err(LedgerError::AccountNotFound)
    );
}

#[test]
fn cash_and_wallet_accept_expenses() {
    let accounts = vec![
        Account {
            id: "cash".into(),
            kind: AccountKind::Cash,
            initial_balance: 50,
        },
        Account {
            id: "wallet".into(),
            kind: AccountKind::Wallet,
            initial_balance: 70,
        },
    ];
    let operations = vec![
        Operation::Expense {
            account_id: "cash".into(),
            category_id: "food".into(),
            amount: 10,
        },
        Operation::Expense {
            account_id: "wallet".into(),
            category_id: "food".into(),
            amount: 20,
        },
    ];
    let actual = balances(&accounts, &categories(), &operations).unwrap();
    assert_eq!(actual["cash"], 40);
    assert_eq!(actual["wallet"], 50);
}
