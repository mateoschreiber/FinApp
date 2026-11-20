import {enrichCatalog, renderData} from './extras.mjs';
import {renderCredit} from './credit.mjs';
import {renderPlanning} from './planning.mjs';
import {renderReconciliation} from './reconciliation.mjs';
import {openUserStore} from './offline-store.mjs';
import {syncUser} from './sync.mjs';
import {calculateBalances, overlayOperations, checkEditBase, setupStep, historyPage, monthlyTotals} from './view-model.mjs';

const $ = id => document.getElementById(id);
const fields = id => $(id).elements;
const sessionKey = 'misfinanzas:last-user';
const movementNames = {
  income:'Ingreso', expense:'Gasto', refund:'Devolución o reintegro', transfer:'Transferencia',
  payment:'Pago de tarjeta', loan_draw:'Desembolso de préstamo', loan_repayment:'Pago de capital',
};
const suggestions = [
  ['food','Alimentación','expense'], ['transport','Transporte','expense'],
  ['housing','Vivienda','expense'], ['health','Salud','expense'], ['services','Servicios','expense'],
  ['leisure','Ocio','expense'], ['salary','Salario','income'], ['other_income','Otros ingresos','income'],
];
const money = n => new Intl.NumberFormat('es-PY').format(n) + ' Gs.';
const today = () => new Intl.DateTimeFormat('en-CA', {
  timeZone:'America/Asuncion', year:'numeric', month:'2-digit', day:'2-digit',
}).format(new Date());

let user = null;
let store = null;
let view = null;
let confirmed = null;
let queue = [];
let syncRun = null;
let editBase = null;
let currentPage = 1;
let activeTab = 'summary';
let tabGeneration = 0;

function notice(message, fromSync = false) {
  $('notice').textContent = message;
  $('notice').hidden = !message;
  $('notice').dataset.syncError = String(fromSync);
}

function lastIdentity() {
  try { return JSON.parse(localStorage.getItem(sessionKey) || 'null'); }
  catch { return null; }
}

function ownsSession(id) {
  return !!id && user?.id === id && lastIdentity()?.id === id;
}

function showError(error, fromSync = false) {
  if (error?.status === 401 && user) $('relogin').hidden = false;
  const message = error instanceof TypeError
    ? 'No se pudo conectar con el servidor. Los cambios locales se conservan.'
    : error?.status === 401
      ? 'La sesión venció. Vuelve a iniciar sesión para sincronizar.'
      : error?.message || 'No se pudo completar la operación.';
  notice(message, fromSync);
}

function element(tag, text, className) {
  const el = document.createElement(tag);
  if (text != null) el.textContent = text;
  if (className) el.className = className;
  return el;
}

function button(label, run) {
  const b = element('button', label, 'secondary');
  b.type = 'button';
  b.addEventListener('click', async () => {
    b.disabled = true;
    try { await run(); }
    catch (error) { showError(error); }
    finally { b.disabled = false; }
  });
  return b;
}

async function api(path, method = 'GET', data = null, format = 'json') {
  const headers = user ? {'X-Finapp-User':user.id} : {};
  if (method !== 'GET') headers['Content-Type'] = 'application/json';
  const response = await fetch(path, {
    method, headers, credentials:'same-origin', cache:'no-store',
    ...(data === null ? {} : {body:JSON.stringify(data)}),
  });
  if (!response.ok) {
    const type = response.headers.get('content-type') || '';
    const detail = type.startsWith('text/plain') ? (await response.text()).trim().slice(0,500) : '';
    const error = new Error(detail || ({
      400:'Revisa los datos del formulario.',
      401:'La sesión venció. Vuelve a iniciar sesión.',
      403:'Esta sesión no tiene acceso a la solicitud.',
      409:'Los datos cambiaron. Actualiza la vista y vuelve a intentarlo.',
      413:'El archivo supera el tamaño admitido.',
      422:'Revisa los valores del formulario.',
    })[response.status] || 'No se pudo completar la solicitud (HTTP ' + response.status + ').');
    error.status = response.status;
    if (error.status === 401 && user) $('relogin').hidden = false;
    throw error;
  }
  if (response.status === 204) return null;
  return format === 'blob' ? response.blob() : response.json();
}

