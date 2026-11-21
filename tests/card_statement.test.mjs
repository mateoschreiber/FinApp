import { test } from 'node:test';
import assert from 'node:assert/strict';
import { cardStatement } from '../static/credit.mjs';

const card = (overrides = {}) => ({
  id: 'visa', kind: 'card', name: 'Visa', closing_day: 20, due_day: 5,
  ...overrides,
});
const op = (id, kind, amount, date, extra = {}) => ({
  id, kind, amount, date, deleted: false, account_id: 'visa', ...extra,
});
const empty = { purchases: [], allocations: [] };

// Cycle is (previous month's closing day, this month's closing day], inclusive
// on the upper bound. Due is this month's due day when it is after closing,
// otherwise next month's due day.
test('cycle boundaries and next-month due date', () => {
  const operations = [
    op('in', 'expense', 10, '2026-02-10'),
    op('on-close', 'expense', 20, '2026-02-20'),
    op('after', 'expense', 40, '2026-02-21'),
    op('old', 'expense', 5, '2026-01-19'),
    op('prev-cycle', 'expense', 7, '2026-01-21'),
  ];
  assert.deepEqual(cardStatement(card(), '2026-02', operations, empty), {
    closing: '2026-02-20', due: '2026-03-05',
    charges: 37, paid: 0, outstanding: 37, opening: 0,
  });
  assert.equal(cardStatement(card(), '2026-01', operations, empty).charges, 5);
});

test('due day after closing stays in the same month', () => {
  const statement = cardStatement(card({ due_day: 25 }), '2026-02', [], empty);
  assert.equal(statement.closing, '2026-02-20');
  assert.equal(statement.due, '2026-02-25');
});

test('leap year, clamped days and december rollover', () => {
  const leap = cardStatement(card({ closing_day: 29, due_day: 31 }), '2024-02', [], empty);
  assert.equal(leap.closing, '2024-02-29');
  assert.equal(leap.due, '2024-03-31');
  const jan = cardStatement(card({ closing_day: 31, due_day: 31 }), '2024-01', [], empty);
  assert.equal(jan.closing, '2024-01-31');
  assert.equal(jan.due, '2024-02-29');
  const short = cardStatement(card({ closing_day: 31, due_day: 5 }), '2026-02', [], empty);
  assert.equal(short.closing, '2026-02-28');
  assert.equal(short.due, '2026-03-05');
  const rollover = cardStatement(card(), '2026-12', [], empty);
  assert.equal(rollover.closing, '2026-12-20');
  assert.equal(rollover.due, '2027-01-05');
});

test('financed purchase charges one installment per cycle, never the full amount', () => {
  const account = card({ closing_day: 20, due_day: 5 });
  const portfolio = {
    purchases: [{
      operation_id: 'buy',
      installments: [
        { date: '2026-01-15', principal: 100 },
        { date: '2026-02-15', principal: 100 },
        { date: '2026-03-15', principal: 100 },
      ],
    }],
    allocations: [],
  };
  const operations = [op('buy', 'expense', 300, '2026-01-02')];
  for (const month of ['2026-01', '2026-02', '2026-03']) {
    const statement = cardStatement(account, month, operations, portfolio);
    assert.equal(statement.charges, 100, month);
  }
  const total = ['2026-01', '2026-02', '2026-03']
    .reduce((sum, month) => sum + cardStatement(account, month, operations, portfolio).charges, 0);
  assert.equal(total, 300);
});

test('only allocations with the current payment version reduce the statement', () => {
  const operations = [
    op('buy', 'expense', 100, '2026-11-10'),
    op('pay', 'payment', 70, '2026-11-15', { card_account_id: 'visa', version: 1 }),
  ];
  const current = {
    purchases: [],
    allocations: [{ operation_id: 'pay', month: '2026-11', amount: 70, payment_version: 1 }],
  };
  assert.deepEqual(
    [cardStatement(card(), '2026-11', operations, current).paid,
      cardStatement(card(), '2026-11', operations, current).outstanding],
    [70, 30],
  );
  const stale = {
    purchases: [],
    allocations: [{ operation_id: 'pay', month: '2026-11', amount: 70, payment_version: 2 }],
  };
  assert.equal(cardStatement(card(), '2026-11', operations, stale).outstanding, 100);
});

test('first installment after closing belongs to the next statement', () => {
  const account = card({ closing_day: 20, due_day: 5 });
  const portfolio = {
    purchases: [{ operation_id: 'buy', installments: [{ date: '2026-11-25', principal: 100 }] }],
    allocations: [],
  };
  const operations = [op('buy', 'expense', 100, '2026-11-02')];
  assert.equal(cardStatement(account, '2026-11', operations, portfolio).charges, 0);
  assert.equal(cardStatement(account, '2026-12', operations, portfolio).charges, 100);
});

test('two financed purchases in one cycle and a year change', () => {
  const account = card({ closing_day: 20, due_day: 5 });
  const portfolio = {
    purchases: [
      { operation_id: 'a', installments: [{ date: '2026-11-10', principal: 100 }] },
      { operation_id: 'b', installments: [{ date: '2026-11-15', principal: 100 }] },
      { operation_id: 'c', installments: [{ date: '2026-12-25', principal: 100 }] },
    ],
    allocations: [],
  };
  const operations = [
    op('a', 'expense', 100, '2026-11-01'),
    op('b', 'expense', 100, '2026-11-02'),
    op('c', 'expense', 100, '2026-12-02'),
  ];
  assert.equal(cardStatement(account, '2026-11', operations, portfolio).charges, 200);
  assert.equal(cardStatement(account, '2027-01', operations, portfolio).charges, 100);
});

// Backend convention: positive card balance = debt, negative = credit. The statement
// must use that same sign in the cutoff cycle so debt is shown as payable.
test('cutoff-cycle opening balance follows the backend sign', () => {
  const debt = card({ initial_balance: 300, cutoff_date: '2026-10-05' });
  assert.deepEqual(
    [cardStatement(debt, '2026-10', [], empty).charges,
      cardStatement(debt, '2026-10', [], empty).opening,
      cardStatement(debt, '2026-10', [], empty).outstanding],
    [300, 300, 300],
  );
  const credit = card({ initial_balance: -300, cutoff_date: '2026-10-05' });
  assert.deepEqual(
    [cardStatement(credit, '2026-10', [], empty).charges,
      cardStatement(credit, '2026-10', [], empty).opening,
      cardStatement(credit, '2026-10', [], empty).outstanding],
    [-300, 0, -300],
  );
  const boundary = card({ initial_balance: 300, cutoff_date: '2026-10-20' });
  assert.equal(cardStatement(boundary, '2026-10', [], empty).opening, 300);
  const outside = card({ initial_balance: 300, cutoff_date: '2026-10-21' });
  assert.equal(cardStatement(outside, '2026-10', [], empty).opening, 0);
  const withoutCutoff = card({ initial_balance: -500 });
  assert.equal(cardStatement(withoutCutoff, '2026-10', [], empty).opening, 0);
});
