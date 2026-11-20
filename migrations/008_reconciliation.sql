CREATE TABLE reconciliation_lines (
    user_id TEXT NOT NULL, account_id TEXT NOT NULL, line_id TEXT NOT NULL,
    date TEXT NOT NULL, amount INTEGER NOT NULL CHECK(amount<>0), description TEXT NOT NULL,
    reference TEXT, PRIMARY KEY(user_id,account_id,line_id),
    FOREIGN KEY(user_id,account_id) REFERENCES finance_accounts(user_id,id)
);
CREATE INDEX reconciliation_date ON reconciliation_lines(user_id,account_id,date,line_id);
CREATE TABLE reconciliations (
    user_id TEXT NOT NULL, account_id TEXT NOT NULL, line_id TEXT NOT NULL,
    operation_id TEXT NOT NULL, version INTEGER NOT NULL,
    PRIMARY KEY(user_id,account_id,line_id), UNIQUE(user_id,account_id,operation_id),
    FOREIGN KEY(user_id,account_id,line_id) REFERENCES reconciliation_lines(user_id,account_id,line_id),
    FOREIGN KEY(user_id,operation_id) REFERENCES finance_operations(user_id,id)
);
