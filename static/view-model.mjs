// All amounts must remain exact integers in the browser.
export function setupStep(snapshot) {
  if (!snapshot.accounts.some(a=>!a.archived)) return 'account';
  if (!snapshot.categories.some(c=>!c.archived && c.kind==='expense')) return 'category';
  return 'ready';
}
export function checkEditBase(openedTail, currentTail) {
  if (openedTail !== currentTail) throw new Error('Este movimiento cambió mientras lo editabas. Ábrelo de nuevo para revisar los cambios.');
}

export function calculateBalances(accounts, operations) {
  const values = new Map(accounts.map(a => {
    if (!Number.isSafeInteger(a.initial_balance)) throw new Error('Saldo inicial inválido');
    return [a.id, BigInt(a.initial_balance)];
  }));
  const catalog = new Map(accounts.map(a => [a.id, a]));
  function change(id, delta, date) {
    if (!values.has(id)) throw new Error('La operación hace referencia a una cuenta inexistente');
    const cutoff = catalog.get(id).cutoff_date;
    if (cutoff && date < cutoff) return;
    values.set(id, values.get(id) + BigInt(delta));
  }
  for (const op of operations) {
    if (op.deleted) continue;
    if (!Number.isSafeInteger(op.amount) || op.amount <= 0) throw new Error('Importe inválido');
    if (op.kind === 'payment') {
      change(op.account_id, -op.amount, op.date);
      change(op.card_account_id, -op.amount, op.date);
    } else if (op.kind === 'transfer') {
      change(op.account_id, -op.amount, op.date);
      change(op.destination_account_id, op.amount, op.date);
    } else if (['income', 'expense', 'refund', 'loan_draw', 'loan_repayment'].includes(op.kind)) {
      const sign = ['income', 'refund', 'loan_draw'].includes(op.kind) ? 1 : -1;
      change(op.account_id, catalog.get(op.account_id)?.kind === 'card' ? -sign * op.amount : sign * op.amount, op.date);
    } else throw new Error('Tipo de movimiento inválido');
  }
  return new Map([...values].map(([id, value]) => {
    if (value > BigInt(Number.MAX_SAFE_INTEGER) || value < BigInt(Number.MIN_SAFE_INTEGER)) throw new Error('El saldo excede el límite admitido');
    return [id, Number(value)];
  }));
}

export function monthlyTotals(operations, month) {
  let income = 0n, expense = 0n;
  for (const op of operations) {
    if (op.deleted || !op.date?.startsWith(month)) continue;
    if (op.kind === 'income') income += BigInt(op.amount);
    else if (op.kind === 'expense') expense += BigInt(op.amount);
    else if (op.kind === 'refund') expense -= BigInt(op.amount);
  }
  return {income, expense, net: income - expense};
}

export function overlayOperations(operations, pending) {
  const values = new Map(operations.map(op => [op.id, {...op}]));
  const remaining = [...pending];
  const processed = new Set();
  const rejected = new Set();
  while (remaining.length) {
    const index = remaining.findIndex(op => !op.dependsOn || processed.has(op.dependsOn));
    if (index < 0) throw new Error('La cola contiene dependencias inválidas');
    const [op] = remaining.splice(index, 1);
    processed.add(op.id);
    if (op.status === 'rejected' || rejected.has(op.dependsOn)) {
      rejected.add(op.id);
      continue;
    }
    const old = values.get(op.recordId) || {id: op.recordId};
    const next = op.action === 'delete' ? {...old, deleted:true} : {...old, ...op.data};
    if (op.action !== 'delete' && next.deleted) next.deleted = false;
    values.set(op.recordId, {...next, pending:true, conflict:op.status === 'conflict' || old.conflict === true});
  }
  return [...values.values()];
}

export function historyPage(operations, filters, requestedPage, pageSize=25) {
  const term = (filters.text || '').trim().toLocaleLowerCase('es');
  const rows = operations.filter(op=>!op.deleted
    && (!filters.from || op.date >= filters.from)
    && (!filters.to || op.date <= filters.to)
    && (!filters.kind || filters.kind === op.kind)
    && (!filters.category || filters.category === op.category_id)
    && (!filters.account || [op.account_id,op.destination_account_id,op.card_account_id].includes(filters.account))
    && (!term || [op.description,op.notes].some(value=>value?.toLocaleLowerCase('es').includes(term))))
    .sort((a,b)=>b.date.localeCompare(a.date) || a.id.localeCompare(b.id));
  const pages = Math.max(1, Math.ceil(rows.length/pageSize));
  const page = Math.max(1,Math.min(requestedPage,pages));
  return {rows: rows.slice((page-1)*pageSize,page*pageSize),total:rows.length,pages,page};
}
