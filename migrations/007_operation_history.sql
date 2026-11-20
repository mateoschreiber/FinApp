CREATE TABLE finance_operation_history (
    seq INTEGER PRIMARY KEY,
    user_id TEXT NOT NULL,
    operation_id TEXT NOT NULL,
    changed_at INTEGER NOT NULL DEFAULT (unixepoch()),
    previous TEXT,
    current TEXT NOT NULL
);
CREATE INDEX finance_operation_history_user ON finance_operation_history(user_id, seq DESC);

CREATE TRIGGER finance_operation_history_insert AFTER INSERT ON finance_operations BEGIN
    INSERT INTO finance_operation_history(user_id,operation_id,current)
    VALUES(NEW.user_id,NEW.id,NEW.data);
END;
CREATE TRIGGER finance_operation_history_update AFTER UPDATE ON finance_operations BEGIN
    INSERT INTO finance_operation_history(user_id,operation_id,previous,current)
    VALUES(NEW.user_id,NEW.id,OLD.data,NEW.data);
END;