function tab(name) {
  activeTab = name;
  const generation = ++tabGeneration;
  for (const view of document.querySelectorAll('[data-view]')) view.hidden = view.id !== name;
  for (const b of document.querySelectorAll('[data-tab]')) b.setAttribute('aria-pressed', b.dataset.tab === name);
  if (!['credit','planning','exchange'].includes(name) || !user) return;
  const userId = user.id;
  const isCurrent = () => ownsSession(userId) && generation === tabGeneration;
  const context = {
    api: (...args) => {
      if (!isCurrent()) return Promise.reject(new Error('Vuelve a abrir esta sección para continuar.'));
      return api(...args);
    },
    userId, isCurrent, view:confirmed, getView:() => confirmed, today:today(), synchronize,
  };
  $(name).querySelector('[data-awaiting]')?.remove();
  if (!confirmed) {
    const status = element('p','Conéctate para cargar los datos iniciales.');
    status.dataset.awaiting = '';
    $(name).prepend(status);
    return;
  }
  if (name === 'credit') renderCredit($(name), context).catch(showError);
  if (name === 'planning') renderPlanning($(name), context).catch(showError);
  if (name === 'exchange') {
    renderData($('data-tools'), context);
    renderReconciliation($('reconciliation'), context);
  }
}

async function render() {
  const local = store;
  if (!local) return;
  const [snapshot, pending] = await Promise.all([local.snapshot(), local.pending()]);
  if (local !== store || !ownsSession(local.userId)) return;
  queue = pending;
  if (!snapshot) {
    $('sync-status').textContent = 'Conéctate para obtener los datos iniciales.';
    return;
  }
  const first = !confirmed;
  confirmed = snapshot;
  enrichCatalog(confirmed);
  view = {...snapshot, operations:overlayOperations(snapshot.operations, queue)};
  const rejected = queue.filter(item => item.status === 'rejected').length;
  const conflicts = queue.filter(item => item.status === 'conflict').length;
  const waiting = queue.length - rejected - conflicts;
  $('sync-status').textContent = queue.length
    ? [waiting && waiting + ' pendientes', rejected && rejected + ' rechazados', conflicts && conflicts + ' en conflicto'].filter(Boolean).join(' · ')
    : 'Sin cambios pendientes.';
  renderBalances();
  renderCatalog();
  renderHistory();
  renderQueue();
  fillSelectors();
  if (first && ['credit','planning','exchange'].includes(activeTab)) tab(activeTab);
}

function renderKpis(totals) {
  const kpis = $('kpis');
  kpis.replaceChildren();
  if (!view.accounts.length) return;
  let net = 0, available = 0, debt = 0;
  for (const a of view.accounts) {
    const balance = totals.get(a.id) ?? 0;
    net += balance;
    if (a.kind === 'card') debt += balance; else available += balance;
  }
  const month = today().slice(0, 7);
  const result = monthlyTotals(view.operations, month);
  const monthLabel = new Intl.DateTimeFormat('es-PY', {month: 'long', year: 'numeric'}).format(new Date());
  const kpi = (label, value, className, detail) => {
    const card = element('div', null, 'kpi');
    card.append(element('p', label, 'label'), element('p', money(value), 'value ' + className));
    if (detail) card.append(element('p', detail, 'detail'));
    kpis.append(card);
  };
  kpi('Patrimonio neto', net, net >= 0 ? 'positive' : 'negative');
  kpi('Disponible', available, available >= 0 ? 'positive' : 'negative');
  kpi('Deuda de tarjetas', debt, debt < 0 ? 'negative' : '');
  kpi('Resultado de ' + monthLabel, Number(result.net), result.net >= 0 ? 'positive' : 'negative',
    'Ingresos ' + money(Number(result.income)) + ' · Gastos ' + money(Number(result.expense)));
}

function renderBalances() {
  const balances = $('balances');
  balances.replaceChildren();
  $('kpis').replaceChildren();
  $('balance-note').textContent = queue.some(item => item.status === 'pending' || item.status === 'conflict')
    ? 'Saldos provisionales con cambios locales.' : '';
  try {
    const totals = calculateBalances(view.accounts, view.operations);
    renderKpis(totals);
    for (const a of view.accounts) {
      const card = element('article',null,'card');
      const balance = totals.get(a.id);
      const label = (a.kind === 'card' ? 'Deuda ' : 'Saldo ') + money(balance);
      const color = balance < 0 || a.kind === 'card' ? 'negative' : 'positive';
      card.append(element('strong', a.name), element('p', label, 'amount ' + color));
      if (a.archived) card.append(element('p','Archivada','muted'));
      balances.append(card);
    }
  } catch (error) { $('balance-note').textContent = error.message; }
  const step = setupStep(view);
  if (step === 'ready') return;
  const prompt = element('div',null,'card');
  if (step === 'account') {
    prompt.append(element('p','Crea una cuenta para registrar tus primeros movimientos.'),
      button('Crear mi primera cuenta', () => followSetup('account-form')));
  } else {
    prompt.append(element('p','Elige una categoría para clasificar tu primer gasto.'),
      button('Configurar categorías', () => followSetup('category-form','expense')));
  }
  balances.append(prompt);
}

