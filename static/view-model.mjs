// All amounts must remain exact integers in the browser.
export function checkEditBase(openedTail, currentTail) {
  if (openedTail !== currentTail) throw new Error('Este movimiento cambió mientras lo editabas. Ábrelo de nuevo para revisar los cambios.');
}

export function calculateBalances(accounts, operations) {
  const values = new Map(accounts.map(a => {
    if (!Number.isSafeInteger(a.initial_balance)) throw new Error('Saldo inicial inválido');
    return [a.id, BigInt(a.initial_balance)];
  }));
  const kinds = new Map(accounts.map(a => [a.id, a.kind]));
  function change(id, delta) {
    if (!values.has(id)) throw new Error('La operación hace referencia a una cuenta inexistente');
    values.set(id, values.get(id) + BigInt(delta));
  }
  for (const op of operations) {
    if (op.deleted) continue;
    if (!Number.isSafeInteger(op.amount) || op.amount <= 0) throw new Error('Importe inválido');
    if (op.kind === 'payment') {
      change(op.account_id, -op.amount);
      change(op.card_account_id, -op.amount);
    } else if (op.kind === 'income' || op.kind === 'expense') {
      const sign = op.kind === 'income' ? 1 : -1;
      change(op.account_id, kinds.get(op.account_id) === 'card' ? -sign * op.amount : sign * op.amount);
    } else throw new Error('Tipo de movimiento inválido');
  }
  return new Map([...values].map(([id, value]) => {
    if (value > BigInt(Number.MAX_SAFE_INTEGER) || value < BigInt(Number.MIN_SAFE_INTEGER)) throw new Error('El saldo excede el límite admitido');
    return [id, Number(value)];
  }));
}

export function overlayOperations(operations, pending) {
  const values = new Map(operations.map(op => [op.id, {...op}]));
  const remaining = [...pending];
  const processed = new Set();
  while (remaining.length) {
    const index = remaining.findIndex(op => !op.dependsOn || processed.has(op.dependsOn));
    if (index < 0) throw new Error('La cola contiene dependencias inválidas');
    const [op] = remaining.splice(index, 1);
    const old = values.get(op.recordId) || {id: op.recordId};
    const next = op.action === 'delete' ? {...old, deleted:true} : {...old, ...op.data};
    values.set(op.recordId, {...next, pending:true, conflict:op.status === 'conflict' || old.conflict === true});
    processed.add(op.id);
  }
  return [...values.values()];
}
