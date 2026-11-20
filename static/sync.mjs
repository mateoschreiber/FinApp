// A synchronization run never deletes pending changes until the server confirms them.
// Sessions are managed by same-origin HttpOnly cookies, never by this module.
const running = new WeakMap();

function httpError(status) {
  const error = new Error(`La sincronización falló (HTTP ${status})`);
  error.status = status;
  return error;
}

function validSnapshot(value) {
  return value && Array.isArray(value.accounts) && Array.isArray(value.categories)
    && Array.isArray(value.operations);
}

function wireChange(item) {
  const data = item.data || {};
  return {
    request_id: item.id,
    id: item.recordId,
    expected_version: item.action === 'create' ? 0 : item.baseVersion,
    deleted: item.action === 'delete',
    kind: data.kind ?? null,
    account_id: data.account_id ?? null,
    category_id: data.category_id ?? null,
    card_account_id: data.card_account_id ?? null,
    destination_account_id: data.destination_account_id ?? null,
    original_operation_id: data.original_operation_id ?? null,
    amount: data.amount ?? null,
    date: data.date ?? null,
    description: data.description ?? null,
    notes: data.notes ?? null,
  };
}

async function runSync(store, userId, fetcher) {
  const headers = {'X-Finapp-User': userId};
  const who = await fetcher('/api/me', {credentials: 'same-origin', cache: 'no-store'});
  if (!who.ok) throw httpError(who.status);
  const identity = await who.json();
  if (identity?.id !== userId) throw new Error('La sesión pertenece a otro usuario');
  const priorCursor = await store.snapshotCursor();
  const cursorValid = Number.isSafeInteger(priorCursor?.cursor) && priorCursor.cursor >= 0 && typeof priorCursor.epoch === 'string';
  const path = cursorValid ? '/api/snapshot?' + new URLSearchParams({since: priorCursor.cursor, epoch: priorCursor.epoch}) : '/api/snapshot';
  const response = await fetcher(path, {credentials: 'same-origin', cache: 'no-store', headers});
  if (!response.ok) throw httpError(response.status);
  const snapshot = await response.json();
  if (!validSnapshot(snapshot)) throw new Error('Respuesta de sincronización inválida');
  let writeSnapshot = true;
  if (snapshot.incremental) {
    if (!cursorValid || snapshot.epoch !== priorCursor.epoch || !Number.isSafeInteger(snapshot.cursor) || snapshot.cursor < priorCursor.cursor) throw new Error('Cursor de sincronización inválido');
    const groups = ['accounts','categories','operations'];
    writeSnapshot = snapshot.cursor !== priorCursor.cursor || groups.some(key=>snapshot[key].length);
    if (writeSnapshot) {
      const previous = await store.snapshot();
      for (const key of groups) {
        const rows = new Map(previous[key].map(row => [row.id, row]));
        for (const row of snapshot[key]) rows.set(row.id, row);
        snapshot[key] = [...rows.values()];
        if (key !== 'operations') snapshot[key].sort((a,b) => (a.name||'').localeCompare(b.name||'','es') || a.id.localeCompare(b.id));
      }
      const detailKey = row => JSON.stringify([row.entity,row.id]);
      const details = new Map((previous.catalog_details || []).map(row => [detailKey(row),row]));
      for (const row of snapshot.catalog_details || []) details.set(detailKey(row),row);
      snapshot.catalog_details = [...details.values()];
    }
  }
  if (writeSnapshot) await store.replaceSnapshot(snapshot);

  let applied = 0;
  let conflicts = 0;
  let rejected = 0;
  while (true) {
    const pending = await store.pending();
    const item = pending.find(op => op.status === 'pending' && op.dependsOn === null);
    if (!item) break;
    const result = await fetcher('/api/operations', {
      method: 'POST', credentials: 'same-origin',
      headers: {...headers, 'Content-Type': 'application/json'},
      body: JSON.stringify(wireChange(item)),
    });
    if (result.status === 409 || [400,413,422].includes(result.status)) {
      const payload = await result.json().catch(()=>null);
      if (result.status === 409 && payload?.status === 'conflict') {
        await store.markConflict(item.id, payload.operation);
        conflicts++;
      } else {
        const message = typeof payload?.message === 'string' ? payload.message.slice(0,500) :
          'El servidor rechazó este movimiento (HTTP '+result.status+'). Revisa los datos antes de reintentar.';
        await store.markRejected(item.id,{status:result.status,message});
        rejected++;
      }
      continue;
    }
    if (!result.ok) throw httpError(result.status);
    const payload = await result.json();
    if (payload.status !== 'applied') throw new Error('Respuesta de sincronización inválida');
    await store.acknowledge(item.id, payload.operation);
    applied++;
  }
  return {applied, conflicts, rejected};
}

export function syncUser(store, expectedUserId, fetcher = globalThis.fetch, locks = globalThis.navigator?.locks) {
  if (!expectedUserId || store.userId !== expectedUserId) return Promise.reject(new Error('Usuario local incorrecto'));
  if (!locks?.request) return Promise.reject(new Error('El navegador no admite bloqueo entre pestañas'));
  if (running.has(store)) return running.get(store);
  const promise = locks.request(`misfinanzas-sync:${expectedUserId}`, () => runSync(store, expectedUserId, fetcher))
    .finally(() => running.delete(store));
  running.set(store, promise);
  return promise;
}
