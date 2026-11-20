CREATE TABLE credit_purchases (
    user_id TEXT NOT NULL, operation_id TEXT NOT NULL, first_due TEXT NOT NULL,
    periods INTEGER NOT NULL CHECK(periods BETWEEN 1 AND 600),
    PRIMARY KEY (user_id, operation_id),
    FOREIGN KEY (user_id,operation_id) REFERENCES finance_operations(user_id,id)
);
CREATE TABLE credit_loans (
    user_id TEXT NOT NULL, id TEXT NOT NULL, name TEXT NOT NULL, account_id TEXT NOT NULL,
    principal INTEGER NOT NULL CHECK (principal>0), draw_date TEXT NOT NULL,
    first_due TEXT NOT NULL, periods INTEGER NOT NULL CHECK(periods BETWEEN 1 AND 600),
    method TEXT NOT NULL CHECK(method IN ('french','german')),
    rate_bps INTEGER NOT NULL CHECK(rate_bps BETWEEN 0 AND 100000),
    rate_type TEXT NOT NULL CHECK(rate_type IN ('nominal','effective')),
    PRIMARY KEY (user_id,id),
    FOREIGN KEY (user_id,account_id) REFERENCES finance_accounts(user_id,id)
);
CREATE TABLE credit_repayments (
    user_id TEXT NOT NULL, id TEXT NOT NULL, loan_id TEXT NOT NULL, date TEXT NOT NULL,
    principal INTEGER NOT NULL CHECK(principal>=0), interest INTEGER NOT NULL CHECK(interest>=0),
    fee INTEGER NOT NULL CHECK(fee>=0), PRIMARY KEY(user_id,id),
    FOREIGN KEY(user_id,loan_id) REFERENCES credit_loans(user_id,id)
);
CREATE INDEX credit_repayments_by_loan ON credit_repayments(user_id,loan_id,date);
CREATE TABLE credit_allocations (
    user_id TEXT NOT NULL, operation_id TEXT NOT NULL, month TEXT NOT NULL,
    amount INTEGER NOT NULL CHECK(amount>0),
    payment_version INTEGER NOT NULL,
    PRIMARY KEY(user_id,operation_id,month),
    FOREIGN KEY(user_id,operation_id) REFERENCES finance_operations(user_id,id)
);
CREATE TABLE credit_requests (
    user_id TEXT NOT NULL, id TEXT NOT NULL, payload TEXT NOT NULL, PRIMARY KEY(user_id,id)
);
CREATE TRIGGER credit_allocation_added AFTER INSERT ON credit_allocations BEGIN
    DELETE FROM planning_reviews WHERE user_id=NEW.user_id AND month=NEW.month;
END;
CREATE TRIGGER credit_allocation_removed AFTER DELETE ON credit_allocations BEGIN
    DELETE FROM planning_reviews WHERE user_id=OLD.user_id AND month=OLD.month;
END;
CREATE TRIGGER credit_payment_amended AFTER UPDATE OF version ON finance_operations
WHEN OLD.kind='payment' AND OLD.version!=NEW.version BEGIN
    DELETE FROM credit_allocations WHERE user_id=OLD.user_id AND operation_id=OLD.id;
END;