function followSetup(formId, kind) {
  resetCatalog(formId);
  tab('settings');
  const form = $(formId);
  if (kind) form.elements.kind.value = kind;
  if (formId === 'category-form') $('suggestions-form').closest('details').open = true;
  if (formId === 'account-form') renderAccountFields();
  form.scrollIntoView({block:'start'});
  form.elements.name.focus();
}

function renderAccountFields() {
  const card = fields('account-form').kind.value === 'card';
  $('card-details').hidden = !card;
  fields('account-form').closing_day.disabled = !card;
  fields('account-form').due_day.disabled = !card;
}

function resetCatalog(id) {
  const form = $(id);
  form.reset();
  form.elements.kind.disabled = false;
  form.elements.id.value = '';
  const account = id === 'account-form';
  form.querySelector('h4').textContent = account ? 'Nueva cuenta' : 'Nueva categoría';
  $(account ? 'account-cancel' : 'category-cancel').hidden = true;
  if (account) renderAccountFields();
}

function renderCatalog() {
  for (const [entity,items] of [['account',view.accounts],['category',view.categories]]) {
    const list = $(entity+'-list');
    list.replaceChildren();
    const formId = entity+'-form';
    for (const item of items) {
      const row = element('article',null,'row');
      const actions = element('div',null,'actions');
      row.append(element('strong',item.name));
      if (item.archived) row.append(element('p','Archivada','muted'));
      actions.append(button('Editar',() => {
        const form = $(formId);
        resetCatalog(formId);
        for (const f of form.elements) {
          if (f.name && Object.hasOwn(item,f.name)) f.value = item[f.name] ?? '';
        }
        form.elements.kind.disabled = true;
        form.querySelector('h4').textContent = 'Editar '+item.name;
        $(entity+'-cancel').hidden = false;
        if (entity === 'account') renderAccountFields();
        form.elements.name.focus();
      }), button(item.archived ? 'Reactivar' : 'Archivar',async () => {
        await api('/api/catalog/archive','POST',{entity,id:item.id,archived:!item.archived});
        await synchronize();
      }));
      row.append(actions);
      list.append(row);
    }
  }
  const container = $('suggested-categories');
  const available = suggestions.filter(([id,name,kind]) => !view.categories.some(c =>
    c.id === 'suggested:'+id || c.kind === kind && c.name.toLocaleLowerCase('es') === name.toLocaleLowerCase('es')));
  const signature = available.map(row => row[0]).join(',');
  if (container.dataset.signature === signature) return;
  container.replaceChildren();
  container.dataset.signature = signature;
  if (!available.length) container.append(element('p','Ya tienes las categorías sugeridas.'));
  for (const [id,name,kind] of available) {
    const label = element('label',name+' · '+movementNames[kind],'checkbox');
    const input = document.createElement('input');
    input.type = 'checkbox';
    input.name = 'suggestion';
    input.value = id;
    input.checked = true;
    label.prepend(input);
    container.append(label);
  }
  $('suggestions-form').querySelector('button').hidden = !available.length;
}

function fillSelect(select, items, chosen) {
  const values = [...select.options];
  if (items.length !== values.length || items.some((item,i) => item.id !== values[i].value || item.name !== values[i].textContent)) {
    select.replaceChildren(...items.map(item => {
      const option = element('option',item.name);
      option.value = item.id;
      return option;
    }));
  }
  if (items.some(item => item.id === chosen)) select.value = chosen;
}

