CREATE TABLE IF NOT EXISTS finance_accounts (
    user_id TEXT NOT NULL, id TEXT NOT NULL, name TEXT NOT NULL,
    kind TEXT NOT NULL, initial_balance INTEGER NOT NULL, PRIMARY KEY(user_id,id)
);
CREATE TABLE IF NOT EXISTS finance_categories (
    user_id TEXT NOT NULL, id TEXT NOT NULL, name TEXT NOT NULL,
    kind TEXT NOT NULL, PRIMARY KEY(user_id,id)
);
CREATE TABLE IF NOT EXISTS finance_operations (
    user_id TEXT NOT NULL, id TEXT NOT NULL, version INTEGER NOT NULL,
    data TEXT NOT NULL, PRIMARY KEY(user_id,id)
);
CREATE TABLE IF NOT EXISTS finance_requests (
    user_id TEXT NOT NULL, id TEXT NOT NULL, request TEXT NOT NULL,
    response TEXT NOT NULL, PRIMARY KEY(user_id,id)
);
