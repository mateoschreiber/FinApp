import test from 'node:test';
import assert from 'node:assert/strict';
import { calculateBalances, overlayOperations, checkEditBase, setupStep, monthlyTotals } from '../static/view-model.mjs';

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
