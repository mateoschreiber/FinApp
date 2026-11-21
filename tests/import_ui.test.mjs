import { test } from 'node:test';
import assert from 'node:assert/strict';
import { renderData } from '../static/extras.mjs';

class El {
  constructor(tag) {
    this.tag = tag;
    this.children = [];
    this.parentNode = null;
    this.textContent = '';
    this.attributes = {};
    this.listeners = {};
    this.dataset = {};
    this.disabled = false;
    this.type = '';
    this.value = '';
    this.checked = false;
    this.files = [];
    this.className = '';
    this.open = false;
  }
  append(...nodes) {
    for (const n of nodes) {
      if (n.parentNode) n.parentNode.children = n.parentNode.children.filter(c => c !== n);
      n.parentNode = this;
      this.children.push(n);
    }
  }
  prepend(...nodes) { this.append(...nodes.reverse()); }
  replaceChildren(...nodes) { this.children = []; this.append(...nodes); }
  setAttribute(k, v) { this.attributes[k] = v; }
  addEventListener(type, fn) { (this.listeners[type] ??= []).push(fn); }
  dispatch(type, ev = {}) { for (const fn of this.listeners[type] ?? []) fn({ preventDefault() {}, ...ev }); }
  matches(sel) {
    if (sel === '[data-confirm]') return 'confirm' in this.dataset;
    if (sel === 'button, select') return this.tag === 'button' || this.tag === 'select';
    if (sel === 'select') return this.tag === 'select';
    if (sel === 'button') return this.tag === 'button';
    return false;
  }
  querySelector(sel) { return this.querySelectorAll(sel)[0] ?? null; }
  querySelectorAll(sel) {
    const out = [];
    const visit = n => { if (n.matches(sel)) out.push(n); for (const c of n.children) visit(c); };
    visit(this);
    return out;
  }
  all(pred, out = []) { if (pred(this)) out.push(this); for (const c of this.children) c.all(pred, out); return out; }
}

globalThis.document = { createElement: t => new El(t), createTextNode: t => { const n = new El('#text'); n.textContent = t; return n; } };
globalThis.confirm = () => true;

const fakeFile = () => ({ size: 1024, arrayBuffer: async () => new Uint8Array([1, 2, 3]).buffer });
const textOf = node => (node.textContent || '') + node.children.map(textOf).join('');

function difference(declared, calculated) {
  return { account_id: 'banco', declared_balance: declared, calculated_balance: calculated, difference: declared - calculated };
}

function fixture(report) {
  const container = new El('div');
  const calls = [];
  let step = 0;
  const api = async (path, method, body) => {
    calls.push({ path, method, body });
    const next = report[Math.min(step++, report.length - 1)];
    if (next instanceof Error) throw next;
    return next;
  };
  const ctx = {
    api, getView: () => ({
      accounts: [],
      categories: [{ id: 'food', name: 'Comida', kind: 'expense' }],
      operations: [{ id: 'expense-existing', kind: 'expense', date: '2026-03-01', amount: 500, description: 'Gasto previo' }],
    }), today: '2026-11-20',
    synchronize: async () => {}, isCurrent: () => true,
  };
  renderData(container, ctx);
  const cutoff = () => container.all(n => n.tag === 'input' && n.type === 'checkbox').find(n => textOf(n.parentNode).includes('Incluir movimientos'));
  const file = () => container.all(n => n.tag === 'input' && n.type === 'file')[0];
  const form = () => container.all(n => n.tag === 'form')[0];
  const confirmButton = () => container.all(n => 'confirm' in n.dataset)[0] ?? null;
  const acceptBox = () => container.all(n => n.tag === 'input' && n.type === 'checkbox').find(n => textOf(n.parentNode).includes('He revisado'));
  const status = () => container.all(n => n.tag === 'p' && n.attributes.role === 'status')[0];
  return { container, calls, cutoff, file, form, confirmButton, acceptBox, status };
}

async function settle(times = 6) {
  for (let i = 0; i < times; i++) await new Promise(r => setImmediate(r));
}

