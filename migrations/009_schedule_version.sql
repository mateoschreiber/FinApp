-- Versionado del algoritmo de amortizacion. Los prestamos existentes conservan V1;
-- los nuevos se crean explicitamente en V2 (interes nominal con aritmetica entera).
ALTER TABLE credit_loans
    ADD COLUMN schedule_version INTEGER NOT NULL DEFAULT 1
    CHECK (schedule_version IN (1, 2));
