import test from 'node:test';
import assert from 'node:assert/strict';
import { calculateBalances, overlayOperations, checkEditBase, setupStep, monthlyTotals, netWorth } from '../static/view-model.mjs';
import { confirmAction } from '../static/ui.mjs';

test('los totales del mes siguen la semántica del informe del servidor', () => {
  const operations = [
    {kind:'income',amount:3000000,date:'2026-11-05'},
    {kind:'expense',amount:150000,date:'2026-11-08'},
    {kind:'refund',amount:50000,date:'2026-11-09'},
    {kind:'transfer',amount:300000,date:'2026-11-16'},
    {kind:'payment',amount:200000,date:'2026-11-12'},
    {kind:'expense',amount:90000,date:'2026-10-31'},
    {kind:'income',amount:700000,date:'2026-11-30',deleted:true},
  ];
  assert.deepEqual(monthlyTotals(operations,'2026-11'),{income:3000000n,expense:100000n,net:2900000n});
  assert.deepEqual(monthlyTotals(operations,'2026-10'),{income:0n,expense:90000n,net:-90000n});
  assert.deepEqual(monthlyTotals([],'2026-11'),{income:0n,expense:0n,net:0n});
});

test('guía la configuración inicial sin volver a pedir datos existentes', () => {
  assert.equal(setupStep({accounts: [], categories: []}), 'account');
  assert.equal(setupStep({accounts: [{id:'a'}], categories: []}), 'category');
  assert.equal(setupStep({accounts: [{id:'a'}], categories: [{id:'c',kind:'income'}]}), 'category');
  assert.equal(setupStep({accounts: [{id:'a'}], categories: [{id:'c',kind:'expense'}]}), 'ready');
});

test('pago de tarjeta reduce ambos saldos sin ser gasto adicional', () => {
  const accounts = [{id:'a',kind:'bank',initial_balance:100000},{id:'c',kind:'card',initial_balance:40000}];
  const result = calculateBalances(accounts,[{kind:'payment',account_id:'a',card_account_id:'c',amount:15000}]);
  assert.equal(result.get('a'),85000);
  assert.equal(result.get('c'),25000);
});

test('acepta saldo final exacto aunque la suma intermedia supere el límite numérico', () => {
  const max = Number.MAX_SAFE_INTEGER;
  const result = calculateBalances([{id:'a',kind:'cash',initial_balance:max}], [
    {kind:'income',account_id:'a',amount:1}, {kind:'expense',account_id:'a',amount:1}
  ]);
  assert.equal(result.get('a'),max);
});

test('una edición pendiente reemplaza la operación y se marca como pendiente', () => {
  const existing = [{id:'m',version:2,kind:'expense',amount:100}];
  const pending = [{id:'q',recordId:'m',action:'update',data:{kind:'expense',amount:150},status:'pending'}];
  assert.deepEqual(overlayOperations(existing,pending),[{id:'m',version:2,kind:'expense',amount:150,pending:true,conflict:false}]);
});

test('una operación en conflicto conserva la versión del servidor sin presentarla como definitiva', () => {
  const pending = [{id:'q',recordId:'m',action:'update',data:{kind:'expense',amount:150},status:'conflict'}];
  assert.equal(overlayOperations([{id:'m',version:2,kind:'expense',amount:100}],pending)[0].conflict,true);
});

test('los cambios dependientes se aplican en orden aunque sus identificadores estén desordenados', () => {
  const pending = [
    {id:'a',recordId:'m',action:'update',dependsOn:'z',data:{kind:'expense',amount:200},status:'pending'},
    {id:'z',recordId:'m',action:'create',dependsOn:null,data:{kind:'expense',amount:100},status:'pending'},
  ];
  assert.equal(overlayOperations([],pending)[0].amount,200);
});

test('impide guardar una edición si cambió la cola desde que se abrió el formulario', () => {
  assert.throws(()=>checkEditBase('previo', 'nuevo'), /cambió/);
  assert.doesNotThrow(()=>checkEditBase('previo','previo'));
  assert.throws(()=>checkEditBase(null,'nuevo'), /cambió/);
});

test('el patrimonio resta la deuda de tarjeta como el informe del servidor', () => {
  const accounts = [{id:'cash',kind:'cash'},{id:'bank',kind:'bank'},{id:'card',kind:'card'}];
  const balances = new Map([['cash',1000000],['bank',0],['card',300000]]);
  assert.equal(netWorth(accounts, balances), 700000n);
  assert.equal(netWorth([{id:'card',kind:'card'}], new Map([['card',-300000]])), 300000n);
  assert.equal(netWorth([], new Map()), 0n);
});

test('confirmAction usa el dialog nativo, resetea returnValue al cerrar y cae en confirm si no hay DOM', async () => {
  let closeHandler = null;
  const messageEl = { textContent: '' };
  const acceptEl = { textContent: '' };
  const dialog = {
    returnValue: 'stale',
    opened: 0,
    showModal() { this.opened++; },
    addEventListener(type, fn) { if (type === 'close') closeHandler = fn; },
  };
  const prevDoc = globalThis.document;
  globalThis.document = {
    getElementById(id) {
      if (id === 'confirm-dialog') return dialog;
      if (id === 'confirm-message') return messageEl;
      if (id === 'confirm-accept') return acceptEl;
      return null;
    },
  };
  try {
    const first = confirmAction('¿Eliminar movimiento?', 'Eliminar');
    assert.equal(dialog.opened, 1);
    assert.equal(messageEl.textContent, '¿Eliminar movimiento?');
    assert.equal(acceptEl.textContent, 'Eliminar');
    assert.equal(dialog.returnValue, '');
    dialog.returnValue = 'confirm';
    closeHandler();
    assert.equal(await first, true);
    assert.equal(dialog.returnValue, '', 'returnValue se limpia tras cerrar para que Esc no herede confirm');

    const second = confirmAction('¿Cancelar?');
    closeHandler();
    assert.equal(await second, false);
  } finally {
    globalThis.document = prevDoc;
  }
});
