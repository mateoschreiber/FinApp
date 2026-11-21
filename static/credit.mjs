import {element as el, money, field, lastIdentity} from './ui.mjs';

// Dates are stored as civil dates in Paraguay. UTC arithmetic avoids DST shifts.
function dayOfMonth(month, day, offset = 0) {
  const [y, m] = month.split('-').map(Number);
  const date = new Date(0);
  date.setUTCFullYear(y, m - 1 + offset + 1, 0);
  const clipped = Math.min(day, date.getUTCDate());
  date.setUTCDate(clipped);
  return date.toISOString().slice(0, 10);
}
function safeNumber(amount) {
  const value = Number(amount);
  if (!Number.isSafeInteger(value)) throw new Error('El total supera el importe admitido.');
  return value;
}
// Amounts still payable at the end of the selected month, using the original
// contractual projection. Allocate actual capital and interest separately, FIFO.
export function creditProjection(view, portfolio, month) {
  const items=[];
  const end=dayOfMonth(month,31);
  const previous=dayOfMonth(month,1,-1).slice(0,7);
  const confirmed=view.operations.filter(o=>!o.pending && !o.conflict && o.date<=end);
  for (const card of view.accounts.filter(a=>a.kind==='card')) {
    for (const cycle of [previous,month]) {
      const statement=cardStatement(card,cycle,confirmed,portfolio);
      if (statement?.due.slice(0,7)!==month || statement.outstanding<=0) continue;
      items.push({source:'card',id:card.id,name:card.name,date:statement.due,principal:statement.outstanding,interest:0,amount:statement.outstanding});
    }
  }
  for (const loan of portfolio.loans) {
    if (loan.draw_date>end) continue;
    let capitalPaid=0n, interestPaid=0n, settled=null;
    for (const p of [...loan.payments].sort((a,b)=>a.date.localeCompare(b.date))) {
      if (p.date>end) break;
      capitalPaid+=BigInt(p.principal); interestPaid+=BigInt(p.interest);
      if (!settled && capitalPaid===BigInt(loan.principal)) settled=p.date;
    }
    for (const due of loan.installments) {
      if (due.date>end || (settled && due.date>settled)) break;
      const capital=BigInt(due.principal), interest=BigInt(due.interest);
      const paidCapital=capital<capitalPaid?capital:capitalPaid;
      const paidInterest=interest<interestPaid?interest:interestPaid;
      capitalPaid-=paidCapital; interestPaid-=paidInterest;
      const amount=capital+interest-paidCapital-paidInterest;
      if (due.date.slice(0,7)===month && amount>0n) {
        items.push({source:'loan',id:loan.id,name:loan.name,date:due.date,principal:safeNumber(capital-paidCapital),interest:safeNumber(interest-paidInterest),amount:safeNumber(amount)});
      }
    }
  }
  items.sort((a,b)=>a.date.localeCompare(b.date)||a.name.localeCompare(b.name));
  return {items,total:safeNumber(items.reduce((n,i)=>n+BigInt(i.amount),0n))};
}
export function cardStatement(account, month, operations, portfolio) {
  if (!/^\d{4}-(0[1-9]|1[0-2])$/.test(month) || !account.closing_day || !account.due_day) return null;
  const closing = dayOfMonth(month, account.closing_day);
  const previous = dayOfMonth(month, account.closing_day, -1);
  const sameMonthDue = dayOfMonth(month, account.due_day);
  const due = sameMonthDue > closing ? sameMonthDue : dayOfMonth(month, account.due_day, 1);
  const financed = new Map(portfolio.purchases.map(p => [p.operation_id, p.installments]));
  const allocations = new Map(portfolio.allocations.filter(a => a.month === month).map(a => [a.operation_id, a]));
  let charges = 0n, paid = 0n;
  for (const op of operations) {
    if (op.deleted) continue;
    const allocation = allocations.get(op.id);
    if (op.kind === 'payment' && op.card_account_id === account.id && allocation && allocation.payment_version === op.version) paid += BigInt(allocation.amount);
    if (op.account_id !== account.id) continue;
    if (op.kind === 'expense' && financed.has(op.id)) {
      for (const part of financed.get(op.id)) {
        if (part.date > previous && part.date <= closing) charges += BigInt(part.principal);
      }
    } else if (op.date > previous && op.date <= closing) {
      if (op.kind === 'expense') charges += BigInt(op.amount);
      else if (op.kind === 'income' || op.kind === 'refund') charges -= BigInt(op.amount);
    }
  }
  // An imported card with a cutoff date carries its opening debt; charge it in the
  // one cycle that contains the cutoff, where the server's balance math starts.
  let opening = 0n;
  if (account.cutoff_date && account.cutoff_date > previous && account.cutoff_date <= closing) {
    // Backend: a positive card balance is a debt, a negative one is a credit.
    const balance = BigInt(account.initial_balance);
    charges += balance;
    if (balance > 0n) opening = balance;
  }
  return {closing, due, charges:safeNumber(charges), paid:safeNumber(paid), outstanding:safeNumber(charges-paid), opening:safeNumber(opening)};
}

