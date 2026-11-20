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

test('un rechazo persiste y permite sincronizar otro movimiento y corregir con dependientes', async () => {
  const user='sync-reject-independent';
  let store=await openUserStore(user);
  const first=await store.enqueue({recordId:'invalid',action:'create',baseVersion:null,data:{amount:0}});
  const child=await store.enqueue({recordId:'invalid',action:'update',dependsOn:first.id,baseVersion:null,data:{amount:15}});
  await store.enqueue({recordId:'valid',action:'create',baseVersion:null,data:{amount:20}});
  const send = async(url,options)=>{
    if (url.startsWith('/api/snapshot')) return Response.json(snapshot());
    const body=JSON.parse(options.body);
    if (body.amount===0) return Response.json({message:'Importe inválido'},{status:400});
    return Response.json({status:'applied',operation:{id:body.id,version:body.expected_version+1,amount:body.amount}});
  };
  assert.deepEqual(await sync(store,send),{applied:1,conflicts:0,rejected:1});
  store.close(); store=await openUserStore(user);
  const rejected=(await store.pending()).find(item=>item.id===first.id);
  assert.equal(rejected.status,'rejected');
  assert.equal(rejected.rejection.message,'Importe inválido');
  assert.deepEqual(await sync(store,send),{applied:0,conflicts:0,rejected:0});
  await assert.rejects(store.discardOperation(first.id),/dependent/i);
  const corrected=await store.retryRejected(first.id,{data:{amount:10}});
  assert.notEqual(corrected.id,first.id);
  assert.equal((await store.pending()).find(item=>item.id===child.id).dependsOn,corrected.id);
  assert.deepEqual(await sync(store,send),{applied:2,conflicts:0,rejected:0});
  assert.deepEqual(await store.pending(),[]);
  store.close();
});

test('la sincronización incremental conserva filas previas, tombstones y cambios locales',async()=>{
  const store=await openUserStore('sync-delta');
  const base={accounts:[{id:'a'}],categories:[],operations:[{id:'x',version:1},{id:'y',version:1}],epoch:'e1',cursor:3};
  await store.replaceSnapshot(base);
  let seen='';
  await sync(store,async url=>{
    seen=url;
    return Response.json({accounts:[],categories:[{id:'c'}],operations:[{id:'x',version:2,deleted:true}],epoch:'e1',cursor:5,incremental:true});
  });
  assert.equal(seen,'/api/snapshot?since=3&epoch=e1');
  const latest=await store.snapshot();
  assert.deepEqual(latest.accounts,base.accounts);
  assert.deepEqual(latest.operations,[{id:'x',version:2,deleted:true},base.operations[1]]);
  await assert.rejects(sync(store,async()=>Response.json({...latest,epoch:'incorrect',cursor:6})),/cursor/i);
  assert.deepEqual(await store.snapshot(),latest);
  store.close();
});

test('una respuesta de sesión o servidor conserva el cambio listo para reintentar',async()=>{
  for (const status of [401,403,429,503]){
    const store=await openUserStore('sync-transient-'+status);
    const item=await store.enqueue({recordId:'r',action:'create',baseVersion:null,data:{amount:10}});
    await assert.rejects(sync(store,async url=>url==='/api/snapshot' ? Response.json(snapshot()) : new Response(null,{status})),new RegExp(String(status)));
    assert.deepEqual(await store.pending(),[item]);
    store.close();
  }
});
