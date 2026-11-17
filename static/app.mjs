import {openUserStore} from './offline-store.mjs';
import {syncUser} from './sync.mjs';
import {calculateBalances, overlayOperations, checkEditBase} from './view-model.mjs';

const $ = id => document.getElementById(id);
const sessionKey = 'misfinanzas:last-user'; // Identity only; the session secret remains in an HttpOnly cookie.
let user = null, store = null, view = null, queue = [], syncing = false, editBase = null;
const money = n => new Intl.NumberFormat('es-PY').format(n) + ' Gs.';
const today = () => new Intl.DateTimeFormat('en-CA', {timeZone:'America/Asuncion', year:'numeric',month:'2-digit',day:'2-digit'}).format(new Date());
function notice(message) { $('notice').textContent = message; $('notice').hidden = !message; }
function ensureLocalSession() {
  let identity;
  try { identity=JSON.parse(localStorage.getItem(sessionKey)||'null'); } catch { identity=null; }
  if (!user || identity?.id!==user.id) throw new Error('La sesión local cambió en otra pestaña.');
}
function element(tag, text, className) { const el = document.createElement(tag); if (text != null) el.textContent = text; if (className) el.className = className; return el; }
function button(label, fn) { const b = element('button',label,'secondary'); b.type='button'; b.addEventListener('click', () => Promise.resolve(fn()).catch(showError)); return b; }
function showError(error) { notice(error instanceof TypeError ? 'No se pudo conectar con el servidor. Los cambios locales se conservan.' : error?.message || 'No se pudo completar la operación.'); }
function tab(name) { for (const id of ['summary','entry','settings']) $(id).hidden = id !== name; }
async function api(path, method='GET', data=null) {
  const headers = user ? {'X-Finapp-User':user.id} : {};
  if (method !== 'GET') headers['Content-Type']='application/json';
  const response = await fetch(path,{method,headers,credentials:'same-origin',cache:'no-store',...(data === null ? {} : {body:JSON.stringify(data)})});
  if (!response.ok) { const e = new Error(response.status === 401 ? 'La sesión venció. Vuelve a iniciar sesión para sincronizar.' : `No se pudo completar la solicitud (HTTP ${response.status}).`); e.status=response.status; throw e; }
  return response.status === 204 ? null : response.json();
}
async function render() {
  if (!store) return;
  const snapshot = await store.snapshot(); queue = await store.pending();
  if (!snapshot) { $('sync-status').textContent = 'Conéctate para obtener los datos iniciales.'; return; }
  const operations = overlayOperations(snapshot.operations,queue); view = {...snapshot,operations};
  $('sync-status').textContent = queue.length ? `${queue.length} cambio(s) pendiente(s) o en conflicto.` : 'Sin cambios pendientes.';
  $('balance-note').textContent = queue.length ? 'Los saldos incluyen cambios locales y pueden cambiar al resolver conflictos.' : '';
  const balances = $('balances'); balances.replaceChildren();
  try {
    const totals = calculateBalances(snapshot.accounts, operations);
    for (const a of snapshot.accounts) { const card = element('div',null,'card'); card.append(element('strong',a.name),element('p',`${a.kind === 'card' ? 'Deuda' : 'Saldo'}: ${money(totals.get(a.id))}`)); balances.append(card); }
  } catch (error) { $('balance-note').textContent = 'No se puede mostrar un saldo confiable: ' + error.message; }
  if (!snapshot.accounts.length) balances.append(element('p','Agrega una cuenta en Configuración.','muted'));
  for (const [target,items,formId] of [['account-list',snapshot.accounts,'account-form'],['category-list',snapshot.categories,'category-form']]) {
    const list=$(target); list.replaceChildren();
    for (const item of items) list.append(button(`Editar ${item.name}`,()=>{ const f=$(formId); f.elements.id.value=item.id; f.elements.name.value=item.name; f.elements.kind.value=item.kind; f.elements.kind.disabled=true; if ('initial_balance' in item) f.elements.initial_balance.value=item.initial_balance; }));
  }
  const list=$('movements'); list.replaceChildren();
  const visible=operations.filter(op=>!op.deleted).sort((a,b)=>(b.date||'').localeCompare(a.date||''));
  for (const op of visible) {
    const row=element('article',null,'row'); const account=snapshot.accounts.find(a=>a.id===op.account_id);
    row.append(element('strong',`${op.kind === 'income' ? 'Ingreso' : op.kind === 'payment' ? 'Pago de tarjeta' : 'Gasto'} · ${money(op.amount)}`));
    row.append(element('p',`${op.date || ''} · ${account?.name || 'Cuenta'}${op.description ? ' · '+op.description : ''}`));
    if (op.pending) row.append(element('p',op.conflict ? 'Conflicto: requiere revisión' : 'Pendiente de sincronización','muted'));
    const actions=element('div',null,'row-actions');
    if (!op.conflict) { actions.append(button('Editar',()=>edit(op)),button('Eliminar',()=>remove(op))); }
    row.append(actions); list.append(row);
  }
  if (!visible.length) list.append(element('p','Todavía no hay movimientos.','muted'));
  const pendingList=$('pending-list'); pendingList.replaceChildren(); $('pending-title').hidden=!queue.length;
  for (const item of queue.filter(op=>op.status==='pending')) {
    const row=element('article',null,'row');
    row.append(element('p',`${item.action === 'create' ? 'Nuevo movimiento' : item.action === 'delete' ? 'Eliminación' : 'Edición'}: ${item.data?.description || item.recordId}`));
    row.append(button('Descartar este cambio local',async()=>{
      const dependent=queue.some(op=>op.dependsOn===item.id);
      if (!confirm(dependent ? 'Se descartará este cambio y todos los cambios posteriores asociados. ¿Continuar?' : '¿Descartar este cambio local?')) return;
      await navigator.locks.request(`misfinanzas-sync:${user.id}`,async()=>{ensureLocalSession(); await store.discardOperation(item.id,{discardDependents:dependent});});
      await render(); await synchronize();
    })); pendingList.append(row);
  }
  const conflicts=$('conflicts'); conflicts.replaceChildren(); const items=queue.filter(op=>op.status==='conflict'); $('conflicts-title').hidden=!items.length;
  for (const item of items) {
    const row=element('article',null,'row conflict'); row.append(element('strong','Se modificó este movimiento en otro dispositivo.'));
    row.append(element('p','Tu cambio: '+(item.action === 'delete' ? 'eliminar' : JSON.stringify(item.data))));
    row.append(element('p','Versión del servidor: '+(item.server ? JSON.stringify(item.server) : 'no existe')));
    const actions=element('div',null,'row-actions');
    const hasDependents=queue.some(op=>op.dependsOn===item.id);
    async function decide(choice) {
      const discardDependents=hasDependents && confirm('Hay cambios posteriores sobre este movimiento. ¿Descartarlos para resolver el conflicto?');
      if (hasDependents && !discardDependents) return;
      await navigator.locks.request(`misfinanzas-sync:${user.id}`,async()=>{ ensureLocalSession();
        if (choice==='server') await store.acceptServer(item.id,{discardDependents});
        else await store.resolveConflict(item.id,{action:item.server === null && item.action !== 'delete' ? 'create' : item.action,data:item.data},{discardDependents});
      });
      await render(); await synchronize();
    }
    actions.append(button('Conservar versión del servidor',()=>decide('server')));
    if (!(item.server === null && item.action === 'delete')) actions.append(button('Conservar mi cambio',()=>decide('local')));
    row.append(actions); conflicts.append(row);
  }
  fillSelectors();
}
function fillSelect(select, items, chosen) { select.replaceChildren(); for (const item of items) { const opt=element('option',item.name); opt.value=item.id; select.append(opt); } if (items.some(v=>v.id===chosen)) select.value=chosen; }
function fillSelectors() {
  if (!view) return; const form=$('movement-form'), kind=form.elements.kind.value;
  fillSelect(form.elements.account_id,view.accounts.filter(a=>kind!=='payment'||a.kind!=='card'),form.elements.account_id.value);
  fillSelect(form.elements.category_id,view.categories.filter(c=>c.kind===kind),form.elements.category_id.value);
  fillSelect(form.elements.card_account_id,view.accounts.filter(a=>a.kind==='card'),form.elements.card_account_id.value);
  $('category-label').hidden=kind==='payment'; $('card-label').hidden=kind!=='payment';
  form.elements.category_id.required=kind!=='payment'; form.elements.card_account_id.required=kind==='payment';
}
async function synchronize() {
  if (!store || syncing || !navigator.onLine) { await render(); return false; }
  syncing=true; $('connection').textContent='Sincronizando…';
  let success=false;
  try { await syncUser(store,user.id); notice(''); success=true; }
  catch (error) { showError(error); }
  finally { syncing=false; $('connection').textContent=success?'Conectado':navigator.onLine?'Servidor no disponible':'Sin conexión'; await render(); }
  return success;
}
async function openSession(identity) {
  user=identity; store=await openUserStore(user.id); localStorage.setItem(sessionKey,JSON.stringify(user));
  $('login-view').hidden=true; $('app-view').hidden=false; $('user-label').textContent=`Sesión: ${user.username}`; $('user-form').hidden=!user.is_admin;
  $('movement-form').elements.date.value=today();
  await render(); await synchronize();
}
function tail(id) { const own=queue.filter(op=>op.recordId===id); return own.find(op=>!own.some(other=>other.dependsOn===op.id)); }
async function enqueue(action, id, data, version, openedTail = undefined) {
  if (!navigator.locks?.request) throw new Error('Este navegador no permite guardar cambios de manera segura entre pestañas.');
  await navigator.locks.request(`misfinanzas-sync:${user.id}`,async()=>{
    ensureLocalSession();
    queue=await store.pending(); const previous=tail(id);
    if (openedTail !== undefined) checkEditBase(openedTail,previous?.id ?? null);
    if (previous?.status==='conflict') throw new Error('Resuelve el conflicto antes de modificar este movimiento.');
    await store.enqueue({recordId:id,action,data,baseVersion:previous ? null : action==='create' ? null : version,dependsOn:previous?.id ?? null});
  });
  await render(); await synchronize();
}
function edit(op) {
  editBase={version:op.version ?? 0,tail:tail(op.id)?.id ?? null};
  const f=$('movement-form'); f.reset(); f.elements.id.value=op.id; f.elements.kind.value=op.kind; fillSelectors();
  for (const key of ['amount','account_id','category_id','card_account_id','date','description','notes']) f.elements[key].value=op[key] ?? '';
  $('entry-heading').textContent='Editar movimiento'; $('cancel-edit').hidden=false; tab('entry');
}
async function remove(op) { const openedTail=tail(op.id)?.id ?? null; if (confirm('¿Eliminar este movimiento?')) await enqueue('delete',op.id,null,op.version ?? 0,openedTail); }
function resetEntry() { editBase=null; $('movement-form').reset(); $('movement-form').elements.id.value=''; $('movement-form').elements.date.value=today(); $('entry-heading').textContent='Nuevo movimiento'; $('cancel-edit').hidden=true; fillSelectors(); }

