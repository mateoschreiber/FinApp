CREATE TABLE catalog_details (
    user_id TEXT NOT NULL,
    entity TEXT NOT NULL CHECK(entity IN ('account', 'category')),
    id TEXT NOT NULL,
    archived INTEGER NOT NULL CHECK(archived IN (0, 1)),
    institution TEXT NOT NULL,
    notes TEXT NOT NULL,
    cutoff_date TEXT,
    closing_day INTEGER CHECK(closing_day BETWEEN 1 AND 31),
    due_day INTEGER CHECK(due_day BETWEEN 1 AND 31),
    PRIMARY KEY(user_id, entity, id)
);

CREATE VIEW finance_balance_postings AS
SELECT p.*
FROM finance_postings p
LEFT JOIN catalog_details d ON d.user_id=p.user_id AND d.entity='account' AND d.id=p.account_id
WHERE d.cutoff_date IS NULL OR p.date >= d.cutoff_date;

CREATE TABLE import_sources (
    user_id TEXT NOT NULL,
    entity TEXT NOT NULL,
    source_id TEXT NOT NULL,
    target_id TEXT NOT NULL,
    fingerprint TEXT NOT NULL,
    PRIMARY KEY(user_id, entity, source_id)
);
