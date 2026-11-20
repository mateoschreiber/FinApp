ALTER TABLE finance_operations RENAME TO finance_operations_legacy;

CREATE TABLE finance_operations (
    user_id TEXT NOT NULL, id TEXT NOT NULL, version INTEGER NOT NULL CHECK(version > 0),
    deleted INTEGER NOT NULL CHECK(deleted IN (0,1)), kind TEXT NOT NULL,
    account_id TEXT NOT NULL, category_id TEXT, card_account_id TEXT,
    destination_account_id TEXT, original_operation_id TEXT,
    amount INTEGER NOT NULL CHECK(amount BETWEEN 1 AND 9007199254740991),
    date TEXT NOT NULL, description TEXT, notes TEXT,
    data TEXT GENERATED ALWAYS AS (json_object(
        'id', id, 'version', version, 'deleted', json(CASE deleted WHEN 1 THEN 'true' ELSE 'false' END),
        'kind', kind, 'account_id', account_id, 'category_id', category_id, 'card_account_id', card_account_id,
        'destination_account_id', destination_account_id, 'original_operation_id', original_operation_id,
        'amount', amount, 'date', date, 'description', description, 'notes', notes
    )) VIRTUAL,
    PRIMARY KEY(user_id,id),
    FOREIGN KEY(user_id,account_id) REFERENCES finance_accounts(user_id,id),
    FOREIGN KEY(user_id,category_id) REFERENCES finance_categories(user_id,id),
    FOREIGN KEY(user_id,card_account_id) REFERENCES finance_accounts(user_id,id),
    FOREIGN KEY(user_id,destination_account_id) REFERENCES finance_accounts(user_id,id),
    FOREIGN KEY(user_id,original_operation_id) REFERENCES finance_operations(user_id,id)
);
INSERT INTO finance_operations (user_id,id,version,deleted,kind,account_id,category_id,card_account_id,amount,date,description,notes)
SELECT user_id,id,version,json_extract(data,'$.deleted'),json_extract(data,'$.kind'),json_extract(data,'$.account_id'),
    json_extract(data,'$.category_id'),json_extract(data,'$.card_account_id'),json_extract(data,'$.amount'),
    json_extract(data,'$.date'),json_extract(data,'$.description'),json_extract(data,'$.notes')
FROM finance_operations_legacy;
DROP TABLE finance_operations_legacy;
CREATE INDEX finance_by_date ON finance_operations(user_id,date);
CREATE INDEX finance_by_original ON finance_operations(user_id,original_operation_id) WHERE deleted=0;
CREATE TABLE finance_changes (
    seq INTEGER PRIMARY KEY AUTOINCREMENT, user_id TEXT NOT NULL, entity TEXT NOT NULL,
    id TEXT NOT NULL, UNIQUE(user_id,entity,id)
);
CREATE INDEX finance_changes_user ON finance_changes(user_id,entity,seq);
CREATE INDEX finance_by_account ON finance_operations(user_id,account_id) WHERE deleted=0;
CREATE INDEX finance_by_destination ON finance_operations(user_id,destination_account_id) WHERE deleted=0;
CREATE INDEX finance_by_card ON finance_operations(user_id,card_account_id) WHERE deleted=0;
CREATE VIEW finance_postings AS
    SELECT o.user_id,o.id,o.date,o.account_id,
        CASE WHEN o.kind IN ('income','refund','loan_draw') THEN o.amount
             ELSE -o.amount END * CASE WHEN a.kind='card' THEN -1 ELSE 1 END AS delta
    FROM finance_operations o JOIN finance_accounts a ON a.user_id=o.user_id AND a.id=o.account_id
    WHERE o.deleted=0
    UNION ALL
    SELECT user_id,id,date,destination_account_id,amount FROM finance_operations WHERE deleted=0 AND kind='transfer'
    UNION ALL
    SELECT user_id,id,date,card_account_id,-amount FROM finance_operations WHERE deleted=0 AND kind='payment';