test('differences are shown, the confirmation waits for acceptance and sends ack_saldo', async () => {
  const preview = {
    accounts: [], added: 2, duplicates: 0, skipped: 0, errors: [], warnings: ['Cuenta banco: saldo esperado 1000, saldo calculado 900.'],
    differences: [difference(1000, 900)], balances: { banco: '900' }, token: 'token-1', committed: false,
  };
  const done = { ...preview, committed: true };
  const f = fixture([preview, done]);
  f.file().files = [fakeFile()];
  f.form().dispatch('submit');
  await settle();

  const summary = f.container.all(n => n.tag === 'summary').find(s => s.textContent.includes('Diferencias de saldo'));
  assert.ok(summary, 'the discrepancy section is visible');
  assert.ok(summary.parentNode.textContent.includes('1.000 Gs.') || textOf(summary.parentNode).includes('1.000 Gs.'), 'declared amount is formatted');
  assert.ok(textOf(summary.parentNode).includes('900 Gs.'), 'calculated amount is formatted');
  assert.ok(f.confirmButton().disabled, 'confirmation is disabled without acceptance');
  const accept = f.acceptBox();
  assert.ok(accept && !accept.checked, 'the acceptance box starts unchecked');
  accept.checked = true;
  accept.dispatch('change');
  assert.ok(!f.confirmButton().disabled, 'confirmation enables after acceptance');

  f.confirmButton().dispatch('click');
  await settle();
  const input = f.calls.at(-1).body;
  assert.equal(input.preview_token, 'token-1');
  assert.equal(input.ack_saldo, 'token-1', 'ack is bound to the reviewed preview');
});

test('matching balances import without ack_saldo', async () => {
  const preview = {
    accounts: [], added: 1, duplicates: 0, skipped: 0, errors: [], warnings: [],
    differences: [], balances: { banco: '900' }, token: 'token-2', committed: false,
  };
  const f = fixture([preview, { ...preview, committed: true }]);
  f.file().files = [fakeFile()];
  f.form().dispatch('submit');
  await settle();
  assert.ok(!f.confirmButton().disabled);
  f.confirmButton().dispatch('click');
  await settle();
  const input = f.calls.at(-1).body;
  assert.equal(input.preview_token, 'token-2');
  assert.equal('ack_saldo' in input, false);
});

test('changing the file or the cutoff flag resets the acceptance', async () => {
  const preview = {
    accounts: [], added: 2, duplicates: 0, skipped: 0, errors: [], warnings: [],
    differences: [difference(1000, 900)], balances: { banco: '900' }, token: 'token-3', committed: false,
  };
  const f = fixture([preview]);
  f.file().files = [fakeFile()];
  f.form().dispatch('submit');
  await settle();
  const accept = f.acceptBox();
  accept.checked = true;
  accept.dispatch('change');
  assert.ok(!f.confirmButton().disabled);

  f.cutoff().dispatch('change');
  assert.ok(f.confirmButton().disabled, 'cutoff change requires a new preview');

  f.form().dispatch('submit');
  await settle();
  const fresh = f.acceptBox();
  assert.ok(fresh && !fresh.checked, 'a new preview starts unchecked');
  f.file().dispatch('change');
  assert.equal(f.confirmButton(), null, 'changing the file clears the previous preview');
});

