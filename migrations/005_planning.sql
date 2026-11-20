CREATE TABLE planning_budgets (
    user_id TEXT NOT NULL, month TEXT NOT NULL, category_id TEXT NOT NULL, amount INTEGER NOT NULL CHECK(amount > 0),
    PRIMARY KEY(user_id,month,category_id),
    FOREIGN KEY(user_id,category_id) REFERENCES finance_categories(user_id,id)
);
CREATE TABLE planning_goals (
    user_id TEXT NOT NULL, id TEXT NOT NULL, name TEXT NOT NULL, account_id TEXT NOT NULL,
    target INTEGER NOT NULL CHECK(target > 0), deadline TEXT NOT NULL,
    PRIMARY KEY(user_id,id), FOREIGN KEY(user_id,account_id) REFERENCES finance_accounts(user_id,id)
);
CREATE TABLE planning_recurring (
    user_id TEXT NOT NULL, id TEXT NOT NULL, name TEXT NOT NULL, account_id TEXT NOT NULL, category_id TEXT NOT NULL,
    amount INTEGER NOT NULL CHECK(amount > 0), first_month TEXT NOT NULL, last_month TEXT, day INTEGER NOT NULL CHECK(day BETWEEN 1 AND 31),
    PRIMARY KEY(user_id,id), FOREIGN KEY(user_id,account_id) REFERENCES finance_accounts(user_id,id),
    FOREIGN KEY(user_id,category_id) REFERENCES finance_categories(user_id,id)
);
CREATE TABLE planning_reviews (
    user_id TEXT NOT NULL REFERENCES users(id), month TEXT NOT NULL, digest TEXT NOT NULL,
    PRIMARY KEY(user_id,month)
);
CREATE TABLE planning_receipts (
    user_id TEXT NOT NULL, id TEXT NOT NULL, operation_id TEXT NOT NULL, name TEXT NOT NULL, mime TEXT NOT NULL,
    digest TEXT NOT NULL, content BLOB NOT NULL,
    PRIMARY KEY(user_id,id), UNIQUE(user_id,operation_id,digest),
    FOREIGN KEY(user_id,operation_id) REFERENCES finance_operations(user_id,id)
);
CREATE INDEX planning_receipt_operation ON planning_receipts(user_id,operation_id);
CREATE TRIGGER planning_review_after_insert AFTER INSERT ON finance_operations BEGIN
    DELETE FROM planning_reviews WHERE user_id=NEW.user_id AND month=substr(json_extract(NEW.data,'$.date'),1,7);
END;
CREATE TRIGGER planning_review_after_update AFTER UPDATE ON finance_operations BEGIN
    DELETE FROM planning_reviews WHERE user_id=NEW.user_id AND month IN
        (substr(json_extract(OLD.data,'$.date'),1,7),substr(json_extract(NEW.data,'$.date'),1,7));
END;
CREATE TRIGGER planning_review_after_delete AFTER DELETE ON finance_operations BEGIN
    DELETE FROM planning_reviews WHERE user_id=OLD.user_id AND month=substr(json_extract(OLD.data,'$.date'),1,7);
END;