function renderHistory() {
  if (!view) return;
  const list = $('movements');
  list.replaceChildren();
  const filters = $('history-filter');
  for (const [field,items] of [['account',view.accounts],['category',view.categories]]) {
    const select = filters.elements[field];
    fillSelect(select,[{id:'',name:'Todas'},...items],select.value);
  }
  const page = historyPage(view.operations,Object.fromEntries(new FormData(filters)),currentPage);
  currentPage = page.page;
  $('history-count').textContent = page.total ? page.total+' movimientos · Página '+page.page+' de '+page.pages : '';
  $('history-pages').hidden = page.pages === 1;
  $('previous-page').disabled = page.page === 1;
  $('next-page').disabled = page.page === page.pages;
  const accounts = new Map(view.accounts.map(a => [a.id,a]));
  const categories = new Map(view.categories.map(c => [c.id,c.name]));
  const original = new Map(view.operations.map(op => [op.id,op]));
  const signClass = op => op.kind === 'income' || op.kind === 'refund' ? 'positive'
    : op.kind === 'transfer' ? '' : 'negative';
  for (const op of page.rows) {
    const row = element('article',null,'row');
    const account = accounts.get(op.account_id);
    const destination = accounts.get(op.destination_account_id || op.card_account_id);
    row.append(element('strong',(movementNames[op.kind] || op.kind)+' · '+money(op.amount),'amount '+signClass(op)));
    const details = [op.date,[account?.name,destination?.name].filter(Boolean).join(' → '),categories.get(op.category_id),op.description];
    row.append(element('p',details.filter(Boolean).join(' · ')));
    if (op.original_operation_id) {
      const expense = original.get(op.original_operation_id);
      row.append(element('p','Gasto original · '+[expense?.date,expense?.description].filter(Boolean).join(' · '),'muted'));
    }
    if ([account,destination].some(a => a?.cutoff_date && op.date < a.cutoff_date)) {
      row.append(element('p','Movimiento anterior a la fecha de corte de una cuenta.','muted'));
    }
    if (op.pending) row.append(element('p',op.conflict ? 'Conflicto por resolver' : 'Pendiente de sincronización','muted'));
    if (!op.conflict && !op.id.startsWith('loan:') && !op.kind.startsWith('loan_')
      && !queue.some(q => q.recordId === op.id && q.status === 'rejected')) {
      const actions = element('div',null,'row-actions');
      actions.append(button('Editar',() => edit(op)),button('Eliminar',() => remove(op)));
      row.append(actions);
    }
    list.append(row);
  }
  if (!page.total) list.append(element('p',view.operations.some(op => !op.deleted)
    ? 'Sin movimientos para estos filtros.' : 'Todavía no hay movimientos.','muted'));
}

function describe(op) {
  if (!op || op.deleted) return 'Movimiento eliminado';
  const account = view.accounts.find(a => a.id === op.account_id)?.name;
  return [movementNames[op.kind],op.date,op.amount != null ? money(op.amount) : '',account,op.description,op.notes]
    .filter(Boolean).join(' · ');
}

async function changeQueue(action) {
  const local = store;
  const id = user?.id;
  if (!navigator.locks?.request) throw new Error('El navegador no permite guardar cambios de forma segura entre pestañas.');
  await navigator.locks.request('misfinanzas-sync:'+id,async () => {
    if (!ownsSession(id) || local !== store) throw new Error('La sesión local cambió.');
    await action(local);
  });
  await render();
  await synchronize();
}

function renderQueue() {
  const pending = $('pending-list');
  const conflicts = $('conflicts');
  pending.replaceChildren();
  conflicts.replaceChildren();
  for (const item of queue) {
    const row = element('article',null,'row');
    row.append(element('strong',item.action === 'delete' ? 'Eliminar movimiento' : describe(item.data)));
    const actions = element('div',null,'actions');
    const hasDependents = queue.some(op => op.dependsOn === item.id);
    const discard = () => confirm(hasDependents
      ? '¿Descartar este cambio y los cambios locales posteriores sobre el mismo movimiento?'
      : '¿Descartar este cambio local?');
    if (item.status === 'conflict') {
      row.classList.add('conflict');
      row.append(element('p','Versión del servidor · '+(item.server ? describe(item.server) : 'Movimiento inexistente')));
      const resolve = async choice => {
        if (hasDependents && !discard()) return;
        await changeQueue(local => choice === 'server'
          ? local.acceptServer(item.id,{discardDependents:hasDependents})
          : local.resolveConflict(item.id,{
            action:item.server == null ? 'create' : item.action === 'create' ? 'update' : item.action,
            data:item.data,
          },{discardDependents:hasDependents}));
      };
      actions.append(button('Conservar versión del servidor',() => resolve('server')));
      if (item.server || item.action !== 'delete') actions.append(button('Conservar mi cambio',() => resolve('local')));
      conflicts.append(row);
    } else {
      row.append(element('p',item.status === 'rejected'
        ? item.rejection.message
        : item.dependsOn ? 'En espera del cambio anterior sobre este movimiento.' : 'Pendiente de sincronización.','muted'));
      if (item.status === 'rejected') {
        if (item.data) actions.append(button('Corregir',() => edit({...item.data,id:item.recordId,version:item.baseVersion},item.id)));
        actions.append(button('Reintentar',() => changeQueue(local => local.retryRejected(item.id))));
      }
      actions.append(button('Descartar cambio',async () => {
        if (discard()) await changeQueue(local => local.discardOperation(item.id,{discardDependents:hasDependents}));
      }));
      pending.append(row);
    }
    row.append(actions);
  }
  $('pending-title').hidden = !pending.children.length;
  $('conflicts-title').hidden = !conflicts.children.length;
}

