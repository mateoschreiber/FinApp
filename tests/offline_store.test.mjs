import { test } from 'node:test';
import assert from 'node:assert/strict';
import { indexedDB } from 'fake-indexeddb';

globalThis.indexedDB = indexedDB;
const { openUserStore } = await import('../static/offline-store.mjs');

test('snapshot and queued operations are isolated by user', async () => {
  const alice = await openUserStore('alice');
  const bob = await openUserStore('bob');
  await alice.replaceSnapshot({ accounts: [{ id: 'a' }] });
  const op = await alice.enqueue({ recordId: 'r1', action: 'create', baseVersion: null, data: { amount: 42 } });
  assert.ok(op.id);
  assert.deepEqual((await alice.pending()).map(x => x.id), [op.id]);
  assert.equal(await bob.snapshot(), undefined);
  assert.deepEqual(await bob.pending(), []);
  alice.close(); bob.close();
});

test('conflict preserves both versions until an explicit new operation resolves it', async () => {
  const store = await openUserStore('conflict-user');
  const original = await store.enqueue({ recordId: 'r', action: 'update', baseVersion: 3, data: { amount: 10 } });
  await store.markConflict(original.id, { id: 'r', version: 4, data: { amount: 20 } });
  const [conflict] = await store.pending();
  assert.equal(conflict.status, 'conflict');
  assert.deepEqual(conflict.data, { amount: 10 });
  assert.deepEqual(conflict.server, { id: 'r', version: 4, data: { amount: 20 } });
  const resolution = await store.resolveConflict(original.id, { action: 'update', data: { amount: 10 } });
  assert.notEqual(resolution.id, original.id);
  assert.equal(resolution.baseVersion, 4);
  assert.deepEqual((await store.pending()).map(x => x.id), [resolution.id]);
  store.close();
});

test('logout cannot discard changes unless explicitly requested', async () => {
  const store = await openUserStore('logout-user');
  await store.replaceSnapshot({ accounts: ['a'] });
  await store.enqueue({ recordId: 'r', action: 'delete', baseVersion: 1, data: null });
  await assert.rejects(store.clear(), /pending/i);
  await assert.rejects(store.clear({ discardPending: 'yes' }), /pending/i);
  assert.equal((await store.pending()).length, 1);
  await store.clear({ discardPending: true });
  assert.deepEqual(await store.pending(), []);
  assert.equal(await store.snapshot(), undefined);
  store.close();
});

test('acknowledgement does not overwrite a newer cached server version', async () => {
  const store = await openUserStore('stale-ack-user');
  await store.replaceSnapshot({ accounts: [], categories: [], operations: [{ id: 'r', version: 3, amount: 30 }] });
  const op = await store.enqueue({ recordId: 'r', action: 'update', baseVersion: 1, data: { amount: 20 } });
  await store.acknowledge(op.id, { id: 'r', version: 2, amount: 20 });
  assert.equal((await store.snapshot()).operations[0].version, 3);
  store.close();
});

test('dependent edits wait for predecessor acknowledgment and receive its version', async () => {
  const store = await openUserStore('chain-user');
  const first = await store.enqueue({ recordId: 'r', action: 'create', baseVersion: null, data: { amount: 10 } });
  const second = await store.enqueue({ recordId: 'r', action: 'update', baseVersion: null, dependsOn: first.id, data: { amount: 20 } });
  assert.equal(second.dependsOn, first.id);
  await store.replaceSnapshot({ accounts: [], categories: [], operations: [] });
  await store.acknowledge(first.id, { id: 'r', version: 1, amount: 10 });
  const [remaining] = await store.pending();
  assert.equal(remaining.id, second.id);
  assert.equal(remaining.baseVersion, 1);
  assert.equal(remaining.dependsOn, null);
  assert.deepEqual((await store.snapshot()).operations, [{ id: 'r', version: 1, amount: 10 }]);
  store.close();
});

test('unknown acknowledgements and mismatched dependencies do not remove data', async () => {
  const store = await openUserStore('guard-user');
  const op = await store.enqueue({ recordId: 'r1', action: 'create', baseVersion: null, data: {} });
  await assert.rejects(store.acknowledge('missing', { id: 'r1', version: 1 }), /not found/i);
  await assert.rejects(store.enqueue({ recordId: 'r2', action: 'update', baseVersion: null, dependsOn: op.id, data: {} }), /dependency/i);
  assert.deepEqual((await store.pending()).map(x => x.id), [op.id]);
  store.close();
});