function section(parent, title) {
  const details=el('details');
  details.append(el('summary',title));
  parent.append(details);
  return details;
}
function number(form, label, name, min = 1, value = '', max = Number.MAX_SAFE_INTEGER) {
  return field(form,label,name,'number',{min,max,step:1,value});
}
function options(items) { return items.map(i=>[i.id,i.name]); }
function scheduleList(parent, installments, loan = null) {
  const list=el('ol');
  for (const i of installments) {
    const amount = loan ? 'Capital ' + money(i.principal) + ' · Interés ' + money(i.interest) : money(i.principal);
    list.append(el('li',i.date+' · '+amount));
  }
  parent.append(list);
}

const renderToken = new WeakMap();
export async function renderCredit(container, ctx) {
  const {api, view, today, userId, synchronize}=ctx;
  const token={};
  renderToken.set(container,token);
  container.replaceChildren(el('h2','Tarjetas y préstamos'),el('p','Cargando créditos…'));
  const current=()=>renderToken.get(container)===token && (!ctx.isCurrent || ctx.isCurrent());
  let book;
  try { book=await api('/api/credit'); }
  catch (error) {
    if (current()) container.lastChild.textContent=navigator.onLine ? error.message : 'Conéctate para consultar créditos.';
    return;
  }
  if (!current()) return;
  container.lastChild.remove();
  const cards=view.accounts.filter(a=>a.kind==='card');
  const accounts=view.accounts.filter(a=>a.kind!=='card' && !a.archived);
  const availableCards=cards.filter(a=>!a.archived);
  const expenses=view.categories.filter(c=>c.kind==='expense' && !c.archived);
  const activeOps=view.operations.filter(o=>!o.deleted && !o.pending && !o.conflict);
  const categoryItems=options(expenses);
  const status=el('p');status.setAttribute('role','status');container.append(status);
  function form(parent, label, read) {
    const form=el('form'), submit=el('button',label);
    let last=null;
    form.append(submit);parent.append(form);
    form.addEventListener('submit',async ev=>{
      ev.preventDefault();
      if (!current() || submit.disabled) return;
      submit.disabled=true;
      try {
        const identity=lastIdentity();
        if (!identity || identity.id!==userId) throw new Error('La sesión cambió. Vuelve a abrir Créditos.');
        const entry=read(Object.fromEntries(new FormData(form)));
        const key=JSON.stringify(entry);
        if (last?.key!==key) last={key,request_id:crypto.randomUUID()};
        await api('/api/credit','POST',{request_id:last.request_id,entry});
        status.textContent='Guardado.';
        await synchronize();
        if (current()) await renderCredit(container,{...ctx,view:ctx.getView?.()??view});
      } catch (error) {
        if (current()) status.textContent=error.status===400 ? 'Revisa las fechas, el saldo de capital y los importes del formulario.' : error.message;
      } finally {submit.disabled=false;}
    });
    return form;
  }
  function end(form) {form.append(form.querySelector('button'));}
  // One-click rectifications: same identity and status handling as form(), plus a
  // confirmation because both actions are destructive.
  function action(parent, label, question, build) {
    const button=el('button',label);button.type='button';button.className='secondary';parent.append(button);
    let last=null;
    button.addEventListener('click',async()=>{
      if (!current() || button.disabled) return;
      if (!confirm(question)) return;
      button.disabled=true;
      try {
        const identity=lastIdentity();
        if (!identity || identity.id!==userId) throw new Error('La sesión cambió. Vuelve a abrir Créditos.');
        const entry=build();
        const key=JSON.stringify(entry);
        if (last?.key!==key) last={key,request_id:crypto.randomUUID()};
        await api('/api/credit','POST',{request_id:last.request_id,entry});
        status.textContent='Anulado.';
        await synchronize();
        if (current()) await renderCredit(container,{...ctx,view:ctx.getView?.()??view});
      } catch (error) {
        if (current()) status.textContent=error.status===400 ? 'No se pudo anular: hay movimientos que dependen de él, por ejemplo una devolución.' : error.status===404 ? 'Ya fue anulado o no existe. Volvé a cargar la sección.' : error.message;
      } finally { button.disabled=false; }
    });
  }
  const statement=section(container,'Resumen de tarjeta');
  statement.open=true;
  if (view.operations.some(o=>o.pending || o.conflict)) statement.append(el('p','Sincroniza los cambios pendientes para incluirlos en el resumen.'));
  if (cards.length) {
    const cardInput=field(statement,'Tarjeta','card','select',{items:options(cards)});
    const month=field(statement,'Mes de cierre','month','month',{value:today.slice(0,7)});
    const output=el('p');statement.append(output);
    function show() {
      try {
        const card=cards.find(a=>a.id===cardInput.value);
        const value=cardStatement(card,month.value,activeOps,book);
        output.textContent=value ? 'Cierre '+value.closing+' · Vencimiento '+value.due+' · Cargos del ciclo '+money(value.charges)+(value.opening ? ' (incluye deuda inicial '+money(value.opening)+')' : '')+' · Pagos asociados '+money(value.paid)+' · Diferencia '+money(value.outstanding)
          : 'Configura el día de cierre y el día de vencimiento de la tarjeta en Cuentas.';
      } catch (error) {output.textContent=error.message;}
    }
    cardInput.onchange=month.onchange=show;show();
  } else statement.append(el('p','Crea una cuenta de tipo Tarjeta de crédito en Configuración.'));
  const paymentOps=activeOps.filter(o=>o.kind==='payment');
  if(paymentOps.length) {
    const panel=section(statement,'Asociar un pago registrado a un resumen');
    const f=form(panel,'Guardar importe asociado',v=>({kind:'allocate',operation_id:v.operation_id,expected_version:paymentOps.find(o=>o.id===v.operation_id).version,month:v.month,amount:Number(v.amount)}));
    const paymentOptions=paymentOps.map(o=>[o.id,o.date+' · '+(cards.find(c=>c.id===o.card_account_id)?.name??'Tarjeta')+' · '+money(o.amount)]);
    const payment=field(f,'Pago de tarjeta','operation_id','select',{items:paymentOptions});
    const month=field(f,'Mes de cierre del resumen','month','month',{value:today.slice(0,7)});
    const amount=number(f,'Importe de este pago asignado al mes (Gs.)','amount',0);
    const info=el('p'); f.append(info);
    function assignment() {
      const op=paymentOps.find(o=>o.id===payment.value);
      const assigned=book.allocations.filter(a=>a.operation_id===op.id && a.payment_version===op.version);
      const available=op.amount-assigned.filter(a=>a.month!==month.value).reduce((a,p)=>a+p.amount,0);
      amount.max=available; amount.value=assigned.find(a=>a.month===month.value)?.amount ?? available;
      info.textContent=assigned.length ? 'Importes asociados: '+assigned.map(a=>a.month+' · '+money(a.amount)).join(', ')+'. Usá cero para quitar la asociación de ese mes.' : 'Puedes distribuir este pago entre varios resúmenes.';
    }
    month.onchange=payment.onchange=assignment;assignment();end(f);
  }
  const purchases=section(container,'Compras en cuotas');
  if (availableCards.length && expenses.length) {
    const f=form(purchases,'Registrar compra',v=>({kind:'purchase',name:v.name,account_id:v.account_id,category_id:v.category_id,amount:Number(v.amount),date:v.date,first_due:v.first_due,periods:Number(v.periods)}));
    field(f,'Descripción de la compra','name','text',{maxLength:160});
    field(f,'Tarjeta','account_id','select',{items:options(availableCards)});
    field(f,'Categoría del gasto','category_id','select',{items:categoryItems});
    number(f,'Importe total de la compra (Gs.)','amount');
    field(f,'Fecha de compra','date','date',{value:today});
    field(f,'Primera facturación','first_due','date',{value:today});
    number(f,'Cantidad de cuotas mensuales','periods',1,2,600);end(f);
  } else purchases.append(el('p','Para registrar una compra financiada, crea una tarjeta y una categoría de gasto.'));
  for(const purchase of book.purchases) {
    const op=view.operations.find(o=>o.id===purchase.operation_id);
    const panel=section(purchases,(op?.description??'Compra financiada')+' · '+money(purchase.installments.reduce((a,i)=>a+i.principal,0)));
    scheduleList(panel,purchase.installments);
    if (op && !op.deleted) {
      action(panel,'Anular compra','Se quita el gasto en cuotas y su proyección del resumen de tarjeta. ¿Continuar?',()=>({kind:'cancel_purchase',operation_id:purchase.operation_id}));
      panel.append(el('p','Para corregir los datos de la compra, anulá y volvé a registrarla.'));
    }
  }
  const newLoan=section(container,'Registrar préstamo y desembolso');
  if (accounts.length) {
    const f=form(newLoan,'Registrar desembolso',v=>({kind:'loan',name:v.name,account_id:v.account_id,principal:Number(v.principal),date:v.date,first_due:v.first_due,periods:Number(v.periods),method:v.method,rate_bps:Math.round(Number(v.rate)*100),rate_type:v.rate_type}));
    field(f,'Nombre o entidad','name','text',{maxLength:160});
    field(f,'Cuenta que recibe el dinero','account_id','select',{items:options(accounts)});
    number(f,'Capital desembolsado (Gs.)','principal');
    field(f,'Fecha de desembolso','date','date',{value:today});
    field(f,'Primer vencimiento','first_due','date',{value:today});
    number(f,'Cantidad de cuotas mensuales','periods',1,12,600);
    field(f,'Sistema de amortización','method','select',{items:[['french','Francés (cuota constante)'],['german','Alemán (capital constante)']]});
    field(f,'Tasa anual (%)','rate','number',{min:0,max:1000,step:0.01,value:0});
    field(f,'Tipo de tasa anual','rate_type','select',{items:[['nominal','Nominal'],['effective','Efectiva']]});end(f);
  } else newLoan.append(el('p','Crea una cuenta de efectivo, banco o billetera para registrar el desembolso.'));
  container.append(el('h3','Mis préstamos'));
  if (!book.loans.length) container.append(el('p','Todavía no hay préstamos.'));
  for(const loan of book.loans) {
    const panel=section(container,loan.name+' · Capital pendiente '+money(loan.outstanding));
    panel.append(el('p','Desembolso '+loan.draw_date+' · '+money(loan.principal)));
    scheduleList(section(panel,'Proyección mensual original'),loan.installments,loan);
    if(loan.payments.length) {
      const history=section(panel,'Pagos registrados');
      for(const p of loan.payments) {
        const line=el('p',p.date+' · Capital '+money(p.principal)+' · Interés '+money(p.interest)+' · Comisión '+money(p.fee));
        history.append(line);
        action(line,'Anular pago','Quita este pago y restaura el capital pendiente del préstamo. ¿Continuar?',()=>({kind:'cancel_repayment',repayment_id:p.id}));
      }
    }
    if(accounts.length) {
      const f=form(panel,'Registrar pago',v=>({kind:'repayment',loan_id:loan.id,account_id:v.account_id,date:v.date,principal:Number(v.principal),interest:Number(v.interest),fee:Number(v.fee),category_id:v.category_id||null}));
      field(f,'Cuenta de pago','account_id','select',{items:options(accounts)});
      field(f,'Fecha de pago','date','date',{min:loan.draw_date,value:today});
      number(f,'Capital pagado (Gs.)','principal',0,0,loan.outstanding);
      number(f,'Intereses pagados (Gs.)','interest',0,0);
      number(f,'Comisiones pagadas (Gs.)','fee',0,0);
      field(f,'Categoría de intereses y comisiones','category_id','select',{required:false,items:[['','Solo pago de capital'],...categoryItems]});end(f);
    }
  }
}