function fillSelectors() {
  if (!view) return;
  const form = $('movement-form');
  const kind = form.elements.kind.value;
  const accounts = view.accounts;
  const editing = confirmed.operations.find(op => op.id === form.elements.id.value);
  const allowed = (item,field) => !item.archived || item.id === editing?.[field];
  const paired = kind === 'transfer' || kind === 'payment';
  const options = [
    ['account_id',accounts.filter(a => allowed(a,'account_id') && (!paired || a.kind !== 'card'))],
    ['category_id',view.categories.filter(c => allowed(c,'category_id') && c.kind === kind)],
    ['card_account_id',accounts.filter(a => allowed(a,'card_account_id') && a.kind === 'card')],
    ['destination_account_id',accounts.filter(a => allowed(a,'destination_account_id') && a.kind !== 'card')],
  ];
  for (const [id,items] of options) {
    const select = form.elements[id];
    fillSelect(select,id === 'destination_account_id' ? items.filter(a => a.id !== form.elements.account_id.value) : items,select.value);
  }
  const refundTo = kind === 'refund' && accounts.find(a => a.id === form.elements.account_id.value);
  const expenses = refundTo ? confirmed.operations.filter(op => op.kind === 'expense' && !op.deleted
    && (refundTo.kind !== 'card' || op.account_id === refundTo.id)) : [];
  const original = form.elements.original_operation_id;
  fillSelect(original,expenses.map(op => ({id:op.id,name:[op.date,op.description || 'Gasto',money(op.amount)].join(' · ')})),original.value);
  for (const [label,field,visible] of [
    ['category-label','category_id',kind === 'income' || kind === 'expense'],
    ['card-label','card_account_id',kind === 'payment'],
    ['destination-label','destination_account_id',kind === 'transfer'],
    ['original-label','original_operation_id',kind === 'refund'],
  ]) {
    $(label).hidden = !visible;
    form.elements[field].disabled = !visible;
    form.elements[field].required = visible;
  }

  const hint = $('entry-needed');
  const missing = [
    ['account_id','Crea una cuenta para este movimiento.','Crear cuenta',() => followSetup('account-form')],
    ['category_id','Crea una categoría de '+movementNames[kind]?.toLowerCase()+'.','Crear categoría',() => followSetup('category-form',kind)],
    ['card_account_id','Crea la tarjeta que quieres pagar.','Crear tarjeta',() => followSetup('account-form','card')],
    ['destination_account_id','Crea otra cuenta para recibir la transferencia.','Crear cuenta de destino',() => followSetup('account-form')],
    ['original_operation_id','Selecciona un gasto ya sincronizado.','Registrar gasto',() => {resetEntry(); tab('entry');}],
  ].find(([name]) => !form.elements[name].disabled && !form.elements[name].options.length);
  hint.replaceChildren();
  if (missing) hint.append(element('p',missing[1]),button(missing[2],missing[3]));
  hint.hidden = !missing;
  $('save-movement').disabled = !!missing || !!form.dataset.busy;
  cutoffHint();
}

function cutoffHint() {
  if (!view) return;
  const f = fields('movement-form');
  const ids = [f.account_id,f.destination_account_id,f.card_account_id].filter(input => !input.disabled).map(input => input.value);
  const before = view.accounts.filter(a => ids.includes(a.id) && a.cutoff_date && f.date.value && f.date.value < a.cutoff_date);
  $('entry-cutoff').hidden = !before.length;
  $('entry-cutoff').textContent = before.length
    ? 'Fecha anterior al corte. Se guarda en el historial y queda fuera del saldo de '+before.map(a=>a.name).join(', ')+'.' : '';
}

