import test from 'node:test';
import assert from 'node:assert/strict';
import { calculateBalances, overlayOperations, checkEditBase } from '../static/view-model.mjs';

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
