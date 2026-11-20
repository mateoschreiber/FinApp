// Offline data is partitioned by user. Authentication remains in HttpOnly cookies.
const DB_VERSION = 1;
const ACTIONS = new Set(['create', 'update', 'delete']);

function request(req) {
  return new Promise((resolve, reject) => {
    req.onsuccess = () => resolve(req.result);
    req.onerror = () => reject(req.error);
  });
}

function transactionDone(tx) {
  return new Promise((resolve, reject) => {
    tx.oncomplete = () => resolve();
    tx.onabort = () => reject(tx.error || new Error('Transaction aborted'));
    tx.onerror = () => {}; // The abort event provides the final error.
  });
}

function versionValid(value) {
  return Number.isSafeInteger(value) && value >= 0;
}

function descendants(operations, id) {
  const found = new Set([id]);
  let changed = true;
  while (changed) {
    changed = false;
    for (const op of operations) {
      if (found.has(op.dependsOn) && !found.has(op.id)) {
        found.add(op.id);
        changed = true;
      }
    }
  }
  found.delete(id);
  return found;
}

function validOperation(op) {
  if (!op || typeof op.recordId !== 'string' || !op.recordId || !ACTIONS.has(op.action)) {
    throw new Error('Invalid operation');
  }
  if (op.action === 'delete' ? op.data !== null : !op.data || typeof op.data !== 'object' || Array.isArray(op.data)) {
    throw new Error('Invalid operation data');
  }
  if (op.dependsOn != null && (typeof op.dependsOn !== 'string' || !op.dependsOn)) {
    throw new Error('Invalid dependency');
  }
  if (op.action === 'create' && (op.baseVersion !== null || op.dependsOn != null)) {
    throw new Error('Invalid create version');
  }
  if (op.action !== 'create' && op.dependsOn == null && !versionValid(op.baseVersion)) {
    throw new Error('Invalid base version');
  }
  if (op.dependsOn != null && op.baseVersion !== null) throw new Error('Invalid dependency version');
}