function tail(id) {
  const own = queue.filter(op => op.recordId === id);
  return own.find(op => !own.some(other => other.dependsOn === op.id));
}
function edit(op, rejectedId = null) {
  editBase = {version:op.version ?? 0, tail:tail(op.id)?.id ?? null, rejectedId};
  const form = $('movement-form');
  form.reset();
  form.elements.id.value = op.id;
  form.elements.kind.value = op.kind;
  fillSelectors();
  if ([...form.elements.account_id.options].some(opt => opt.value === op.account_id)) form.elements.account_id.value = op.account_id;
  fillSelectors();
  for (const name of ['amount','category_id','card_account_id','destination_account_id','original_operation_id','date','description','notes']) {
    if (op[name] != null) form.elements[name].value = op[name];
  }
  fillSelectors();
  $('entry-heading').textContent = rejectedId ? 'Corregir movimiento rechazado' : 'Editar movimiento';
  $('cancel-edit').hidden = false;
  tab('entry');
}
async function remove(op) {
  const editTail = tail(op.id)?.id ?? null;
  if (!confirm('¿Eliminar este movimiento?')) return;
  await changeQueue(async local => {
    queue = await local.pending();
    const previous = tail(op.id);
    checkEditBase(editTail,previous?.id ?? null);
    await local.enqueue({recordId:op.id,action:'delete',data:null,baseVersion:previous ? null : op.version,dependsOn:previous?.id ?? null});
  });
}
function resetEntry() {
  editBase = null;
  $('movement-form').reset();
  fields('movement-form').date.value = today();
  $('entry-heading').textContent = 'Nuevo movimiento';
  $('cancel-edit').hidden = true;
  fillSelectors();
}

function synchronize() {
  if (!store || !navigator.onLine) {
    $('connection').textContent = navigator.onLine ? 'Con conexión' : 'Sin conexión';
    return Promise.resolve(false);
  }
  if (syncRun?.store === store) return syncRun.promise;
  const local = store;
  $('connection').textContent = 'Sincronizando…';
  const promise = (async () => {
    try {
      await syncUser(local,local.userId);
      if (store !== local || !ownsSession(local.userId)) return false;
      if ($('notice').dataset.syncError === 'true') notice('');
      $('relogin').hidden = true;
      $('connection').textContent = 'Conectado';
      return true;
    } catch (error) {
      if (store === local) {
        $('connection').textContent = navigator.onLine ? 'Sin sincronizar' : 'Sin conexión';
        if (navigator.onLine) showError(error,true);
      }
      return false;
    } finally {
      if (syncRun?.store === local) syncRun = null;
      if (store === local) await render();
    }
  })();
  syncRun = {store:local,promise};
  return promise;
}

async function openSession(identity) {
  const local = await openUserStore(identity.id);
  user = identity;
  store = local;
  currentPage = 1;
  $('history-filter').reset();
  localStorage.setItem(sessionKey,JSON.stringify(identity));
  $('user-label').textContent = identity.username;
  $('user-admin').hidden = !identity.is_admin;
  $('login-view').hidden = true;
  $('app-view').hidden = false;
  $('relogin').hidden = false;
  resetEntry();
  tab('summary');
  await render();
  await synchronize();
}

function clearSession() {
  ++tabGeneration;
  store?.close();
  user = store = view = confirmed = null;
  queue = [];
  for (const id of ['credit','planning','data-tools','reconciliation','balances','movements','pending-list','conflicts','account-list','category-list']) $(id).replaceChildren();
  $('history-filter').reset();
  resetCatalog('account-form');
  resetCatalog('category-form');
  $('login-form').reset();
  $('user-form').reset();
  $('app-view').hidden = true;
  $('login-view').hidden = false;
  $('cancel-login').hidden = true;
  resetEntry();
}

function bindForm(id, run) {
  const form = $(id);
  form.addEventListener('submit', async event => {
    event.preventDefault();
    const submit = form.querySelector('button:not([type="button"])');
    if (form.dataset.busy) return;
    form.dataset.busy = 'true';
    submit.disabled = true;
    try { await run(new FormData(form),form); }
    catch (error) { showError(error); }
    finally {
      delete form.dataset.busy;
      submit.disabled = false;
      if (id === 'movement-form') fillSelectors();
    }
  });
}

