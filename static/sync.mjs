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
  const response = await fetcher('/api/snapshot', {credentials: 'same-origin', cache: 'no-store', headers});
  if (!response.ok) throw httpError(response.status);
  const snapshot = await response.json();
  if (!validSnapshot(snapshot)) throw new Error('Respuesta de sincronización inválida');
  await store.replaceSnapshot(snapshot);

  let applied = 0;
  let conflicts = 0;
  while (true) {
    const pending = await store.pending();
    const item = pending.find(op => op.status === 'pending' && op.dependsOn === null);
    if (!item) break;
    const result = await fetcher('/api/operations', {
      method: 'POST', credentials: 'same-origin',
      headers: {...headers, 'Content-Type': 'application/json'},
      body: JSON.stringify(wireChange(item)),
    });
    if (result.status !== 409 && !result.ok) throw httpError(result.status);
    const payload = await result.json();
    if (result.status === 409 && payload.status === 'conflict') {
      await store.markConflict(item.id, payload.operation);
      conflicts++;
    } else if (result.ok && payload.status === 'applied') {
      await store.acknowledge(item.id, payload.operation);
      applied++;
    } else throw new Error('Respuesta de sincronización inválida');
  }
  return {applied, conflicts};
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