$('login-form').addEventListener('submit',async event=>{event.preventDefault();try {
  const form=event.currentTarget, f=new FormData(form); const identity=await api('/api/login','POST',{username:f.get('username'),password:f.get('password')});
  if (user && identity.id!==user.id) throw new Error('Esta sesión pertenece a otro usuario. Ingresa con el usuario de los datos locales pendientes.');
  form.reset();
  if (store) { $('login-view').hidden=true; $('app-view').hidden=false; await synchronize(); }
  else await openSession(identity);
} catch(error){showError(error);} });
$('relogin').addEventListener('click',()=>{ $('login-view').hidden=false; $('app-view').hidden=true; $('cancel-login').hidden=false; $('login-form').elements.username.value=user.username; });
$('cancel-login').addEventListener('click',()=>{ $('login-view').hidden=true; $('app-view').hidden=false; });
$('logout').addEventListener('click',async()=>{try {
  if (navigator.onLine) await synchronize();
  if (!navigator.onLine) throw new Error('Conéctate para cerrar la sesión de forma segura.');
  if (!navigator.locks?.request) throw new Error('El navegador no permite cerrar sesión de forma segura entre pestañas.');
  await navigator.locks.request(`misfinanzas-sync:${user.id}`,async()=>{
    ensureLocalSession();
    const remaining=await store.pending(); let discard=false;
    if (remaining.length) { discard=confirm('Hay cambios pendientes. ¿Descartarlos y cerrar sesión? Esta acción no se puede deshacer.'); if (!discard) throw new Error('Se conservó la sesión y los cambios pendientes.'); }
    const current=await api('/api/me'); if(current.id!==user.id) throw new Error('La sesión activa pertenece a otro usuario; conserva tus cambios y vuelve a iniciar sesión.');
    await api('/api/logout','POST',{}); await store.clear({discardPending:discard}); localStorage.removeItem(sessionKey);
  });
  store.close(); store=null; user=null; $('app-view').hidden=true; $('login-view').hidden=false; $('cancel-login').hidden=true; notice('');
} catch(error){showError(error);} });
document.querySelectorAll('[data-tab]').forEach(b=>b.addEventListener('click',()=>tab(b.dataset.tab)));
$('movement-form').elements.kind.addEventListener('change',fillSelectors);
$('cancel-edit').addEventListener('click',()=>{resetEntry();tab('summary');});
$('movement-form').addEventListener('submit',async event=>{event.preventDefault();try {
  const f=event.currentTarget, id=f.elements.id.value, amount=Number(f.elements.amount.value), kind=f.elements.kind.value;
  if (!Number.isSafeInteger(amount)||amount<=0) throw new Error('Ingresa un importe entero válido.');
  const data={kind,amount,account_id:f.elements.account_id.value,category_id:kind==='payment'?null:f.elements.category_id.value,card_account_id:kind==='payment'?f.elements.card_account_id.value:null,date:f.elements.date.value,description:f.elements.description.value||null,notes:f.elements.notes.value||null};
  if (id && !editBase) throw new Error('Vuelve a abrir el movimiento para editarlo.');
  await enqueue(id?'update':'create',id||crypto.randomUUID(),data,editBase?.version,id ? editBase.tail : undefined); resetEntry();tab('summary');
}catch(error){showError(error);} });
for (const [formId,path,make] of [
  ['account-form','/api/accounts',f=>({id:f.get('id')||crypto.randomUUID(),name:f.get('name'),kind:$('account-form').elements.kind.value,initial_balance:Number(f.get('initial_balance'))})],
  ['category-form','/api/categories',f=>({id:f.get('id')||crypto.randomUUID(),name:f.get('name'),kind:$('category-form').elements.kind.value})],
  ['user-form','/api/users',f=>({username:f.get('username'),password:f.get('password')})],
]) $(formId).addEventListener('submit',async event=>{event.preventDefault();try { const form=event.currentTarget; if(!navigator.onLine) throw new Error('Esta operación requiere conexión.'); const data=make(new FormData(form)); if ('initial_balance' in data && !Number.isSafeInteger(data.initial_balance)) throw new Error('El saldo debe ser un número entero válido.'); await api(path,'POST',data); form.reset(); if (form.elements.kind) form.elements.kind.disabled=false; if (await synchronize()) notice('Guardado correctamente.'); }catch(error){showError(error);} });
for (const [buttonId,formId] of [['account-cancel','account-form'],['category-cancel','category-form']]) $(buttonId).addEventListener('click',()=>{const f=$(formId); f.reset(); f.elements.id.value=''; f.elements.kind.disabled=false;});
window.addEventListener('online',synchronize); window.addEventListener('offline',()=>{$('connection').textContent='Sin conexión';});
$('retry-sync').addEventListener('click',synchronize);
setInterval(()=>{if (user && !document.hidden && navigator.onLine) synchronize();},15000);
window.addEventListener('storage',event=>{ if (event.key!==sessionKey || !user) return; try { ensureLocalSession(); } catch { store?.close(); store=null; user=null; view=null; queue=[]; $('app-view').hidden=true; $('login-view').hidden=false; $('cancel-login').hidden=true; notice('La sesión local cambió en otra pestaña.'); } });
$('connection').textContent=navigator.onLine?'Con conexión':'Sin conexión';
if ('serviceWorker' in navigator) navigator.serviceWorker.register('/sw.js').catch(()=>notice('No se pudo activar el modo sin conexión.'));
if (navigator.storage?.persist) navigator.storage.persist().then(granted=>{ if(!granted){$('storage-warning').textContent='El navegador no garantizó el almacenamiento persistente. Sincroniza con frecuencia para proteger los cambios locales.';$('storage-warning').hidden=false;} }).catch(()=>{$('storage-warning').textContent='No se pudo comprobar la persistencia del almacenamiento local.';$('storage-warning').hidden=false;});
try { const prior=JSON.parse(localStorage.getItem(sessionKey)||'null'); if(prior?.id && prior?.username) await openSession(prior); } catch(error) { showError(error); }