bindForm('login-form',async data => {
  const credentials = Object.fromEntries(data);
  if (user && credentials.username.trim().toLowerCase() !== user.username.toLowerCase()) {
    throw new Error('Usa el usuario de los datos guardados en este dispositivo.');
  }
  const identity = await api('/api/login','POST',credentials);
  $('login-form').reset();
  notice('');
  if (!store) await openSession(identity);
  else if (identity.id === user.id) {
    $('login-view').hidden = true;
    $('app-view').hidden = false;
    await synchronize();
  }
});
$('relogin').addEventListener('click',() => {
  $('login-view').hidden = false;
  $('app-view').hidden = true;
  $('cancel-login').hidden = false;
  fields('login-form').username.value = user.username;
  fields('login-form').password.focus();
});
$('cancel-login').addEventListener('click',() => {
  $('login-view').hidden = true;
  $('app-view').hidden = false;
  $('login-form').reset();
});
$('logout').addEventListener('click',async () => {
  try {
    if (!navigator.onLine) throw new Error('Conéctate para cerrar la sesión.');
    if (!navigator.locks?.request) throw new Error('El navegador no permite cerrar sesión de forma segura entre pestañas.');
    await synchronize();
    const local = store;
    const id = user?.id;
    if (!local || !id) return;
    await navigator.locks.request('misfinanzas-sync:'+id,async () => {
      if (!ownsSession(id) || local !== store) return;
      const remaining = await local.pending();
      if (remaining.length && !confirm('Hay cambios locales sin sincronizar. ¿Descartarlos y cerrar la sesión?')) return;
      await api('/api/logout','POST',{});
      await local.clear({discardPending:remaining.length > 0});
      localStorage.removeItem(sessionKey);
      clearSession();
      notice('');
    });
  } catch (error) { showError(error); }
});

document.querySelectorAll('[data-tab]').forEach(b => b.addEventListener('click',() => tab(b.dataset.tab)));
for (const name of ['kind','account_id']) fields('movement-form')[name].addEventListener('change',fillSelectors);
for (const name of ['date','destination_account_id','card_account_id']) fields('movement-form')[name].addEventListener('change',cutoffHint);
$('cancel-edit').addEventListener('click',() => {resetEntry(); tab('summary');});

bindForm('movement-form',async input => {
  const f = Object.fromEntries(input);
  const amount = Number(f.amount);
  if (!Number.isSafeInteger(amount) || amount <= 0) throw new Error('Ingresa un importe entero válido.');
  const data = {
    kind:f.kind, amount, account_id:f.account_id, date:f.date,
    category_id:f.category_id || null, card_account_id:f.card_account_id || null,
    destination_account_id:f.destination_account_id || null,
    description:f.description || null, notes:f.notes || null,
  };
  if (f.kind === 'refund') {
    const original = confirmed.operations.find(op => !op.deleted && op.id === f.original_operation_id);
    if (!original) throw new Error('Selecciona un gasto sincronizado.');
    data.original_operation_id = original.id;
    data.category_id = original.category_id;
    const previous = view.operations.filter(op => op.kind === 'refund' && !op.deleted && op.id !== f.id
      && op.original_operation_id === original.id).reduce((total,op) => total + BigInt(op.amount),0n);
    if (f.date < original.date || previous + BigInt(amount) > BigInt(original.amount)) {
      throw new Error('Verifica la fecha y el importe pendiente de devolución.');
    }
  }
  const base = editBase;
  if (f.id && !base) throw new Error('Vuelve a abrir el movimiento para editarlo.');
  await changeQueue(async local => {
    if (base?.rejectedId) {
      await local.retryRejected(base.rejectedId,{data});
      return;
    }
    queue = await local.pending();
    const previous = tail(f.id);
    if (f.id) checkEditBase(base.tail,previous?.id ?? null);
    if (previous && previous.status !== 'pending') throw new Error('Resuelve primero el cambio anterior.');
    await local.enqueue({
      recordId:f.id || crypto.randomUUID(), action:f.id ? 'update' : 'create', data,
      baseVersion:previous ? null : base?.version ?? null, dependsOn:previous?.id ?? null,
    });
  });
  resetEntry();
  tab('summary');
});

