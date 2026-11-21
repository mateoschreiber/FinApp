import {element as el, money} from './ui.mjs';

const mounted = new WeakMap();
const labelForKind = {income:'Ingreso',expense:'Gasto',payment:'Pago de tarjeta',transfer:'Transferencia',refund:'Devolución',loan_draw:'Desembolso',loan_repayment:'Pago de préstamo'};

export function renderReconciliation(container, context) {
  const existing=mounted.get(container);
  if (existing?.userId===context.userId && container.contains(existing.select)) {
    existing.update(context);
    return;
  }
  const status=el('p');
  status.setAttribute('role','status');
  const select=el('select');
  const label=el('label','Cuenta del extracto');
  label.append(select);
  const file=el('input');
  file.type='file';
  file.accept='.csv,text/csv,text/plain';
  const uploadLabel=el('label','Extracto CSV');
  uploadLabel.append(file);
  const actions=el('div');
  actions.className='actions';
  const results=el('div');
  container.replaceChildren(el('h2','Conciliación bancaria'),label,uploadLabel,
    el('p','CSV UTF-8 con fecha, importe, descripción y referencia. Usa AAAA-MM-DD e importes enteros. Los débitos llevan signo negativo. En tarjetas, un cargo aumenta la deuda y lleva signo positivo. La referencia es opcional y permite distinguir filas idénticas.'),
    actions,status,results);
  let latest=context, preview=null, loadId=0, page=1;
  const alive=()=>mounted.get(container)?.select===select && container.contains(select);
  function button(parent,title,action) {
    const b=el('button',title);
    b.type='button';
    b.addEventListener('click',async()=>{
      b.disabled=true;
      try {await action();}
      catch(error) {if(alive()) status.textContent=error.message || 'No se pudo completar la solicitud.';}
      finally {b.disabled=false;}
    });
    parent.append(b);
    return b;
  }
  function update(next) {
    latest=next;
    const items=next.getView()?.accounts ?? [];
    const before=select.value;
    const choices=[...select.options];
    if (choices.length===items.length && items.every((a,i)=>choices[i].value===a.id && choices[i].textContent===a.name)) return;
    select.replaceChildren(...items.map(a=>{const opt=el('option',a.name);opt.value=a.id;return opt;}));
    if (items.some(a=>a.id===before)) select.value=before;
    if (before!==select.value) clear();
  }
  function clear() {
    loadId++; page=1; preview=null; confirm.hidden=true;
    status.textContent='';results.replaceChildren();
  }
  async function load(requested=1) {
    const account=select.value;
    if (!account) throw new Error('Crea una cuenta para conciliar su extracto.');
    const token=++loadId;
    const data=await latest.api('/api/reconciliation?'+new URLSearchParams({account_id:account,page:requested}));
    if (!alive() || token!==loadId) return;
    page=data.page; results.replaceChildren();
    results.append(el('p',data.total ? data.total+' línea(s) · Página '+data.page+' de '+data.pages : 'Aún no hay líneas de extracto.'));
    for (const line of data.rows) {
      const row=el('article');row.className='row';
      row.append(el('strong',line.date+' · '+money(line.amount)),el('p',[line.description,line.reference].filter(Boolean).join(' · ')));
      const link={account_id:account,line_id:line.line_id};
      if(line.operation_id) {
        row.append(el('p',line.stale ? 'El movimiento vinculado cambió. Revisa su conciliación.' : 'Conciliado.'));
        button(row,'Quitar vínculo',async()=>{
          await latest.api('/api/reconciliation/unmatch','POST',{...link,operation_id:line.operation_id});
          if(alive() && select.value===account) await load(page);
        });
      }
      if(!line.operation_id || line.stale) {
        const choices=el('div');
        async function candidates(candidatePage) {
          const options=await latest.api('/api/reconciliation/candidates?'+new URLSearchParams({...link,page:candidatePage}));
          if (!alive() || token!==loadId) return;
          choices.replaceChildren(el('p',options.total ? 'Elige la coincidencia por fecha e importe.' : 'Sin movimientos disponibles con esa fecha e importe.'));
          for(const op of options.rows) {
            const b=button(choices,[labelForKind[op.kind] ?? op.kind,op.description,op.id].filter(Boolean).join(' · '),async()=>{
              await latest.api('/api/reconciliation/match','POST',{...link,operation_id:op.id,expected_version:op.version});
              if (alive() && select.value===account) await load(page);
            });
            if(line.operation_id && op.id!==line.operation_id) b.disabled=true;
          }
          if(options.page>1) button(choices,'Coincidencias anteriores',()=>candidates(options.page-1));
          if(options.page<options.pages) button(choices,'Más coincidencias',()=>candidates(options.page+1));
        }
        button(row,'Buscar coincidencias',()=>candidates(1));row.append(choices);
      }
      results.append(row);
    }
    if(data.page>1) button(results,'Anterior',()=>load(page-1));
    if(data.page<data.pages) button(results,'Siguiente',()=>load(page+1));
  }
  button(actions,'Ver extracto',()=>load());
  button(actions,'Previsualizar CSV',async()=>{
    const selected=file.files[0],account=select.value;
    if(!selected || !account) throw new Error('Selecciona una cuenta y un archivo CSV.');
    if(selected.size>1024*1024) throw new Error('El archivo supera 1 MiB.');
    preview=null;confirm.hidden=true;
    const csv=new TextDecoder('utf-8',{fatal:true}).decode(await selected.arrayBuffer());
    const result=await latest.api('/api/reconciliation/import','POST',{account_id:account,csv,confirm:false});
    if(!alive() || account!==select.value || selected!==file.files[0]) return;
    preview={account_id:account,csv};
    status.textContent=result.added+' línea(s) nueva(s) y '+result.existing+' ya registrada(s).';
    confirm.hidden=!result.added;
  });
  const confirm=button(actions,'Confirmar importación',async()=>{
    const input=preview;
    if(!input || input.account_id!==select.value) throw new Error('Vuelve a previsualizar el archivo.');
    const result=await latest.api('/api/reconciliation/import','POST',{...input,confirm:true});
    if (!alive() || preview!==input) return;
    preview=null;confirm.hidden=true;file.value='';
    status.textContent=result.added+' línea(s) importada(s).';
    await load();
  });
  confirm.hidden=true;
  file.addEventListener('change',()=>{preview=null;confirm.hidden=true;status.textContent='';});
  select.addEventListener('change',clear);
  mounted.set(container,{select,userId:context.userId,update});
  update(context);
}