test('a double click only sends one confirmation', async () => {
  const preview = {
    accounts: [], added: 2, duplicates: 0, skipped: 0, errors: [], warnings: [],
    differences: [difference(1000, 900)], balances: { banco: '900' }, token: 'token-4', committed: false,
  };
  let release;
  const gate = new Promise(resolve => { release = resolve; });
  const calls = [];
  let step = 0;
  const api = async () => {
    calls.push(++step);
    if (step === 1) return preview;
    await gate;
    return { ...preview, committed: true };
  };
  const container = new El('div');
  renderData(container, {
    api, getView: () => ({ accounts: [] }), today: '2026-11-20',
    synchronize: async () => {}, isCurrent: () => true,
  });
  const file = container.all(n => n.tag === 'input' && n.type === 'file')[0];
  file.files = [fakeFile()];
  const form = container.all(n => n.tag === 'form')[0];
  form.dispatch('submit');
  await settle();
  assert.deepEqual(calls, [1]);
  const accept = container.all(n => n.tag === 'input' && n.type === 'checkbox').find(n => textOf(n.parentNode).includes('He revisado'));
  accept.checked = true;
  accept.dispatch('change');
  const confirm = container.all(n => 'confirm' in n.dataset)[0];
  confirm.dispatch('click');
  confirm.dispatch('click');
  assert.deepEqual(calls, [1, 2], 'only one confirmation request is in flight');
  release();
  await settle();
  assert.deepEqual(calls, [1, 2], 'the second click was ignored while the first was in flight');
});

test('category links are offered and invalidate the preview', async () => {
  const preview = {
    accounts: [], categories: [{ id: 'cat-food', name: 'Comida externa', kind: 'expense', linked: null }],
    references: [], added: 2, duplicates: 0, skipped: 0, errors: [], warnings: [],
    differences: [], balances: {}, token: 'token-cat', committed: false,
  };
  const done = { ...preview, committed: true };
  const f = fixture([preview, preview, done]);
  f.file().files = [fakeFile()];
  f.form().dispatch('submit');
  await settle();
  const select = f.container.all(n => n.tag === 'select').find(n => textOf(n.parentNode).includes('Comida externa'));
  assert.ok(select, 'the category selector is rendered');
  select.value = 'food';
  select.dispatch('change');
  assert.ok(f.confirmButton().disabled, 'changing a category link requires a new preview');
  assert.ok(f.status().textContent.includes('vista previa'), 'the user is told to regenerate');
  f.form().dispatch('submit');
  await settle();
  f.confirmButton().dispatch('click');
  await settle();
  const input = f.calls.at(-1).body;
  assert.deepEqual(input.category_mapping, { 'cat-food': 'food' });
});

test('unresolved operation references offer a bounded selector and invalidate the preview', async () => {
  const preview = {
    accounts: [], categories: [], references: [{ source_id: 'e1', movement_id: 'r1' }],
    added: 2, duplicates: 0, skipped: 0, errors: [], warnings: [],
    differences: [], balances: {}, token: 'token-ref', committed: false,
  };
  const done = { ...preview, committed: true };
  const f = fixture([preview, preview, done]);
  f.file().files = [fakeFile()];
  f.form().dispatch('submit');
  await settle();
  const select = f.container.all(n => n.tag === 'select').find(n => textOf(n.parentNode).includes('referencia e1'));
  assert.ok(select, 'the reference selector is rendered');
  assert.ok(textOf(select.parentNode).includes('Gasto previo'), 'existing expenses are offered');
  select.value = 'expense-existing';
  select.dispatch('change');
  assert.ok(f.confirmButton().disabled, 'changing a reference requires a new preview');
  f.form().dispatch('submit');
  await settle();
  f.confirmButton().dispatch('click');
  await settle();
  const input = f.calls.at(-1).body;
  assert.deepEqual(input.operation_mapping, { e1: 'expense-existing' });
});

test('server rejection is shown and the confirmation is disabled', async () => {
  const preview = {
    accounts: [], added: 2, duplicates: 0, skipped: 0, errors: [], warnings: [],
    differences: [difference(1000, 900)], balances: { banco: '900' }, token: 'token-5', committed: false,
  };
  const rejection = new Error('Hay diferencias de saldo. Marca la aceptación de la vista previa antes de importar.');
  const f = fixture([preview, rejection]);
  f.file().files = [fakeFile()];
  f.form().dispatch('submit');
  await settle();
  const accept = f.acceptBox();
  accept.checked = true;
  accept.dispatch('change');
  f.confirmButton().dispatch('click');
  await settle();
  assert.ok(f.status().textContent.includes('diferencias de saldo'), 'the rejection message is visible');
  assert.ok(f.confirmButton().disabled, 'confirmation is disabled after rejection');
});