test('accepting the server version with dependent edits requires explicit review', async () => {
  const store = await openUserStore('accept-server-user');
  await store.replaceSnapshot({ accounts: [], categories: [], operations: [{ id: 'r', version: 1 }] });
  const first = await store.enqueue({ recordId: 'r', action: 'update', baseVersion: 1, data: { amount: 10 } });
  const later = await store.enqueue({ recordId: 'r', action: 'update', baseVersion: null, dependsOn: first.id, data: { amount: 30 } });
  await store.markConflict(first.id, { id: 'r', version: 2, amount: 20 });
  await assert.rejects(store.acceptServer(first.id), /dependent/i);
  assert.deepEqual((await store.snapshot()).operations, [{ id: 'r', version: 1 }]);
  assert.ok((await store.pending()).some(op => op.id === later.id));
  store.close();
});

test('a missing server version never silently discards dependent changes', async () => {
  const store = await openUserStore('missing-server-user');
  await store.replaceSnapshot({ accounts: [], categories: [], operations: [{ id: 'r', version: 1 }] });
  const first = await store.enqueue({ recordId: 'r', action: 'update', baseVersion: 1, data: { amount: 10 } });
  await store.enqueue({ recordId: 'r', action: 'update', baseVersion: null, dependsOn: first.id, data: { amount: 30 } });
  await store.markConflict(first.id, null);
  await assert.rejects(store.acceptServer(first.id), /dependent/i);
  assert.equal((await store.pending()).length, 2);
  store.close();
});

test('explicit acceptance can discard the entire dependent chain', async () => {
  const store = await openUserStore('accept-discard-chain');
  await store.replaceSnapshot({accounts:[],categories:[],operations:[{id:'r',version:1}]});
  const first = await store.enqueue({recordId:'r',action:'update',baseVersion:1,data:{amount:10}});
  const second = await store.enqueue({recordId:'r',action:'update',baseVersion:null,dependsOn:first.id,data:{amount:20}});
  await store.enqueue({recordId:'r',action:'delete',baseVersion:null,dependsOn:second.id,data:null});
  await store.markConflict(first.id,{id:'r',version:2,amount:30});
  await store.acceptServer(first.id,{discardDependents:true});
  assert.deepEqual(await store.pending(),[]);
  assert.equal((await store.snapshot()).operations[0].amount,30);
  store.close();
});

test('resolving a conflict does not automatically rebase dependent edits', async () => {
  const store = await openUserStore('resolve-dependent');
  const first = await store.enqueue({recordId:'r',action:'update',baseVersion:1,data:{amount:10}});
  await store.enqueue({recordId:'r',action:'update',baseVersion:null,dependsOn:first.id,data:{amount:20}});
  await store.markConflict(first.id,{id:'r',version:2,amount:30});
  await assert.rejects(store.resolveConflict(first.id,{action:'update',data:{amount:10}}), /dependent/i);
  assert.equal((await store.pending()).length,2);
  store.close();
});

test('discarding a rejected change preserves snapshot and rejects dependent loss without explicit approval', async () => {
  const store = await openUserStore('discard-chain');
  const initial = {accounts:[{id:'a'}],categories:[],operations:[{id:'r',version:1}]};
  await store.replaceSnapshot(initial);
  const first = await store.enqueue({recordId:'r',action:'update',baseVersion:1,data:{amount:10}});
  const second = await store.enqueue({recordId:'r',action:'update',baseVersion:null,dependsOn:first.id,data:{amount:20}});
  await store.enqueue({recordId:'r',action:'delete',baseVersion:null,dependsOn:second.id,data:null});
  await assert.rejects(store.discardOperation(first.id), /dependent/i);
  assert.equal((await store.pending()).length,3);
  await assert.rejects(store.discardOperation(first.id,{discardDependents:'yes'}), /dependent/i);
  await store.discardOperation(first.id,{discardDependents:true});
  assert.deepEqual(await store.pending(),[]);
  assert.deepEqual(await store.snapshot(),initial);
  await assert.rejects(store.discardOperation('missing'), /not found/i);
  store.close();
});
