import { test } from 'node:test';
import assert from 'node:assert/strict';
import { renderCredit } from '../static/credit.mjs';

// Minimal DOM so the real renderCredit() runs under node. Only the APIs it uses.
class Node {
  constructor(tag) {
    this.tag = tag;
    this.children = [];
    this.parentNode = null;
    this.textContent = '';
    this.attributes = {};
    this.listeners = {};
  }
  get lastChild() { return this.children.at(-1) ?? null; }
  append(...nodes) {
    for (const n of nodes) {
      if (n.parentNode) n.parentNode.children = n.parentNode.children.filter(c => c !== n);
      n.parentNode = this;
      this.children.push(n);
    }
  }
  replaceChildren(...nodes) { this.children = []; this.append(...nodes); }
  remove() { if (this.parentNode) this.parentNode.children = this.parentNode.children.filter(c => c !== this); this.parentNode = null; }
  setAttribute(k, v) { this.attributes[k] = v; }
  addEventListener(type, fn) { (this.listeners[type] ??= []).push(fn); }
  dispatch(type, ev = {}) { for (const fn of this.listeners[type] ?? []) fn({ preventDefault() {}, ...ev }); }
  querySelector(sel) {
    for (const c of this.children) {
      if (sel === 'button' && c.tag === 'button') return c;
      const found = c.querySelector?.(sel);
      if (found) return found;
    }
    return null;
  }
  all(pred, out = []) {
    if (pred(this)) out.push(this);
    for (const c of this.children) c.all(pred, out);
    return out;
  }
  named(name) { return this.all(n => n.name === name)[0]; }
  get value() { return this._value ?? ''; }
  set value(v) { this._value = v; }
}

globalThis.document = { createElement: tag => new Node(tag) };
globalThis.localStorage = { getItem: () => JSON.stringify({ id: 'u' }) };
Object.defineProperty(globalThis, 'navigator', { value: { onLine: true }, configurable: true });
globalThis.FormData = class {
  constructor(form) {
    this.entries = new Map();
    for (const n of form.all(n => n.name && n.tag !== 'button')) this.entries.set(n.name, n.value);
  }
  *[Symbol.iterator]() { yield* this.entries; }
};

const portfolio = () => ({
  loans: [{ id: 'loan1', name: 'Préstamo', principal: 1000, outstanding: 1000,
    draw_date: '2026-01-01', first_due: '2026-02-01', periods: 2, method: 'german',
    rate_bps: 0, rate_type: 'nominal', account_id: 'bank', installments: [], payments: [] }],
  purchases: [], allocations: [],
});
const view = () => ({
  accounts: [{ id: 'bank', kind: 'bank', name: 'Banco' }],
  categories: [{ id: 'interest', kind: 'expense', name: 'Intereses' }],
  operations: [],
});

function harness(apiImpl, { failOnce = false } = {}) {
  const posts = [];
  let failNext = failOnce;
  const container = new Node('div');
  const api = async (path, method = 'GET', data = null) => {
    if (path === '/api/credit' && method === 'GET') return portfolio();
    if (path === '/api/credit' && method === 'POST') {
      posts.push(data);
      if (failNext) { failNext = false; throw Object.assign(new Error('connection lost'), { status: 0 }); }
      return null;
    }
    throw new Error('unexpected api call ' + path + ' ' + method);
  };
  const ctx = { api, view: view(), today: '2026-11-20', userId: 'u', synchronize: async () => {} };
  const ready = () => container.all(n => n.tag === 'button' && n.textContent === 'Registrar pago')
    .map(b => b.parentNode);
  const status = () => container.all(n => n.tag === 'p' && n.attributes.role === 'status')[0]?.textContent;
  return { container, ctx, posts, ready, status, fail: () => { failNext = true; } };
}

function fillPayment(form, principal = '100') {
  form.named('account_id').value = 'bank';
  form.named('date').value = '2026-11-20';
  form.named('principal').value = principal;
  form.named('interest').value = '0';
  form.named('fee').value = '0';
  form.named('category_id').value = '';
}

async function settle() {
  // renderCredit re-renders on success; flush its awaited GET.
  await new Promise(r => setImmediate(r));
  await new Promise(r => setImmediate(r));
}

test('A/E: two identical legitimate payments use different request_ids', async () => {
  const h = harness(() => {});
  await renderCredit(h.container, h.ctx);
  const first = h.ready()[0];
  fillPayment(first);
  first.dispatch('submit');
  await settle();
  const second = h.ready()[0];
  fillPayment(second);
  second.dispatch('submit');
  await settle();
  assert.equal(h.posts.length, 2);
  assert.notEqual(h.posts[0].request_id, h.posts[1].request_id);
  assert.deepEqual(h.posts[0].entry, h.posts[1].entry);
});

test('C: a double click while a request is pending sends only one POST', async () => {
  const h = harness(() => {});
  await renderCredit(h.container, h.ctx);
  const form = h.ready()[0];
  fillPayment(form);
  form.dispatch('submit');
  form.dispatch('submit');
  await settle();
  assert.equal(h.posts.length, 1);
});

test('D: a failed network retry keeps the same request_id', async () => {
  const h = harness(() => {}, { failOnce: true });
  await renderCredit(h.container, h.ctx);
  const form = h.ready()[0];
  fillPayment(form);
  form.dispatch('submit');
  await settle();
  assert.equal(h.status(), 'connection lost');
  const retry = h.ready()[0];
  fillPayment(retry);
  retry.dispatch('submit');
  await settle();
  assert.equal(h.posts.length, 2);
  assert.equal(h.posts[0].request_id, h.posts[1].request_id);
  assert.notEqual(h.status(), 'connection lost');
});

test('F: the UI does not say Guardado after a rejected POST', async () => {
  const h = harness(() => {});
  await renderCredit(h.container, h.ctx);
  const form = h.ready()[0];
  fillPayment(form);
  h.fail();
  form.dispatch('submit');
  await settle();
  assert.equal(h.posts.length, 1);
  assert.notEqual(h.status(), 'Guardado.');
});

// F10.1C: a corrupted session value must degrade to the session message, not a crash.
test('G: corrupted localStorage degrades to the session message', async () => {
  const original = globalThis.localStorage;
  globalThis.localStorage = { getItem: () => '{bad' };
  try {
    const h = harness(() => {});
    await renderCredit(h.container, h.ctx);
    const form = h.ready()[0];
    fillPayment(form);
    form.dispatch('submit');
    await settle();
    assert.equal(h.posts.length, 0);
    assert.equal(h.status(), 'La sesión cambió. Vuelve a abrir Créditos.');
  } finally {
    globalThis.localStorage = original;
  }
});

test('H: unpaid loan renders Anular préstamo action and posts cancel_loan', async () => {
  const h = harness(() => {});
  await renderCredit(h.container, h.ctx);
  const cancelBtn = h.container.all(n => n.tag === 'button' && n.textContent === 'Anular préstamo')[0];
  assert.ok(cancelBtn, 'Anular préstamo button is rendered for a loan without payments');
  cancelBtn.dispatch('click');
  await settle();
  assert.equal(h.posts.length, 1);
  assert.deepEqual(h.posts[0].entry, { kind: 'cancel_loan', loan_id: 'loan1' });
});
