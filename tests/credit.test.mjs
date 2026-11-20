import { test } from 'node:test';
import assert from 'node:assert/strict';
import { cardStatement } from '../static/credit.mjs';

// An imported card with a cutoff date starts with its opening debt; it must show up
// as a due amount exactly once, in the cycle that contains the cutoff.
test('opening card debt is charged once in the cycle containing its cutoff', () => {
  const card = {id:'visa', kind:'card', name:'Visa', initial_balance:-500, closing_day:20, due_day:10, cutoff_date:'2026-10-05'};
  const portfolio = {purchases:[], allocations:[]};
  assert.deepEqual(
    cardStatement(card, '2026-10', [], portfolio),
    {closing:'2026-10-20', due:'2026-11-10', charges:500, paid:0, outstanding:500, opening:500}
  );
  assert.equal(cardStatement(card, '2026-09', [], portfolio).opening, 0);
  assert.equal(cardStatement(card, '2026-11', [], portfolio).opening, 0);
});

test('opening credit on a card reduces the charges of the cutoff cycle', () => {
  const card = {id:'visa', kind:'card', name:'Visa', initial_balance:200, closing_day:20, due_day:10, cutoff_date:'2026-10-05'};
  const portfolio = {purchases:[], allocations:[]};
  const statement = cardStatement(card, '2026-10', [], portfolio);
  assert.equal(statement.charges, -200);
  assert.equal(statement.opening, 0);
});

test('cards without a cutoff never fold the initial balance into a cycle', () => {
  const card = {id:'visa', kind:'card', name:'Visa', initial_balance:-500, closing_day:20, due_day:10};
  assert.equal(cardStatement(card, '2026-10', [], {purchases:[], allocations:[]}).opening, 0);
});
