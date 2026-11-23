import {test} from 'node:test';
import assert from 'node:assert/strict';
import {readFileSync} from 'node:fs';
import {
  expenseCategorySlices,
  formatRelativeDate,
  initialsOf,
  monthlyCashflowSeries,
  passwordStrength,
} from '../static/visuals.mjs';

test('initialsOf extracts clean 1- or 2-letter uppercase initials', () => {
  assert.equal(initialsOf('admin'), 'AD');
  assert.equal(initialsOf('María González'), 'MG');
  assert.equal(initialsOf('  juan   carlos  perez '), 'JC');
  assert.equal(initialsOf(''), '?');
});

test('passwordStrength scores passwords against the 12-char minimum policy', () => {
  assert.equal(passwordStrength('').score, 0);
  assert.equal(passwordStrength('short123').score, 1);
  assert.equal(passwordStrength('short123').tone, 'negative');
  assert.equal(passwordStrength('abcdefghijkl').score, 1);
  assert.equal(passwordStrength('abcdefghijkl').tone, 'warning');
  assert.equal(passwordStrength('Abcdefghijkl1').score, 3);
  assert.equal(passwordStrength('Abcdefghijkl1').tone, 'positive');
  assert.equal(passwordStrength('Abcdefghijkl1234!').score, 4);
});

test('formatRelativeDate formats today, yesterday and localized dates', () => {
  assert.equal(formatRelativeDate('2026-05-10', '2026-05-10'), 'Hoy');
  assert.equal(formatRelativeDate('2026-05-09', '2026-05-10'), 'Ayer');
  const older = formatRelativeDate('2026-05-01', '2026-05-10');
  assert.ok(older.length > 0 && older !== 'Hoy' && older !== 'Ayer');
});

test('monthlyCashflowSeries builds cumulative PYG BigInt points ignoring deleted and transfer ops', () => {
  const ops = [
    {date: '2026-05-01', kind: 'income', amount: 5000000, deleted: false},
    {date: '2026-05-02', kind: 'expense', amount: 1200000, deleted: false},
    {date: '2026-05-02', kind: 'transfer', amount: 900000, deleted: false},
    {date: '2026-05-03', kind: 'refund', amount: 200000, deleted: false},
    {date: '2026-05-03', kind: 'expense', amount: 999999, deleted: true},
    {date: '2026-04-30', kind: 'expense', amount: 100000, deleted: false},
  ];
  const series = monthlyCashflowSeries(ops, '2026-05', '2026-05-04');
  assert.equal(series.points.length, 4);
  assert.equal(series.points[0].cumulative, 5000000n);
  assert.equal(series.points[1].cumulative, 3800000n);
  assert.equal(series.points[2].cumulative, 4000000n);
  assert.equal(series.total, 4000000n);
  assert.equal(series.max, 5000000n);
});

test('expenseCategorySlices aggregates net expenses (minus refunds) and rolls up overflow categories', () => {
  const categories = [
    {id: 'c1', name: 'Alimentación'},
    {id: 'c2', name: 'Transporte'},
    {id: 'c3', name: 'Servicios'},
    {id: 'c4', name: 'Ocio'},
  ];
  const ops = [
    {date: '2026-05-01', kind: 'expense', category_id: 'c1', amount: 600000, deleted: false},
    {date: '2026-05-02', kind: 'refund', category_id: 'c1', amount: 100000, deleted: false},
    {date: '2026-05-03', kind: 'expense', category_id: 'c2', amount: 300000, deleted: false},
    {date: '2026-05-04', kind: 'expense', category_id: 'c3', amount: 150000, deleted: false},
    {date: '2026-05-05', kind: 'expense', category_id: 'c4', amount: 50000, deleted: false},
  ];
  const {total, slices} = expenseCategorySlices(ops, categories, '2026-05', 2);
  assert.equal(total, 1000000n);
  assert.equal(slices.length, 3);
  assert.equal(slices[0].name, 'Alimentación');
  assert.equal(slices[0].amount, 500000n);
  assert.equal(slices[0].percent, 50);
  assert.equal(slices[1].name, 'Transporte');
  assert.equal(slices[1].percent, 30);
  assert.equal(slices[2].name, 'Otras categorías');
  assert.equal(slices[2].amount, 200000n);
});

test('every static DOM id referenced via $() in app.mjs exists in index.html', () => {
  const html = readFileSync('static/index.html', 'utf8');
  const app = readFileSync('static/app.mjs', 'utf8');
  const missing = [];
  for (const match of app.matchAll(/\$\('([a-z0-9-]+)'\)/g)) {
    const id = match[1];
    if (!html.includes(`id="${id}"`)) missing.push(id);
  }
  assert.deepEqual([...new Set(missing)], []);
});
