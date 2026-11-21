import { test } from 'node:test';
import assert from 'node:assert/strict';
import { readFileSync } from 'node:fs';
import { creditProjection, cardStatement } from '../static/credit.mjs';

const fixture = JSON.parse(readFileSync(new URL('./fixtures/f92d.json', import.meta.url), 'utf8'));
const emptyView = { accounts: [], categories: [], operations: [] };

function loanPortfolio(principal, installments, payments, outstanding, id = 'loan-1') {
  return {
    loans: [{
      id, name: 'Préstamo', principal, draw_date: installments[0].date,
      installments: installments.map(i => ({ interest: 0, ...i })),
      payments: payments.map((p, n) => ({ id: `pay-${n}`, fee: 0, interest: 0, ...p })),
      outstanding,
    }],
    purchases: [], allocations: [],
  };
}

test('D01 projected installments match the shared backend fixture', () => {
  const { loan, projection } = fixture.d01;
  const portfolio = loanPortfolio(
    loan.principal, loan.installments, [loan.first_payment], loan.outstanding_after_first,
  );
  for (const month of ['2026-01', '2026-02', '2026-03']) {
    assert.deepEqual(creditProjection(emptyView, portfolio, month), projection[month], month);
  }
  assert.equal(
    loan.installments.reduce((sum, i) => sum + BigInt(i.principal), 0n),
    BigInt(loan.principal),
  );
});

test('D02 settled and partial extra payments leave no fictitious debt', () => {
  const { principal, installments, settled_payments, partial_payments, settled_total, partial_total, partial_outstanding } = fixture.d02;
  const settled = loanPortfolio(principal, installments, settled_payments, 0);
  assert.equal(creditProjection(emptyView, settled, '2026-03').total, settled_total);
  assert.equal(settled.loans[0].installments.reduce((s, i) => s + i.principal, 0), principal);

  const partial = loanPortfolio(principal, installments, partial_payments, partial_outstanding);
  const projection = creditProjection(emptyView, partial, '2026-03');
  assert.equal(projection.total, partial_total);
  assert.equal(projection.items[0].amount, partial_total);
});

test('D03 FIFO payments use the selected month end as reference date', () => {
  const { loan } = fixture.d01;
  const payments = [
    { date: '2026-01-15', principal: 0, interest: 12000 },
    { date: '2026-01-31', principal: 396027, interest: 0 },
    { date: '2026-03-05', principal: 399987, interest: 8040 },
  ];
  const portfolio = loanPortfolio(loan.principal, loan.installments, payments, loan.principal - 396027 - 399987);
  assert.equal(creditProjection(emptyView, portfolio, '2026-01').total, 0);
  assert.equal(creditProjection(emptyView, portfolio, '2026-02').total, 408027);
  assert.equal(creditProjection(emptyView, portfolio, '2026-03').total, 408026);
});

test('D04 V1 and V2 tables are used verbatim, with exact integers', () => {
  const { principal, v1_interest, v2_interest } = fixture.d04;
  const projectionFor = interest => {
    const portfolio = loanPortfolio(
      principal,
      [{ date: fixture.d04.first_due, principal, interest }],
      [], principal,
    );
    return creditProjection(emptyView, portfolio, '2025-01');
  };
  for (const interest of [v1_interest, v2_interest]) {
    const expected = Number(BigInt(principal) + BigInt(interest));
    assert.ok(expected <= Number.MAX_SAFE_INTEGER);
    assert.equal(projectionFor(interest).total, expected);
  }
  assert.equal(projectionFor(v1_interest).total + 1, projectionFor(v2_interest).total);
});

test('D05 imported card signs drive the statement and projection consistently', () => {
  const { month, debt, credit, zero, no_cutoff } = fixture.d05;
  const card = (id, values) => ({
    id, kind: 'card', name: id, closing_day: 20, due_day: 10,
    initial_balance: values.initial_balance, cutoff_date: values.cutoff_date,
  });
  const viewFor = cards => ({ ...emptyView, accounts: cards });
  const portfolio = { loans: [], purchases: [], allocations: [] };

  const debtView = viewFor([card('debt', debt)]);
  assert.equal(cardStatement(debtView.accounts[0], '2026-11', [], portfolio).opening, debt.statement_opening);
  assert.equal(cardStatement(debtView.accounts[0], '2026-11', [], portfolio).outstanding, debt.statement_outstanding);
  const debtProjection = creditProjection(debtView, portfolio, month);
  assert.equal(debtProjection.total, debt.projection_total);
  assert.equal(debtProjection.items[0].date, debt.projection_due);

  for (const [name, values] of [['credit', credit], ['zero', zero], ['no_cutoff', no_cutoff]]) {
    const view = viewFor([card(name, values)]);
    assert.equal(creditProjection(view, portfolio, month).total, values.projection_total, name);
  }
  assert.equal(cardStatement(debtView.accounts[0], '2026-10', [], portfolio).opening, 0);
});

test('D07 unallocated payment leaves the statement pending but reduces total debt', () => {
  const account = { id: 'visa', kind: 'card', name: 'Visa', closing_day: 20, due_day: 10 };
  const operations = [
    { id: 'buy', kind: 'expense', amount: 100, date: '2026-11-10', deleted: false, account_id: 'visa' },
    { id: 'pay', kind: 'payment', amount: 100, date: '2026-11-15', deleted: false, account_id: 'bank', card_account_id: 'visa', version: 1 },
  ];
  const noAllocation = { loans: [], purchases: [], allocations: [] };
  const statement = cardStatement(account, '2026-11', operations, noAllocation);
  assert.deepEqual([statement.charges, statement.paid, statement.outstanding], [100, 0, 100]);
  // The card balance itself is zero after the payment; only the statement differs.
  assert.equal(cardStatement(account, '2026-11', operations, {
    loans: [], purchases: [],
    allocations: [{ operation_id: 'pay', month: '2026-11', amount: 100, payment_version: 1 }],
  }).outstanding, 0);
  const view = { accounts: [account], categories: [], operations };
  assert.equal(creditProjection(view, noAllocation, '2026-12').total, 100);
});

test('D08 pending, conflict and deleted operations are not confirmed payments', () => {
  const account = { id: 'visa', kind: 'card', name: 'Visa', closing_day: 20, due_day: 10 };
  const base = { id: 'pay', kind: 'payment', amount: 100, date: '2026-11-15', account_id: 'bank', card_account_id: 'visa', version: 1, deleted: false };
  const operations = [
    { ...base, id: 'pay' },
    { ...base, id: 'pending', pending: true },
    { ...base, id: 'conflict', conflict: true },
    { ...base, id: 'deleted', deleted: true },
  ];
  const portfolio = {
    loans: [], purchases: [],
    allocations: [{ operation_id: 'pay', month: '2026-11', amount: 100, payment_version: 1 }],
  };
  // Only 'pay' exists in the portfolio allocation, so none of the synthetic
  // pending/conflict/deleted operations can reduce the statement.
  const view = { accounts: [account], categories: [], operations };
  const statement = cardStatement(account, '2026-11', operations, portfolio);
  assert.equal(statement.paid, 100);
  // creditProjection drops pending/conflict before calling cardStatement.
  const projectionView = { ...view, operations: operations.filter(o => !o.pending && !o.conflict) };
  assert.equal(creditProjection(projectionView, portfolio, '2026-12').total, 0);
  const withPending = creditProjection(view, portfolio, '2026-12');
  assert.equal(withPending.total, 0, 'pending/conflict rows never become confirmed payments');
});