bindForm('account-form',async (input,form) => {
  const f = Object.fromEntries(input);
  const kind = form.elements.kind.value;
  const initial = Number(f.initial_balance);
  if (!Number.isSafeInteger(initial)) throw new Error('El saldo debe ser un número entero válido.');
  const id = f.id || crypto.randomUUID();
  form.elements.id.value = id;
  await api('/api/accounts','POST',{
    id,name:f.name.trim(),kind,initial_balance:initial,
    details:{
      institution:f.institution.trim(),notes:f.notes,cutoff_date:f.cutoff_date || null,
      closing_day:f.closing_day ? Number(f.closing_day) : null,
      due_day:f.due_day ? Number(f.due_day) : null,
    },
  });
  const first = !view || setupStep(view) === 'account';
  resetCatalog('account-form');
  await synchronize();
  if (first && view && setupStep(view) === 'category') followSetup('category-form','expense');
  notice('Cuenta guardada.');
});
bindForm('category-form',async (input,form) => {
  const f = Object.fromEntries(input);
  const id = f.id || crypto.randomUUID();
  form.elements.id.value = id;
  await api('/api/categories','POST',{id,name:f.name.trim(),kind:form.elements.kind.value});
  resetCatalog('category-form');
  await synchronize();
  notice('Categoría guardada.');
});
bindForm('suggestions-form',async input => {
  const selected = input.getAll('suggestion');
  if (!selected.length) throw new Error('Elige al menos una categoría.');
  await api('/api/catalog/suggest','POST',{selected});
  await synchronize();
  notice('Categorías guardadas.');
});
bindForm('user-form',async data => {
  const f = Object.fromEntries(data);
  await api('/api/users','POST',{username:f.username.trim(),password:f.password});
  $('user-form').reset();
  notice('Usuario creado.');
});
$('account-cancel').addEventListener('click',() => resetCatalog('account-form'));
$('category-cancel').addEventListener('click',() => resetCatalog('category-form'));
fields('account-form').kind.addEventListener('change',renderAccountFields);

$('history-filter').addEventListener('submit',event => event.preventDefault());
$('history-filter').addEventListener('input',() => {currentPage=1; renderHistory();});
$('previous-page').addEventListener('click',() => {currentPage--; renderHistory();});
$('next-page').addEventListener('click',() => {currentPage++; renderHistory();});
$('retry-sync').addEventListener('click',() => synchronize().catch(showError));
window.addEventListener('online',() => synchronize().catch(showError));
window.addEventListener('offline',() => {$('connection').textContent='Sin conexión';});
window.addEventListener('storage',event => {
  if (event.key === sessionKey && user && !ownsSession(user.id)) {
    clearSession();
    notice('La sesión local cambió en otra pestaña.');
  }
});
setInterval(() => {
  if (user && !document.hidden && navigator.onLine) synchronize().catch(showError);
},15000);

$('connection').textContent = navigator.onLine ? 'Con conexión' : 'Sin conexión';
if ('serviceWorker' in navigator) {
  navigator.serviceWorker.register('/sw.js').then(registration => {
    const update = () => {$('reload-app').hidden = !registration.waiting || !navigator.serviceWorker.controller;};
    update();
    registration.addEventListener('updatefound', () => registration.installing?.addEventListener('statechange',update));
    $('reload-app').addEventListener('click', async () => {
      if (!registration.waiting) return;
      // Pending edits live in IndexedDB and survive the reload; confirm so the user picks the moment.
      const pending = store ? await store.pending().catch(() => []) : [];
      if (pending.length && !confirm('Hay cambios sin sincronizar. Se conservarán después de actualizar. ¿Continuar?')) return;
      navigator.serviceWorker.addEventListener('controllerchange',() => location.reload(),{once:true});
      registration.waiting.postMessage('ACTIVATE_UPDATE');
    });
    // A long-lived PWA window only revalidates the worker on navigation; check hourly.
    setInterval(() => registration.update().catch(() => {}), 60 * 60 * 1000);
  }).catch(() => notice('No se pudo activar el modo sin conexión.'));
}
if (navigator.storage?.persist) {
  navigator.storage.persist().then(granted => {
    $('storage-warning').hidden = granted;
    $('storage-warning').textContent = 'El navegador no garantizó el almacenamiento persistente. Sincroniza tus cambios con frecuencia.';
  }).catch(() => {});
}
const previous = lastIdentity();
if (previous?.id && previous?.username) {
  try { await openSession(previous); }
  catch (error) { showError(error); }
}