export async function openUserStore(userId) {
  if (typeof userId !== 'string' || !userId) throw new Error('Invalid user ID');
  const open = indexedDB.open(`misfinanzas:v1:${encodeURIComponent(userId)}`, DB_VERSION);
  open.onupgradeneeded = () => {
    open.result.createObjectStore('snapshot');
    open.result.createObjectStore('operations', { keyPath: 'id' });
  };
  const db = await request(open);

  async function run(names, mode, callback) {
    const tx = db.transaction(names, mode);
    const done = transactionDone(tx);
    try {
      const result = await callback(tx);
      await done;
      return result;
    } catch (error) {
      try { tx.abort(); } catch { /* Already complete or aborted. */ }
      await done.catch(() => {});
      throw error;
    }
  }

  return {
    get userId() { return userId; },
    close() { db.close(); },
    snapshot() {
      return run(['snapshot'], 'readonly', tx => request(tx.objectStore('snapshot').get('current')));
    },
    snapshotCursor() {
      return run(['snapshot'], 'readonly', tx => request(tx.objectStore('snapshot').get('cursor')));
    },
    replaceSnapshot(value) {
      return run(['snapshot'], 'readwrite', async tx => {
        const snapshots = tx.objectStore('snapshot');
        await request(snapshots.put(value, 'current'));
        if (Number.isSafeInteger(value?.cursor) && typeof value.epoch === 'string') {
          await request(snapshots.put({cursor:value.cursor, epoch:value.epoch}, 'cursor'));
        } else await request(snapshots.delete('cursor'));
      });
    },
    pending() {
      return run(['operations'], 'readonly', tx => request(tx.objectStore('operations').getAll()));
    },
    async enqueue(input) {
      validOperation(input);
      return run(['operations'], 'readwrite', async tx => {
        const store = tx.objectStore('operations');
        const operations = await request(store.getAll());
        const sameRecord = operations.filter(op => op.recordId === input.recordId);
        if (sameRecord.length) {
          const predecessor = sameRecord.find(op => op.id === input.dependsOn);
          if (!predecessor || predecessor.status !== 'pending' || sameRecord.some(op => op.dependsOn === predecessor.id)) {
            throw new Error('Invalid dependency or unresolved conflict');
          }
        } else if (input.dependsOn != null) throw new Error('Invalid dependency');
        const op = {
          id: crypto.randomUUID(), recordId: input.recordId, action: input.action,
          baseVersion: input.baseVersion, data: input.data, dependsOn: input.dependsOn ?? null,
          status: 'pending',
        };
        await request(store.add(op));
        return op;
      });
    },
    acknowledge(id, serverOperation) {
      if (!serverOperation || typeof serverOperation.id !== 'string' || !versionValid(serverOperation.version)) throw new Error('Invalid server operation');
      return run(['operations', 'snapshot'], 'readwrite', async tx => {
        const store = tx.objectStore('operations');
        const current = await request(store.get(id));
        if (!current) throw new Error('Operation not found');
        if (current.status !== 'pending' || current.dependsOn !== null) throw new Error('Operation is not ready');
        if (serverOperation.id !== current.recordId) throw new Error('Mismatched server operation');
        const snapshotStore = tx.objectStore('snapshot');
        const snapshot = await request(snapshotStore.get('current'));
        if (!snapshot || !Array.isArray(snapshot.operations)) throw new Error('Snapshot unavailable');
        const index = snapshot.operations.findIndex(op => op.id === serverOperation.id);
        if (index >= 0) {
          if (snapshot.operations[index].version <= serverOperation.version) snapshot.operations[index] = serverOperation;
        } else snapshot.operations.push(serverOperation);
        await request(snapshotStore.put(snapshot, 'current'));
        const operations = await request(store.getAll());
        for (const op of operations) {
          if (op.dependsOn === id) {
            op.dependsOn = null;
            op.baseVersion = serverOperation.version;
            await request(store.put(op));
          }
        }
        await request(store.delete(id));
      });
    },
    markConflict(id, server) {
      if (server !== null && (!server || typeof server.id !== 'string' || !versionValid(server.version))) throw new Error('Invalid server operation');
      return run(['operations'], 'readwrite', async tx => {
        const store = tx.objectStore('operations');
        const op = await request(store.get(id));
        if (!op) throw new Error('Operation not found');
        if (op.status !== 'pending' || op.dependsOn !== null) throw new Error('Operation is not ready');
        if (server !== null && server.id !== op.recordId) throw new Error('Mismatched server operation');
        op.status = 'conflict';
        op.server = server;
        await request(store.put(op));
      });
    },
    markRejected(id, rejection) {
      if (!Number.isInteger(rejection?.status) || typeof rejection.message !== 'string') throw new Error('Invalid rejection');
      return run(['operations'], 'readwrite', async tx => {
        const operations = tx.objectStore('operations');
        const item = await request(operations.get(id));
        if (item?.status !== 'pending' || item.dependsOn !== null) throw new Error('Operation is not ready');
        await request(operations.put({...item, status:'rejected', rejection}));
      });
    },
    retryRejected(id, correction = {}) {
      return run(['operations'], 'readwrite', async tx => {
        const operations = tx.objectStore('operations');
        const item = await request(operations.get(id));
        if (item?.status !== 'rejected') throw new Error('El cambio ya fue resuelto.');
        const replacement = {
          ...item, id: crypto.randomUUID(), status: 'pending',
          data: Object.hasOwn(correction, 'data') ? correction.data : item.data,
        };
        validOperation(replacement);
        delete replacement.rejection;
        const children = (await request(operations.getAll())).filter(child=>child.dependsOn === item.id);
        await request(operations.add(replacement));
        for (const child of children) await request(operations.put({...child, dependsOn:replacement.id}));
        await request(operations.delete(id));
        return replacement;
      });
    },
    resolveConflict(id, resolution, { discardDependents = false } = {}) {
      return run(['operations'], 'readwrite', async tx => {
        const store = tx.objectStore('operations');
        const conflict = await request(store.get(id));
        if (!conflict || conflict.status !== 'conflict') throw new Error('Conflict not found');
        const input = { recordId: conflict.recordId, action: resolution?.action, data: resolution?.data, baseVersion: conflict.server === null ? null : conflict.server.version };
        validOperation(input);
        const op = { ...input, id: crypto.randomUUID(), dependsOn: null, status: 'pending' };
        const operations = await request(store.getAll());
        const dependentIds = descendants(operations, id);
        if (dependentIds.size && discardDependents !== true) throw new Error('Dependent changes require review');
        for (const dependentId of dependentIds) await request(store.delete(dependentId));
        await request(store.add(op));
        await request(store.delete(id));
        return op;
      });
    },
    acceptServer(id, { discardDependents = false } = {}) {
      return run(['operations', 'snapshot'], 'readwrite', async tx => {
        const store = tx.objectStore('operations');
        const conflict = await request(store.get(id));
        if (!conflict || conflict.status !== 'conflict') throw new Error('Conflict not found');
        const operations = await request(store.getAll());
        const dependents = operations.filter(op => op.dependsOn === id);
        if (dependents.length && discardDependents !== true) throw new Error('Dependent changes require review');
        const snapshots = tx.objectStore('snapshot');
        const snapshot = await request(snapshots.get('current'));
        if (!snapshot || !Array.isArray(snapshot.operations)) throw new Error('Snapshot unavailable');
        const cached = snapshot.operations.find(op=>op.id === conflict.recordId);
        if (!cached || (conflict.server && cached.version <= conflict.server.version)) {
          snapshot.operations = snapshot.operations.filter(op => op.id !== conflict.recordId);
          if (conflict.server !== null) snapshot.operations.push(conflict.server);
        } else if (!conflict.server && cached.version === conflict.baseVersion) {
          snapshot.operations = snapshot.operations.filter(op => op.id !== conflict.recordId);
        }
        await request(snapshots.put(snapshot, 'current'));
        for (const dependentId of descendants(operations, id)) await request(store.delete(dependentId));
        await request(store.delete(id));
      });
    },
    discardOperation(id, { discardDependents = false } = {}) {
      return run(['operations'], 'readwrite', async tx => {
        const operations = tx.objectStore('operations');
        if (!await request(operations.get(id))) throw new Error('Operation not found');
        const dependentIds = descendants(await request(operations.getAll()), id);
        if (dependentIds.size && discardDependents !== true) throw new Error('Dependent changes require review');
        for (const dependentId of dependentIds) await request(operations.delete(dependentId));
        await request(operations.delete(id));
      });
    },
    clear({ discardPending = false } = {}) {
      return run(['snapshot', 'operations'], 'readwrite', async tx => {
        const operations = tx.objectStore('operations');
        if (discardPending !== true && await request(operations.count()) > 0) throw new Error('Pending operations exist');
        await request(operations.clear());
        await request(tx.objectStore('snapshot').clear());
      });
    },
  };
}
