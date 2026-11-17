import { test } from 'node:test';
import assert from 'node:assert/strict';
import { indexedDB } from 'fake-indexeddb';
import { openUserStore } from '../static/offline-store.mjs';
import { syncUser } from '../static/sync.mjs';

globalThis.indexedDB = indexedDB;
const snapshot = () => ({accounts: [], categories: [], operations: []});
const locks = { request: (_name, task) => task() };
const sync = (store, fetcher) => syncUser(store, store.userId, (url, options) =>
  url === '/api/me' ? Promise.resolve(Response.json({id: store.userId})) : fetcher(url, options), locks);

test('submits stable ids, acknowledges and updates the snapshot', async () => {
  const store = await openUserStore('sync-success');
  const item = await store.enqueue({recordId:'record', action:'create', baseVersion:null, data:{kind:'expense', amount:100}});
  const sent = [];
  const fetcher = async (url, options) => {
    assert.equal(options.headers['X-Finapp-User'], 'sync-success');
    if (url === '/api/snapshot') return Response.json(snapshot());
    sent.push(JSON.parse(options.body));
    return Response.json({status:'applied', operation:{id:'record',version:1,kind:'expense',amount:100}});
  };
  const result = await sync(store, fetcher);
  assert.equal(sent[0].request_id, item.id);
  assert.equal(sent[0].id, 'record');
  assert.equal(sent[0].expected_version, 0);
  assert.equal(sent[0].deleted, false);
  assert.equal(result.applied, 1);
  assert.deepEqual(await store.pending(), []);
  assert.equal((await store.snapshot()).operations[0].version, 1);
  store.close();
});

test('retains failed network changes and reuses the same request id on retry', async () => {
  const store = await openUserStore('sync-retry');
  const item = await store.enqueue({recordId:'record', action:'create', baseVersion:null, data:{kind:'income',amount:25}});
  const captured = [];
  await assert.rejects(sync(store, async (url, options) => {
    if (url === '/api/snapshot') return Response.json(snapshot());
    captured.push(JSON.parse(options.body).request_id);
    throw new Error('connection lost');
  }), /connection lost/);
  assert.equal((await store.pending())[0].id, item.id);
  await sync(store, async (url, options) => {
    if (url === '/api/snapshot') return Response.json(snapshot());
    captured.push(JSON.parse(options.body).request_id);
    return Response.json({status:'applied',operation:{id:'record',version:1}});
  });
  assert.deepEqual(captured, [item.id,item.id]);
  store.close();
});

test('conflict keeps both versions and does not send dependent changes', async () => {
  const store = await openUserStore('sync-conflict');
  const first = await store.enqueue({recordId:'r', action:'update', baseVersion:1, data:{amount:10}});
  const second = await store.enqueue({recordId:'r', action:'update', baseVersion:null, dependsOn:first.id, data:{amount:30}});
  let calls = 0;
  const result = await sync(store, async (url) => {
    if (url === '/api/snapshot') return Response.json(snapshot());
    calls++;
    return Response.json({status:'conflict',operation:{id:'r',version:2,amount:20}}, {status:409});
  });
  assert.equal(result.conflicts,1);
  assert.equal(calls,1);
  const pending = await store.pending();
  assert.equal(pending.find(op=>op.id===first.id).server.amount,20);
  assert.equal(pending.find(op=>op.id===second.id).dependsOn, first.id);
  store.close();
});

test('expired session leaves the local snapshot and queue untouched', async () => {
  const store = await openUserStore('sync-unauthorized');
  await store.replaceSnapshot({accounts:['cached'],categories:[],operations:[]});
  await store.enqueue({recordId:'r',action:'create',baseVersion:null,data:{amount:1}});
  await assert.rejects(sync(store, async () => new Response('',{status:401})), /401/);
  assert.deepEqual((await store.snapshot()).accounts,['cached']);
  assert.equal((await store.pending()).length,1);
  store.close();
});

test('does not send or replace data when the active session belongs to another user', async () => {
  const store = await openUserStore('sync-alice');
  await store.replaceSnapshot({accounts:['alice'], categories:[], operations:[]});
  await store.enqueue({recordId:'r',action:'create',baseVersion:null,data:{amount:1}});
  const calls = [];
  await assert.rejects(syncUser(store, 'sync-alice', async (url) => {
    calls.push(url);
    return Response.json({id:'bob'});
  }, locks), /usuario/i);
  assert.deepEqual(calls, ['/api/me']);
  assert.deepEqual((await store.snapshot()).accounts,['alice']);
  assert.equal((await store.pending()).length,1);
  store.close();
});

test('requires cross-tab locking and a matching store identity', async () => {
  const store = await openUserStore('sync-owner');
  await assert.rejects(syncUser(store, 'someone-else', async()=>{}, locks), /usuario/i);
  await assert.rejects(syncUser(store, 'sync-owner', async()=>{}, null), /bloqueo/i);
  store.close();
});
