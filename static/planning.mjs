import {creditProjection} from './credit.mjs';

const formatMoney = amount => new Intl.NumberFormat('es-PY').format(BigInt(amount)) + ' Gs.';
const byId = items => new Map(items.map(item => [item.id, item.name]));
const kinds = {income:'Ingreso', expense:'Gasto', payment:'Pago de tarjeta', transfer:'Transferencia', refund:'Devolución'};

function node(tag, text) {
  const el = document.createElement(tag);
  if (text !== undefined) el.textContent = text;
  return el;
}
function field(form, label, name, type, options = {}) {
  const wrap = node('label',label);
  const input = node(type === 'select' ? 'select' : 'input');
  input.name = name;
  if (type === 'select') {
    for (const item of options.items) {
      const opt = node('option',item.name);
      opt.value=item.id;
      input.append(opt);
    }
  } else input.type=type;
  for (const [key,value] of Object.entries(options)) if (key !== 'items') input[key]=value;
  input.required = options.required ?? true;
  wrap.append(input); form.append(wrap);
  return input;
}
function section(root, title) {
  const el=node('section');
  el.append(node('h3',title));
  root.append(el);
  return el;
}
function action(label, callback) {
  const b=node('button',label);
  b.type='button';
  b.addEventListener('click',callback);
  return b;
}
function table(root, headers, rows) {
  if (!rows.length) return;
  const t=node('table'), head=node('thead'), tr=node('tr'), body=node('tbody');
  for (const title of headers) tr.append(node('th',title));
  head.append(tr);
  for (const row of rows) {
    const line=node('tr');
    for (const cell of row) {
      const td=node('td');
      if (cell instanceof Node) td.append(cell); else td.textContent=cell;
      line.append(td);
    }
    body.append(line);
  }
  const scroll=node('div');
  scroll.className='table-scroll';
  t.append(head,body);
  scroll.append(t);
  root.append(scroll);
}
function newForm(root, title, finish) {
  const details=node('details'), summary=node('summary',title), form=node('form');
  details.append(summary,form); root.append(details);
  form.addEventListener('submit',event=>{event.preventDefault();finish(Object.fromEntries(new FormData(form)));});
  return {form, details, end(){form.append(node('button','Guardar'));}};
}
function hexOf(buffer) {
  const chars=Array.from({length:256},(_,i)=>i.toString(16).padStart(2,'0'));
  const bytes=new Uint8Array(buffer), chunks=[];
  for(let start=0;start<bytes.length;start+=8192) chunks.push(Array.from(bytes.subarray(start,start+8192),v=>chars[v]).join(''));
  return chunks.join('');
}
export async function renderPlanning(container, {api, getView, today, isCurrent, synchronize}) {
  const shell=node('div'), heading=node('h2','Planificación e informes'), controls=node('div'), body=node('div');
  const month=field(controls,'Mes','month','month',{value:today.slice(0,7)});
  const status=node('p');
  status.setAttribute('role','status');
  shell.append(heading,controls,status,body); container.replaceChildren(shell);
  let generation=0;
  const live=()=>isCurrent() && shell.parentNode===container;
  const error=e=>{if(live()) status.textContent=e.status===409?'Los datos cambiaron. Actualiza el informe y vuelve a revisarlo.':(e.message || 'No se pudo completar la solicitud.');};
  async function refresh() {
    const current=++generation, period=month.value;
    body.replaceChildren();status.textContent='Consultando movimientos sincronizados…';
    if (!period) {status.textContent='Selecciona un mes.';return;}
    try {
      const [report, credit]=await Promise.all([api('/api/planning/month/'+period), api('/api/credit')]);
      if (!live() || current!==generation) return;
      status.textContent='';
      draw(report, credit, getView());
    } catch(e){if(current===generation)error(e);}
  }
  async function run(fn) {
    if(!live()) return;
    try {await fn();if(live()) await refresh();} catch(e){error(e);}
  }
  function draw(report, credit, view) {
    const accounts=byId(view.accounts), categories=byId(view.categories);
    const liquid=view.accounts.filter(a=>a.kind!=='card' && !a.archived);
    const expenses=view.categories.filter(c=>c.kind==='expense' && !c.archived);
    const save=v=>run(()=>api('/api/planning','POST',v));
    const removePlan=(kind,id)=>action('Quitar',()=>{if(confirm('¿Quitar esta planificación?'))save({kind,id});});
    const totals=section(body,'Resultado del mes');
    table(totals,['Ingresos','Gastos netos','Resultado','Flujo de efectivo'],[
      [report.income,report.expense,report.net,report.cash_flow].map(formatMoney),
    ]);
    totals.append(node('p','Patrimonio al cierre: '+formatMoney(report.net_worth)+'. Capital pendiente de préstamos: '+formatMoney(report.loan_debt)+'.'));
    if ([report.loan_principal,report.loan_interest,report.loan_fees].some(v => v !== '0')) {
      table(totals,['Capital pagado','Intereses pagados','Comisiones'],[[report.loan_principal,report.loan_interest,report.loan_fees].map(formatMoney)]);
    }
    totals.append(node('p',report.reviewed?'Mes revisado.':'Mes pendiente de revisión.'));
    if(!report.reviewed) totals.append(action('Marcar mes como revisado',()=>run(()=>api('/api/planning/month/'+report.month,'POST',{digest:report.digest}))));
    table(totals,['Cuenta','Gasto neto'],Object.entries(report.accounts).map(([id,amount])=>[accounts.get(id)||id,formatMoney(amount)]));
    table(totals,['Categoría','Gasto neto'],Object.entries(report.categories).map(([id,amount])=>[categories.get(id)||id,formatMoney(amount)]));
    const budgets=section(body,'Presupuestos del mes');
    table(budgets,['Categoría','Presupuesto','Gastado','Disponible',''],report.budgets.map(b=>{
      const spent=BigInt(report.categories[b.category_id]||0);
      return [categories.get(b.category_id)||b.category_id,formatMoney(b.amount),formatMoney(spent),formatMoney(BigInt(b.amount)-spent),action('Quitar',()=>{if(confirm('¿Quitar el presupuesto del mes?'))save({kind:'remove_budget',month:report.month,category_id:b.category_id});})];
    }));
    if(expenses.length) {
      const f=newForm(budgets,'Definir o actualizar presupuesto',v=>save({kind:'budget',month:report.month,...v,amount:Number(v.amount)}));
      field(f.form,'Categoría','category_id','select',{items:expenses});
      field(f.form,'Límite en guaraníes','amount','number',{min:1,step:1}); f.end();
    } else budgets.append(node('p','Crea una categoría de gasto en Configuración.'));
    const goals=section(body,'Objetivos de ahorro');
    let goalForm;
    table(goals,['Objetivo','Cuenta','Saldo al cierre','Meta','Mes objetivo',''],report.goals.map(g=>{
      const actions=node('div');
      actions.className='actions';
      if (liquid.length) actions.append(action('Editar',() => openForEdit(goalForm,g)));
      actions.append(removePlan('remove_goal',g.id));
      return [g.name,accounts.get(g.account_id)||g.account_id,formatMoney(report.balances[g.account_id]||0),formatMoney(g.target),g.deadline,actions];
    }));
    if(liquid.length) {
      const f=newForm(goals,'Nuevo objetivo',v=>save({kind:'goal',...v,id:v.id || crypto.randomUUID(),target:Number(v.target)}));
      field(f.form,'','id','hidden',{required:false}).parentElement.hidden=true;
      field(f.form,'Objetivo','name','text',{maxLength:100});field(f.form,'Cuenta de ahorro','account_id','select',{items:liquid});
      field(f.form,'Importe objetivo','target','number',{min:1,step:1});field(f.form,'Mes objetivo','deadline','month',{value:report.month}); f.end();
      goalForm=f;
    }
    const recur=section(body,'Gastos recurrentes y proyección');
    const pending=report.recurring.filter(r=>r.due&&!r.posted);
    const outstanding=pending.reduce((sum,r)=>sum+BigInt(r.amount),0n);
    recur.append(node('p','Recurrentes pendientes del mes: '+formatMoney(outstanding)+'. Resultado previsto al registrarlos: '+formatMoney(BigInt(report.net)-outstanding)+'.'));
    const projection=creditProjection(view,credit,report.month);
    table(recur,['Vencimiento','Concepto','Origen','Importe previsto'],projection.items.map(item=>[
      item.date,item.name,item.source==='card'?'Tarjeta':'Préstamo',formatMoney(item.amount),
    ]));
    recur.append(node('p','Total de egresos proyectados: '+formatMoney(BigInt(projection.total)+outstanding)+'.'));
    let recurringForm;
    table(recur,['Concepto','Importe','Período','Vencimiento','Estado',''],report.recurring.map(r=>{
      const register=()=>run(async()=>{
        await api('/api/planning','POST',{kind:'confirm_recurring',id:r.id,month:report.month});
        await synchronize();
      });
      const actions=node('div');
      actions.className='actions';
      if (liquid.length && expenses.length) actions.append(action('Editar',() => openForEdit(recurringForm,r)));
      actions.append(removePlan('remove_recurring',r.id));
      return [r.name,formatMoney(r.amount),[r.first_month,r.last_month].filter(Boolean).join(' – '),r.due||'Fuera del mes',r.posted?'Registrado':r.due?action('Registrar gasto',register):'—',actions];
    }));
    if(liquid.length && expenses.length) {
      const f=newForm(recur,'Nuevo gasto mensual',v=>save({kind:'recurring',...v,id:v.id || crypto.randomUUID(),amount:Number(v.amount),day:Number(v.day),last_month:v.last_month||null}));
      field(f.form,'','id','hidden',{required:false}).parentElement.hidden=true;
      field(f.form,'Concepto','name','text',{maxLength:100});field(f.form,'Cuenta','account_id','select',{items:liquid});
      field(f.form,'Categoría','category_id','select',{items:expenses});field(f.form,'Importe mensual','amount','number',{min:1,step:1});
      field(f.form,'Desde','first_month','month',{value:report.month});field(f.form,'Hasta (opcional)','last_month','month',{required:false});field(f.form,'Día del mes','day','number',{min:1,max:31,value:1});f.end();
      recurringForm=f;
    }
    const receipts=section(body,'Comprobantes');
    const movements=[...report.operations].sort((a,b)=>b.date.localeCompare(a.date));
    const opName=op=>[op.date,op.description || kinds[op.kind] || op.kind,formatMoney(op.amount)].join(' · ');
    const names=new Map(movements.map(op=>[op.id,opName(op)]));
    table(receipts,['Movimiento','Archivo',''],report.receipts.map(r=>{
      const link=action(r.name,async()=>{
        try {
          const data=await api('/api/receipts/'+encodeURIComponent(r.id),'GET',null,'blob');
          if(!live())return;
          const url=URL.createObjectURL(data), a=node('a');a.href=url;a.download=r.name;a.click();
          setTimeout(()=>URL.revokeObjectURL(url),60000);
        } catch(e){error(e);}
      });
      return [names.get(r.operation_id)||r.operation_id,link,action('Eliminar',()=>{if(confirm('¿Eliminar este comprobante?'))run(()=>api('/api/receipts/'+r.id,'DELETE',{}));})];
    }));
    if(movements.length) {
      const form=node('form');
      field(form,'Movimiento','operation_id','select',{items:movements.map(op=>({id:op.id,name:opName(op)}))});
      const file=field(form,'Archivo PDF, JPEG o PNG (hasta 5 MiB)','file','file',{accept:'image/jpeg,image/png,application/pdf'});
      form.append(node('button','Adjuntar comprobante'));receipts.append(form);
      form.addEventListener('submit',event=>{event.preventDefault();run(async()=>{
        const item=file.files[0];const operationId=form.elements.operation_id.value;
        if(!item||item.size>5*1024*1024)throw new Error('Selecciona un archivo de hasta 5 MiB.');
        const content=hexOf(await item.arrayBuffer());
        if(!live())return;
        await api('/api/receipts','POST',{operation_id:operationId,name:item.name,mime:item.type,content});
      });});
    } else receipts.append(node('p','Registra y sincroniza un movimiento para adjuntarle un comprobante.'));
    const history=node('details');
    history.append(node('summary','Historial de cambios del mes'));
    const dateFormat=new Intl.DateTimeFormat('es-PY',{timeZone:'America/Asuncion',dateStyle:'short',timeStyle:'short'});
    table(history,['Fecha de cambio','Acción','Anterior','Actual'],report.changes.map(change=>[
      dateFormat.format(new Date(change.at*1000)),
      change.current.deleted?'Eliminación':change.previous?'Edición':'Alta',
      change.previous ? opName(change.previous) : '—',
      change.current.deleted ? '—' : opName(change.current),
    ]));
    if (!report.changes.length) history.append(node('p','El mes no tiene cambios registrados en el historial.'));
    body.append(history);
  }
  month.addEventListener('change',refresh);
  controls.append(action('Actualizar informe',() => run(synchronize)));await refresh();
}

function openForEdit(target, record) {
  target.details.open=true;
  for (const input of target.form.elements) {
    if (input.name in record) input.value=record[input.name] ?? '';
  }
  target.form.elements.name.focus();
}
