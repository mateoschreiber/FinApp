import {enrichCatalog, renderData} from './extras.mjs';
import {renderCredit} from './credit.mjs';
import {renderPlanning} from './planning.mjs';
import {renderReconciliation} from './reconciliation.mjs';
import {openUserStore} from './offline-store.mjs';
import {syncUser} from './sync.mjs';
import {calculateBalances, overlayOperations, checkEditBase, setupStep, historyPage, monthlyTotals, netWorth} from './view-model.mjs';
import {confirmAction, element, icon, lastIdentity, money, pill, sessionKey} from './ui.mjs';
import {
  animateMoney, bindAnimatedDialog, buildDonutSvg, buildSparklineSvg, closeTopLayer,
  expenseCategorySlices, formatRelativeDate, initSpotlight, initialsOf,
  monthlyCashflowSeries, passwordStrength, showToast,
} from './visuals.mjs';

const $ = id => document.getElementById(id);
const fields = id => $(id).elements;
const movementNames = {
  income: 'Ingreso', expense: 'Gasto', refund: 'Devolución o reintegro', transfer: 'Transferencia',
  payment: 'Pago de tarjeta', loan_draw: 'Desembolso de préstamo', loan_repayment: 'Pago de capital',
};
const accountTypes = {
  bank: 'Banco', cash: 'Efectivo', wallet: 'Billetera', card: 'Tarjeta de crédito',
};
const suggestions = [
  ['food', 'Alimentación', 'expense'], ['transport', 'Transporte', 'expense'],
  ['housing', 'Vivienda', 'expense'], ['health', 'Salud', 'expense'], ['services', 'Servicios', 'expense'],
  ['leisure', 'Ocio', 'expense'], ['salary', 'Salario', 'income'], ['other_income', 'Otros ingresos', 'income'],
];
const today = () => new Intl.DateTimeFormat('en-CA', {
  timeZone: 'America/Asuncion', year: 'numeric', month: '2-digit', day: '2-digit',
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
let activeSettingsTab = 'accounts';
let tabGeneration = 0;
let lastAvailableBalance = null;
const IDLE_TIMEOUT_MS = 5 * 60 * 1000;
const activityKey = 'misfinanzas:last-activity';
let lastActivity = Date.now();
let idleTimer = null;
let closingIdle = false;

function notice(message, tone = 'success') {
  if (!message) return;
  showToast(message, tone);
}

function setConnectionStatus(text, offline = !navigator.onLine) {
  $('connection').textContent = text;
  const dot = $('sidebar-connection');
  if (dot) {
    dot.dataset.offline = String(offline);
    dot.title = text;
  }
}

function ownsSession(id) {
  return !!id && user?.id === id && lastIdentity()?.id === id;
}

function showError(error) {
  if (error?.status === 401 && user) $('relogin').hidden = false;
  const message = error instanceof TypeError
    ? 'No se pudo conectar con el servidor. Los cambios locales se conservan.'
    : error?.status === 401
      ? 'La sesión venció. Vuelve a iniciar sesión para sincronizar.'
      : error?.message || 'No se pudo completar la operación.';
  showToast(message, 'error', {duration: 5600});
}

function button(label, run, className = 'secondary compact') {
  const b = element('button', label, className);
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
  const headers = user ? {'X-Finapp-User': user.id} : {};
  if (method !== 'GET') headers['Content-Type'] = 'application/json';
  const response = await fetch(path, {
    method, headers, credentials: 'same-origin', cache: 'no-store',
    ...(data === null ? {} : {body: JSON.stringify(data)}),
  });
  if (!response.ok) {
    const type = response.headers.get('content-type') || '';
    const detail = type.startsWith('text/plain') ? (await response.text()).trim().slice(0, 500) : '';
    const error = new Error(detail || ({
      400: 'Revisa los datos del formulario.',
      401: 'La sesión venció. Vuelve a iniciar sesión.',
      403: 'Esta sesión no tiene acceso a la solicitud.',
      409: 'Los datos cambiaron. Actualiza la vista y vuelve a intentarlo.',
      413: 'El archivo supera el tamaño admitido.',
      422: 'Revisa los valores del formulario.',
    })[response.status] || 'No se pudo completar la solicitud (HTTP ' + response.status + ').');
    error.status = response.status;
    if (error.status === 401 && user) $('relogin').hidden = false;
    throw error;
  }
  if (response.status === 204) return null;
  return format === 'blob' ? response.blob() : response.json();
}

const sections = ['summary', 'entry', 'settings', 'credit', 'planning', 'exchange'];
const sectionFromHash = () => sections.includes(location.hash.slice(1)) ? location.hash.slice(1) : 'summary';

function showMore(open) {
  $('more-menu').dataset.open = open ? 'true' : 'false';
  $('more-toggle').setAttribute('aria-expanded', String(open));
}

function updateUserBadge() {
  if (!user) return;
  $('user-label').textContent = user.username;
  $('user-avatar').textContent = initialsOf(user.username);
  $('user-role-meta').textContent = user.is_admin ? 'Administrador' : 'Usuario';
  $('stab-users').hidden = !user.is_admin;
  if (!user.is_admin && activeSettingsTab === 'users') setSettingsTab('accounts');
}

function setSettingsTab(name, {focus = false} = {}) {
  const valid = ['accounts', 'categories', 'users', 'profile'];
  let target = valid.includes(name) ? name : 'accounts';
  if (target === 'users' && !user?.is_admin) target = 'accounts';
  activeSettingsTab = target;
  for (const btn of document.querySelectorAll('#settings-tabs [role="tab"]')) {
    const selected = btn.dataset.settingsTab === target;
    btn.setAttribute('aria-selected', String(selected));
    btn.tabIndex = selected ? 0 : -1;
    if (selected && focus) btn.focus();
  }
  for (const panel of ['accounts', 'categories', 'users', 'profile']) {
    $('spanel-' + panel).hidden = panel !== target;
  }
  const slot = $('settings-primary-cta');
  slot.replaceChildren();
  if (target === 'accounts') {
    slot.append(button('+ Nueva cuenta', () => openAccountDrawer(), ''));
  } else if (target === 'categories') {
    slot.append(button('+ Nueva categoría', () => openCategoryDrawer(), ''));
  } else if (target === 'users' && user?.is_admin) {
    slot.append(button('+ Nuevo usuario', () => openUserDrawer(), ''));
  }
  if (target === 'users' && user?.is_admin && navigator.onLine) loadUsers().catch(() => {});
  if (target === 'profile' && user) renderProfile();
}

function tab(name, {navigate = true, focus = true} = {}) {
  if (navigate && sectionFromHash() !== name) history.pushState(null, '', '#' + name);
  const changed = activeTab !== name;
  activeTab = name;
  const generation = ++tabGeneration;
  const applyDom = () => {
    for (const viewEl of document.querySelectorAll('[data-view]')) viewEl.hidden = viewEl.id !== name;
    for (const link of document.querySelectorAll('[data-tab]')) {
      if (link.dataset.tab === name) link.setAttribute('aria-current', 'page');
      else link.removeAttribute('aria-current');
    }
    $('more-toggle').classList.toggle('current', ['settings', 'exchange'].includes(name));
    showMore(false);
  };
  if (changed && document.startViewTransition) {
    const transition = document.startViewTransition(applyDom);
    if (focus) transition.finished.finally(() => { if (generation === tabGeneration) $(name).querySelector('h1')?.focus(); });
  } else {
    applyDom();
    if (focus) $(name).querySelector('h1')?.focus();
  }
  if (name === 'settings' && user) {
    setSettingsTab(activeSettingsTab);
    renderProfile();
    if (user.is_admin && navigator.onLine) loadUsers().catch(() => {});
  }
  if (!['credit', 'planning', 'exchange'].includes(name) || !user) return;
  const userId = user.id;
  const isCurrent = () => ownsSession(userId) && generation === tabGeneration;
  const context = {
    api: (...args) => {
      if (!isCurrent()) return Promise.reject(new Error('Vuelve a abrir esta sección para continuar.'));
      return api(...args);
    },
    userId, isCurrent, view: confirmed, getView: () => confirmed, today: today(), synchronize,
  };
  $(name).querySelector('[data-awaiting]')?.remove();
  if (!confirmed) {
    const status = element('p', 'Conéctate para cargar los datos iniciales.');
    status.dataset.awaiting = '';
    $(name).prepend(status);
    return;
  }
  if (name === 'credit') renderCredit($('credit-content'), context).catch(showError);
  if (name === 'planning') renderPlanning($('planning-content'), context).catch(showError);
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
  view = {...snapshot, operations: overlayOperations(snapshot.operations, queue)};
  const rejected = queue.filter(item => item.status === 'rejected').length;
  const conflicts = queue.filter(item => item.status === 'conflict').length;
  const waiting = queue.length - rejected - conflicts;
  $('local-changes').hidden = !queue.length;
  $('sync-status').textContent = queue.length
    ? [waiting && waiting + ' pendientes', rejected && rejected + ' rechazados', conflicts && conflicts + ' en conflicto'].filter(Boolean).join(' · ')
    : 'Todo sincronizado';
  renderBalances();
  renderCatalog();
  renderHistory();
  renderQueue();
  fillSelectors();
  if (first && ['credit', 'planning', 'exchange'].includes(activeTab)) tab(activeTab, {navigate: false, focus: false});
}

function renderKpis(totals) {
  const kpis = $('kpis');
  kpis.replaceChildren();
  if (!view.accounts.length) return;
  const net = netWorth(view.accounts, totals);
  let available = 0n, debt = 0n;
  for (const a of view.accounts) {
    const balance = BigInt(totals.get(a.id) ?? 0);
    if (a.kind === 'card') debt += balance; else available += balance;
  }
  const month = today().slice(0, 7);
  const result = monthlyTotals(view.operations, month);
  const monthLabel = new Intl.DateTimeFormat('es-PY', {month: 'long', year: 'numeric', timeZone: 'America/Asuncion'})
    .format(new Date(month + '-15T12:00:00Z'));

  // 1. Vault Hero Card with SVG Cashflow Sparkline
  const vault = element('article', null, 'kpi kpi--vault spotlight-card');
  const vaultHead = element('div', null, 'kpi-head');
  vaultHead.append(element('p', 'Saldo disponible en cuentas', 'label'), element('span', 'Bóveda PYG', 'kpi-pill'));
  const vaultValue = element('p', money(available), 'value' + (available < 0n ? ' negative' : ''));
  if (lastAvailableBalance !== null) vaultValue.dataset.value = String(lastAvailableBalance);
  animateMoney(vaultValue, available);
  lastAvailableBalance = available;

  const vaultMetrics = element('div', null, 'vault-metrics');
  const inMetric = element('span', null, 'vault-metric vault-metric--in');
  inMetric.append(document.createTextNode('Ingresos '), element('strong', '+ ' + money(result.income)));
  const outMetric = element('span', null, 'vault-metric vault-metric--out');
  outMetric.append(document.createTextNode('Gastos '), element('strong', '− ' + money(result.expense)));
  vaultMetrics.append(inMetric, outMetric);

  const series = monthlyCashflowSeries(view.operations, month, today());
  const sparkWrap = element('div', null, 'sparkline-wrap');
  sparkWrap.append(buildSparklineSvg(series));
  const sparkCaption = element('div', null, 'spark-caption');
  sparkCaption.append(
    element('span', 'Flujo acumulado de ' + monthLabel),
    element('span', (result.net >= 0n ? '+ ' : '') + money(result.net))
  );
  sparkWrap.append(sparkCaption);
  vault.append(vaultHead, vaultValue, vaultMetrics, sparkWrap);
  kpis.append(vault);

  // 2. Category Expense Donut Card
  const donutCard = element('article', null, 'kpi donut-card spotlight-card');
  const donutHead = element('div', null, 'kpi-head');
  const donutBadge = element('span', null, 'icon-badge');
  donutBadge.append(icon('bars'));
  donutHead.append(element('p', 'Gastos de ' + monthLabel, 'label'), donutBadge);
  donutCard.append(donutHead);
  const {total: expenseTotal, slices} = expenseCategorySlices(view.operations, view.categories, month);
  if (slices.length) {
    const layout = element('div', null, 'donut-layout');
    const wrap = element('div', null, 'donut-wrap');
    const center = element('div', null, 'donut-center');
    center.append(element('small', 'Total'), element('strong', money(expenseTotal)));
    wrap.append(buildDonutSvg(slices), center);
    const legend = element('div', null, 'donut-legend');
    for (const slice of slices) {
      const item = element('div', null, 'donut-item');
      const dot = element('span', null, 'donut-swatch');
      dot.style.background = slice.color;
      item.append(dot, element('span', slice.name, 'donut-name'), element('span', slice.percent.toFixed(0) + '%', 'donut-pct'));
      legend.append(item);
    }
    layout.append(wrap, legend);
    donutCard.append(layout);
  } else {
    donutCard.append(element('p', 'Sin gastos registrados este mes.', 'detail'));
  }
  kpis.append(donutCard);

  // 3 & 4. Secondary KPIs
  const secondaryKpi = (label, value, tone, detail, iconName) => {
    const card = element('article', null, 'kpi kpi--secondary spotlight-card');
    const head = element('div', null, 'kpi-head');
    const badge = element('span', null, 'icon-badge' + (tone === 'negative' ? ' negative' : ''));
    badge.append(icon(iconName));
    head.append(element('p', label, 'label'), badge);
    card.append(head, element('p', money(value), 'value ' + tone), element('p', detail, 'detail'));
    kpis.append(card);
  };
  secondaryKpi(
    'Saldo neto de cuentas y tarjetas', net, net >= 0n ? 'positive' : 'negative',
    'El patrimonio completo con préstamos está en Planificación.', 'trend'
  );
  secondaryKpi(
    'Deuda de tarjetas', debt, debt > 0n ? 'negative' : '',
    debt > 0n ? 'Saldo pendiente en tarjetas activas' : 'Sin deuda de tarjetas', 'card'
  );
}

function toggleAccountFilter(accountId) {
  const select = $('history-filter').elements.account;
  select.value = select.value === accountId ? '' : accountId;
  currentPage = 1;
  renderBalances();
  renderHistory();
  if (select.value) $('movements-heading').scrollIntoView({behavior: 'smooth', block: 'start'});
}

function renderBalances() {
  const balances = $('balances');
  balances.replaceChildren();
  $('kpis').replaceChildren();
  $('balance-note').textContent = queue.some(item => item.status === 'pending' || item.status === 'conflict')
    ? 'Saldos provisionales con cambios locales.' : '';
  const activeAccount = $('history-filter').elements.account.value;
  try {
    const totals = calculateBalances(view.accounts, view.operations);
    renderKpis(totals);
    for (const a of view.accounts) {
      const isCard = a.kind === 'card';
      const card = element('article', null, 'card balance-card spotlight-card' + (isCard ? ' balance-card--card' : ''));
      card.tabIndex = 0;
      card.setAttribute('role', 'button');
      card.dataset.activeFilter = String(activeAccount === a.id);
      card.setAttribute('aria-pressed', String(activeAccount === a.id));
      card.addEventListener('click', () => toggleAccountFilter(a.id));
      card.addEventListener('keydown', event => {
        if (event.key === 'Enter' || event.key === ' ') {
          event.preventDefault();
          toggleAccountFilter(a.id);
        }
      });

      const balance = totals.get(a.id);
      const color = (isCard ? balance > 0 : balance < 0) ? 'negative' : '';
      const type = accountTypes[a.kind];
      const head = element('div', null, 'balance-card-head');
      if (isCard) {
        head.append(element('span', null, 'card-chip'), pill(type, 'brand'));
      } else {
        const badge = element('span', null, 'icon-badge' + (color === 'negative' ? ' negative' : ''));
        badge.append(icon(a.kind));
        head.append(badge, pill(type));
      }
      card.append(head, element('strong', a.name), element('p', money(balance), 'amount ' + color));
      const meta = element('div', null, 'actions');
      if (a.institution) meta.append(pill(a.institution));
      if (isCard) meta.append(pill(balance < 0 ? 'Saldo a favor' : balance > 0 ? 'Deuda de tarjeta' : 'Al día', balance > 0 ? 'negative' : 'positive'));
      if (a.archived) meta.append(pill('Archivada', 'warning'));
      if (meta.children.length) card.append(meta);
      balances.append(card);
    }
  } catch (error) { $('balance-note').textContent = error.message; }

  const step = setupStep(view);
  if (step === 'ready') return;
  const prompt = element('div', null, 'card setup-card');
  if (step === 'account') {
    prompt.append(element('p', 'Crea tu primera cuenta para empezar a registrar movimientos.'),
      button('Crear mi primera cuenta', () => openAccountDrawer()));
  } else {
    prompt.append(element('p', 'Elige o crea una categoría para clasificar tu primer gasto.'),
      button('Configurar categorías', () => { tab('settings'); setSettingsTab('categories'); $('suggestions-details').open = true; }));
  }
  balances.append(prompt);
}

function previewAmount(input, output, allowZero = false) {
  if (!input || !output) return;
  const raw = input.value.trim();
  if (!raw) { output.textContent = ''; return; }
  const n = Number(raw);
  output.textContent = Number.isSafeInteger(n) && (allowZero ? true : n > 0) ? money(n) : '';
}

function updateAmountPreviews() {
  previewAmount(fields('movement-form').amount, $('amount-preview'));
  previewAmount(fields('account-form').initial_balance, $('initial-balance-preview'), true);
}

function updatePasswordStrength(input, containerId) {
  const box = $(containerId);
  if (!box) return;
  const result = passwordStrength(input.value);
  box.hidden = !result.score;
  box.dataset.score = String(result.score);
  box.dataset.tone = result.tone;
  box.querySelector('.pw-label').textContent = result.label;
}

function renderAccountFields() {
  const card = fields('account-form').kind.value === 'card';
  $('card-details').hidden = !card;
  fields('account-form').closing_day.disabled = !card;
  fields('account-form').due_day.disabled = !card;
}

function openAccountDrawer({item = null, inUse = false, kind = 'bank', targetSelect = ''} = {}) {
  if (!navigator.onLine) {
    notice('Conéctate al servidor para crear o editar cuentas.', 'warning');
    return;
  }
  const form = $('account-form');
  form.reset();
  form.elements.id.value = item?.id ?? '';
  form.elements.target_select.value = targetSelect;
  form.elements.name.value = item?.name ?? '';
  form.elements.kind.value = item?.kind ?? kind;
  form.elements.kind.disabled = Boolean(item && inUse);
  form.elements.initial_balance.value = item?.initial_balance ?? 0;
  form.elements.closing_day.value = item?.closing_day ?? '';
  form.elements.due_day.value = item?.due_day ?? '';
  form.elements.institution.value = item?.institution ?? '';
  form.elements.cutoff_date.value = item?.cutoff_date ?? '';
  form.elements.notes.value = item?.notes ?? '';
  $('account-advanced').open = Boolean(item && (item.institution || item.cutoff_date || item.notes));
  $('account-drawer-title').textContent = item ? 'Editar ' + item.name : kind === 'card' ? 'Nueva tarjeta de crédito' : 'Nueva cuenta';
  $('account-submit').textContent = item ? 'Guardar cambios' : 'Guardar cuenta';
  renderAccountFields();
  previewAmount(form.elements.initial_balance, $('initial-balance-preview'), true);
  $('account-drawer').showModal();
  form.elements.name.focus();
}

function openCategoryDrawer({item = null, inUse = false, kind = 'expense', targetSelect = ''} = {}) {
  if (!navigator.onLine) {
    notice('Conéctate al servidor para crear o editar categorías.', 'warning');
    return;
  }
  const form = $('category-form');
  form.reset();
  form.elements.id.value = item?.id ?? '';
  form.elements.target_select.value = targetSelect;
  form.elements.name.value = item?.name ?? '';
  form.elements.kind.value = item?.kind ?? kind;
  form.elements.kind.disabled = Boolean(item && inUse);
  $('category-drawer-title').textContent = item ? 'Editar ' + item.name : 'Nueva categoría';
  $('category-submit').textContent = item ? 'Guardar cambios' : 'Guardar categoría';
  $('category-drawer').showModal();
  form.elements.name.focus();
}

function openUserDrawer(item = null) {
  if (!navigator.onLine) {
    notice('Conéctate al servidor para administrar usuarios.', 'warning');
    return;
  }
  const form = $('user-form');
  form.reset();
  form.elements.id.value = item?.id ?? '';
  form.elements.username.value = item?.username ?? '';
  form.elements.is_admin.checked = Boolean(item?.is_admin);
  form.elements.password.required = !item;
  form.elements.password.placeholder = item ? 'Dejar vacío para mantener la actual' : '';
  $('user-drawer-title').textContent = item ? 'Editar «' + item.username + '»' : 'Nuevo usuario';
  $('user-password-label').textContent = item ? 'Nueva contraseña (opcional, mín. 12 caracteres)' : 'Contraseña (mínimo 12 caracteres)';
  $('user-submit').textContent = item ? 'Guardar cambios' : 'Crear usuario';
  updatePasswordStrength(form.elements.password, 'user-pw-strength');
  $('user-drawer').showModal();
  form.elements.username.focus();
}

function renderMiniMetrics(container, items) {
  container.replaceChildren();
  for (const [label, value] of items) {
    const card = element('div', null, 'mini-metric');
    card.append(element('span', label), element('strong', String(value)));
    container.append(card);
  }
}

function renderCatalog() {
  if (!view) return;
  let totals = new Map();
  try { totals = calculateBalances(view.accounts, view.operations); } catch {}
  const usedAccounts = new Set();
  const categoryUsage = new Map();
  for (const op of view.operations) {
    if (op.deleted) continue;
    usedAccounts.add(op.account_id);
    if (op.card_account_id) usedAccounts.add(op.card_account_id);
    if (op.destination_account_id) usedAccounts.add(op.destination_account_id);
    if (op.category_id) categoryUsage.set(op.category_id, (categoryUsage.get(op.category_id) ?? 0) + 1);
  }

  $('count-accounts').textContent = String(view.accounts.length);
  $('count-categories').textContent = String(view.categories.length);

  renderMiniMetrics($('accounts-metrics'), [
    ['Total cuentas', view.accounts.length],
    ['Activas', view.accounts.filter(a => !a.archived).length],
    ['Tarjetas', view.accounts.filter(a => a.kind === 'card').length],
    ['Archivadas', view.accounts.filter(a => a.archived).length],
  ]);

  // Accounts list with instant filter
  const query = ($('account-filter')?.value || '').trim().toLocaleLowerCase('es');
  const accountList = $('account-list');
  accountList.replaceChildren();
  const filteredAccounts = view.accounts.filter(item => !query
    || item.name.toLocaleLowerCase('es').includes(query)
    || (item.institution || '').toLocaleLowerCase('es').includes(query)
    || (accountTypes[item.kind] || '').toLocaleLowerCase('es').includes(query));

  if (!filteredAccounts.length) {
    accountList.append(element('p', query ? 'No hay cuentas que coincidan con la búsqueda.' : 'Todavía no tienes cuentas registradas.', 'muted empty-history'));
  }
  for (const item of filteredAccounts) {
    const inUse = usedAccounts.has(item.id);
    const row = element('article', null, 'row catalog-row');
    const lead = element('div', null, 'catalog-lead');
    const badge = element('span', null, 'icon-badge');
    badge.append(icon(item.kind));
    const info = element('div', null, 'catalog-info');
    info.append(element('strong', item.name));
    const meta = element('div', null, 'movement-meta');
    meta.append(pill(accountTypes[item.kind] || item.kind, item.kind === 'card' ? 'brand' : ''));
    if (item.institution) meta.append(pill(item.institution));
    if (item.kind === 'card' && (item.closing_day || item.due_day)) {
      meta.append(pill('Cierre ' + (item.closing_day || '—') + ' · Vto. ' + (item.due_day || '—')));
    }
    if (totals.has(item.id)) meta.append(pill('Saldo ' + money(totals.get(item.id))));
    if (item.archived) meta.append(pill('Archivada', 'warning'));
    info.append(meta);
    lead.append(badge, info);
    row.append(lead);

    const actions = element('div', null, 'actions');
    actions.append(
      button('Editar', () => openAccountDrawer({item, inUse})),
      button(item.archived ? 'Reactivar' : 'Archivar', async () => {
        await api('/api/catalog/archive', 'POST', {entity: 'account', id: item.id, archived: !item.archived});
        await synchronize();
        notice(item.archived ? 'Cuenta reactivada.' : 'Cuenta archivada.');
      })
    );
    if (!inUse) {
      actions.append(button('Eliminar', async () => {
        if (!(await confirmAction('¿Eliminar la cuenta «' + item.name + '»?', 'Eliminar'))) return;
        await api('/api/accounts/' + encodeURIComponent(item.id), 'DELETE', {});
        await synchronize();
        notice('Cuenta eliminada.');
      }, 'danger-ghost compact'));
    }
    row.append(actions);
    accountList.append(row);
  }

  // Categories split into Expenses vs Income
  for (const [kind, listId] of [['expense', 'category-expense-list'], ['income', 'category-income-list']]) {
    const list = $(listId);
    list.replaceChildren();
    const items = view.categories.filter(c => c.kind === kind);
    if (!items.length) {
      list.append(element('p', 'Sin categorías de ' + (kind === 'expense' ? 'gasto' : 'ingreso') + '.', 'muted empty-history'));
      continue;
    }
    for (const item of items) {
      const count = categoryUsage.get(item.id) ?? 0;
      const inUse = count > 0;
      const row = element('article', null, 'row catalog-row');
      const info = element('div', null, 'catalog-info');
      info.append(element('strong', item.name));
      const meta = element('div', null, 'movement-meta');
      meta.append(pill(count ? count + (count === 1 ? ' movimiento' : ' movimientos') : 'Sin uso'));
      if (item.archived) meta.append(pill('Archivada', 'warning'));
      info.append(meta);
      row.append(info);
      const actions = element('div', null, 'actions');
      actions.append(
        button('Editar', () => openCategoryDrawer({item, inUse})),
        button(item.archived ? 'Reactivar' : 'Archivar', async () => {
          await api('/api/catalog/archive', 'POST', {entity: 'category', id: item.id, archived: !item.archived});
          await synchronize();
          notice(item.archived ? 'Categoría reactivada.' : 'Categoría archivada.');
        })
      );
      if (!inUse) {
        actions.append(button('Eliminar', async () => {
          if (!(await confirmAction('¿Eliminar la categoría «' + item.name + '»?', 'Eliminar'))) return;
          await api('/api/categories/' + encodeURIComponent(item.id), 'DELETE', {});
          await synchronize();
          notice('Categoría eliminada.');
        }, 'danger-ghost compact'));
      }
      row.append(actions);
      list.append(row);
    }
  }

  // Suggested categories
  const container = $('suggested-categories');
  const available = suggestions.filter(([id, name, kind]) => !view.categories.some(c =>
    c.id === 'suggested:' + id || (c.kind === kind && c.name.toLocaleLowerCase('es') === name.toLocaleLowerCase('es'))));
  const signature = available.map(row => row[0]).join(',');
  if (container.dataset.signature === signature) return;
  container.replaceChildren();
  container.dataset.signature = signature;
  if (!available.length) container.append(element('p', 'Ya tienes todas las categorías sugeridas.', 'muted'));
  for (const [id, name, kind] of available) {
    const label = element('label', name + ' · ' + movementNames[kind], 'checkbox');
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
  if (items.length !== values.length || items.some((item, i) => item.id !== values[i].value || item.name !== values[i].textContent)) {
    select.replaceChildren(...items.map(item => {
      const option = element('option', item.name);
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
  for (const [field, items] of [['account', view.accounts], ['category', view.categories]]) {
    const select = filters.elements[field];
    fillSelect(select, [{id: '', name: 'Todas'}, ...items], select.value);
  }
  const activeAccountId = filters.elements.account.value;
  const chip = $('active-account-chip');
  const activeAccountObj = activeAccountId ? view.accounts.find(a => a.id === activeAccountId) : null;
  chip.hidden = !activeAccountObj;
  if (activeAccountObj) chip.textContent = 'Cuenta: ' + activeAccountObj.name + ' ✕';

  const page = historyPage(view.operations, Object.fromEntries(new FormData(filters)), currentPage);
  currentPage = page.page;
  $('history-count').textContent = page.total ? page.total + ' movimientos · Página ' + page.page + ' de ' + page.pages : '';
  $('history-pages').hidden = page.pages === 1;
  $('previous-page').disabled = page.page === 1;
  $('next-page').disabled = page.page === page.pages;

  const accounts = new Map(view.accounts.map(a => [a.id, a]));
  const categories = new Map(view.categories.map(c => [c.id, c.name]));
  const original = new Map(view.operations.map(op => [op.id, op]));
  const signClass = op => op.kind === 'income' || op.kind === 'refund' || op.kind === 'loan_draw' ? 'positive'
    : op.kind === 'transfer' ? '' : 'negative';
  const signPrefix = op => op.kind === 'income' || op.kind === 'refund' || op.kind === 'loan_draw' ? '+ '
    : op.kind === 'transfer' ? '' : '− ';
  const iconKind = op => op.kind.startsWith('loan_') ? 'loan' : op.kind;

  // Group rows by date with relative headers and daily net flow
  const groups = [];
  for (const op of page.rows) {
    let group = groups.at(-1);
    if (!group || group.date !== op.date) {
      group = {date: op.date, rows: [], net: 0n};
      groups.push(group);
    }
    group.rows.push(op);
    if (op.kind === 'income' || op.kind === 'refund') group.net += BigInt(op.amount);
    else if (op.kind === 'expense') group.net -= BigInt(op.amount);
  }

  const todayIso = today();
  for (const group of groups) {
    const head = element('div', null, 'ledger-date-head');
    head.append(element('span', formatRelativeDate(group.date, todayIso)));
    if (group.net !== 0n) {
      head.append(element('span', (group.net > 0n ? '+ ' : '− ') + money(group.net < 0n ? -group.net : group.net),
        'ledger-date-net ' + (group.net > 0n ? 'positive' : 'negative')));
    }
    list.append(head);

    for (const op of group.rows) {
      const row = element('article', null, 'row movement-row');
      const account = accounts.get(op.account_id);
      const destination = accounts.get(op.destination_account_id || op.card_account_id);
      const kind = movementNames[op.kind] || op.kind;
      const avatar = element('span', null, 'movement-icon movement-icon--' + iconKind(op));
      avatar.append(icon(iconKind(op)));
      const description = element('div', null, 'movement-description');
      description.append(element('strong', op.description || kind));
      const meta = element('div', null, 'movement-meta');
      const route = [account?.name, destination?.name].filter(Boolean).join(' → ');
      if (route) meta.append(pill(route));
      const cat = categories.get(op.category_id);
      if (cat) meta.append(pill(cat, op.kind === 'income' ? 'positive' : 'brand'));
      if (op.description && !cat) meta.append(pill(kind));
      description.append(meta);
      if (op.original_operation_id) {
        const expense = original.get(op.original_operation_id);
        description.append(element('p', 'Gasto original · ' + [expense?.date, expense?.description].filter(Boolean).join(' · '), 'muted'));
      }
      if ([account, destination].some(a => a?.cutoff_date && op.date < a.cutoff_date)) {
        description.append(element('p', 'Movimiento anterior a la fecha de corte de una cuenta.', 'muted'));
      }
      if (op.pending) description.append(pill(op.conflict ? 'Conflicto por resolver' : 'Pendiente de sincronización', op.conflict ? 'warning' : 'brand'));
      row.append(avatar, description, element('strong', signPrefix(op) + money(op.amount), 'amount ' + signClass(op)));
      if (!op.conflict && !op.id.startsWith('loan:') && !op.kind.startsWith('loan_')
        && !queue.some(q => q.recordId === op.id && q.status === 'rejected')) {
        const actions = element('div', null, 'row-actions');
        actions.append(
          button('Duplicar', () => duplicate(op), 'secondary compact'),
          button('Editar', () => edit(op), 'secondary compact'),
          button('Eliminar', () => remove(op), 'danger-ghost compact')
        );
        row.append(actions);
      }
      list.append(row);
    }
  }
  if (!page.total) {
    list.append(element('p', view.operations.some(op => !op.deleted)
      ? 'Sin movimientos para estos filtros.' : 'Todavía no hay movimientos.', 'muted empty-history'));
  }
}

function describe(op) {
  if (!op || op.deleted) return 'Movimiento eliminado';
  const account = view.accounts.find(a => a.id === op.account_id)?.name;
  return [movementNames[op.kind], op.date, op.amount != null ? money(op.amount) : '', account, op.description, op.notes]
    .filter(Boolean).join(' · ');
}

async function changeQueue(action) {
  const local = store;
  const id = user?.id;
  if (!navigator.locks?.request) throw new Error('El navegador no permite guardar cambios de forma segura entre pestañas.');
  await navigator.locks.request('misfinanzas-sync:' + id, async () => {
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
    const row = element('article', null, 'row');
    row.append(element('strong', item.action === 'delete' ? 'Eliminar movimiento' : describe(item.data)));
    const actions = element('div', null, 'actions');
    const hasDependents = queue.some(op => op.dependsOn === item.id);
    const discard = () => confirmAction(hasDependents
      ? '¿Descartar este cambio y los cambios locales posteriores sobre el mismo movimiento?'
      : '¿Descartar este cambio local?', 'Descartar');
    if (item.status === 'conflict') {
      row.classList.add('conflict');
      row.append(element('p', 'Versión del servidor · ' + (item.server ? describe(item.server) : 'Movimiento inexistente')));
      const resolve = async choice => {
        if (hasDependents && !(await discard())) return;
        await changeQueue(local => choice === 'server'
          ? local.acceptServer(item.id, {discardDependents: hasDependents})
          : local.resolveConflict(item.id, {
            action: item.server == null ? 'create' : item.action === 'create' ? 'update' : item.action,
            data: item.data,
          }, {discardDependents: hasDependents}));
      };
      actions.append(button('Conservar versión del servidor', () => resolve('server')));
      if (item.server || item.action !== 'delete') actions.append(button('Conservar mi cambio', () => resolve('local')));
      conflicts.append(row);
    } else {
      row.append(element('p', item.status === 'rejected'
        ? item.rejection.message
        : item.dependsOn ? 'En espera del cambio anterior sobre este movimiento.' : 'Pendiente de sincronización.', 'muted'));
      if (item.status === 'rejected') {
        if (item.data) actions.append(button('Corregir', () => edit({...item.data, id: item.recordId, version: item.baseVersion}, item.id)));
        actions.append(button('Reintentar', () => changeQueue(local => local.retryRejected(item.id))));
      }
      actions.append(button('Descartar cambio', async () => {
        if (await discard()) await changeQueue(local => local.discardOperation(item.id, {discardDependents: hasDependents}));
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
  for (const btn of document.querySelectorAll('#kind-pills .kind-pill')) {
    btn.setAttribute('aria-pressed', String(btn.dataset.kind === kind));
  }
  const accounts = view.accounts;
  const editing = confirmed.operations.find(op => op.id === form.elements.id.value);
  const allowed = (item, field) => !item.archived || item.id === editing?.[field];
  const paired = kind === 'transfer' || kind === 'payment';
  const options = [
    ['account_id', accounts.filter(a => allowed(a, 'account_id') && (!paired || a.kind !== 'card'))],
    ['category_id', view.categories.filter(c => allowed(c, 'category_id') && c.kind === kind)],
    ['card_account_id', accounts.filter(a => allowed(a, 'card_account_id') && a.kind === 'card')],
    ['destination_account_id', accounts.filter(a => allowed(a, 'destination_account_id') && a.kind !== 'card')],
  ];
  for (const [id, items] of options) {
    const select = form.elements[id];
    fillSelect(select, id === 'destination_account_id' ? items.filter(a => a.id !== form.elements.account_id.value) : items, select.value);
  }
  const refundTo = kind === 'refund' && accounts.find(a => a.id === form.elements.account_id.value);
  const expenses = refundTo ? confirmed.operations.filter(op => op.kind === 'expense' && !op.deleted
    && (refundTo.kind !== 'card' || op.account_id === refundTo.id))
    .toSorted((a, b) => b.date.localeCompare(a.date) || b.id.localeCompare(a.id)) : [];
  const original = form.elements.original_operation_id;
  fillSelect(original, expenses.map(op => ({id: op.id, name: [op.date, op.description || 'Gasto', money(op.amount)].join(' · ')})), original.value);
  for (const [label, field, visible] of [
    ['category-label', 'category_id', kind === 'income' || kind === 'expense'],
    ['card-label', 'card_account_id', kind === 'payment'],
    ['destination-label', 'destination_account_id', kind === 'transfer'],
    ['original-label', 'original_operation_id', kind === 'refund'],
  ]) {
    $(label).hidden = !visible;
    form.elements[field].disabled = !visible;
    form.elements[field].required = visible;
  }

  const hint = $('entry-needed');
  const missing = [
    ['account_id', 'Crea una cuenta para este movimiento.', 'Crear cuenta', () => openAccountDrawer({targetSelect: 'account_id'})],
    ['category_id', 'Crea una categoría de ' + movementNames[kind]?.toLowerCase() + '.', 'Crear categoría', () => openCategoryDrawer({kind: kind === 'income' ? 'income' : 'expense', targetSelect: 'category_id'})],
    ['card_account_id', 'Crea la tarjeta que quieres pagar.', 'Crear tarjeta', () => openAccountDrawer({kind: 'card', targetSelect: 'card_account_id'})],
    ['destination_account_id', 'Crea otra cuenta para recibir la transferencia.', 'Crear cuenta de destino', () => openAccountDrawer({targetSelect: 'destination_account_id'})],
    ['original_operation_id', 'Selecciona un gasto ya sincronizado.', 'Registrar gasto', () => { resetEntry(); tab('entry'); }],
  ].find(([name]) => !form.elements[name].disabled && !form.elements[name].options.length);
  hint.replaceChildren();
  if (missing) hint.append(element('p', missing[1]), button(missing[2], missing[3]));
  hint.hidden = !missing;
  const disabled = !!missing || !!form.dataset.busy;
  $('save-movement').disabled = disabled;
  $('save-another').disabled = disabled;
  $('save-another').hidden = Boolean(form.elements.id.value);
  cutoffHint();
}

function cutoffHint() {
  if (!view) return;
  const f = fields('movement-form');
  const ids = [f.account_id, f.destination_account_id, f.card_account_id].filter(input => !input.disabled).map(input => input.value);
  const before = view.accounts.filter(a => ids.includes(a.id) && a.cutoff_date && f.date.value && f.date.value < a.cutoff_date);
  $('entry-cutoff').hidden = !before.length;
  $('entry-cutoff').textContent = before.length
    ? 'Fecha anterior al corte. Se guarda en el historial y queda fuera del saldo de ' + before.map(a => a.name).join(', ') + '.' : '';
}

function tail(id) {
  const own = queue.filter(op => op.recordId === id);
  return own.find(op => !own.some(other => other.dependsOn === op.id));
}

function edit(op, rejectedId = null) {
  editBase = {version: op.version ?? 0, tail: tail(op.id)?.id ?? null, rejectedId};
  const form = $('movement-form');
  form.reset();
  form.elements.id.value = op.id;
  form.elements.kind.value = op.kind;
  fillSelectors();
  if ([...form.elements.account_id.options].some(opt => opt.value === op.account_id)) form.elements.account_id.value = op.account_id;
  fillSelectors();
  for (const name of ['amount', 'category_id', 'card_account_id', 'destination_account_id', 'original_operation_id', 'date', 'description', 'notes']) {
    if (op[name] != null) form.elements[name].value = op[name];
  }
  fillSelectors();
  updateAmountPreviews();
  $('entry-heading').textContent = rejectedId ? 'Corregir movimiento rechazado' : 'Editar movimiento';
  $('cancel-edit').hidden = false;
  tab('entry');
}

function duplicate(op) {
  resetEntry();
  const form = $('movement-form');
  form.elements.kind.value = op.kind;
  fillSelectors();
  if ([...form.elements.account_id.options].some(opt => opt.value === op.account_id)) form.elements.account_id.value = op.account_id;
  fillSelectors();
  for (const name of ['amount', 'category_id', 'card_account_id', 'destination_account_id', 'original_operation_id', 'description', 'notes']) {
    if (op[name] != null) form.elements[name].value = op[name];
  }
  form.elements.date.value = today();
  fillSelectors();
  updateAmountPreviews();
  $('entry-heading').textContent = 'Nuevo movimiento';
  tab('entry');
}

async function remove(op) {
  const editTail = tail(op.id)?.id ?? null;
  if (!(await confirmAction('¿Eliminar este movimiento?', 'Eliminar'))) return;
  await changeQueue(async local => {
    queue = await local.pending();
    const previous = tail(op.id);
    checkEditBase(editTail, previous?.id ?? null);
    await local.enqueue({recordId: op.id, action: 'delete', data: null, baseVersion: previous ? null : op.version, dependsOn: previous?.id ?? null});
  });
  notice('Movimiento eliminado.');
}

function resetEntry({keepContext = false} = {}) {
  editBase = null;
  const f = fields('movement-form');
  const saved = {
    kind: f.kind.value || 'expense',
    account_id: f.account_id.value,
    category_id: f.category_id.value,
    card_account_id: f.card_account_id.value,
    destination_account_id: f.destination_account_id.value,
    date: f.date.value,
  };
  $('movement-form').reset();
  f.id.value = '';
  if (keepContext) f.kind.value = saved.kind;
  f.date.value = keepContext && saved.date ? saved.date : today();
  $('entry-heading').textContent = 'Nuevo movimiento';
  $('cancel-edit').hidden = true;
  fillSelectors();
  if (saved.account_id && [...f.account_id.options].some(opt => opt.value === saved.account_id)) {
    f.account_id.value = saved.account_id;
    fillSelectors();
  }
  if (keepContext) {
    for (const key of ['category_id', 'card_account_id', 'destination_account_id']) {
      if (saved[key] && [...f[key].options].some(opt => opt.value === saved[key])) f[key].value = saved[key];
    }
    cutoffHint();
  }
  updateAmountPreviews();
}

function renderProfile() {
  if (!user) return;
  $('profile-form').reset();
  fields('profile-form').username.value = user.username;
  $('profile-role').textContent = user.is_admin ? 'Administrador' : 'Usuario';
  updatePasswordStrength(fields('profile-form').new_password, 'profile-pw-strength');
}

async function loadUsers() {
  if (!user?.is_admin || !navigator.onLine) return;
  const currentId = user.id;
  const users = await api('/api/users');
  if (!ownsSession(currentId)) return;
  $('count-users').textContent = String(users.length);
  const adminCount = users.filter(u => u.is_admin).length;
  const sessionsTotal = users.reduce((sum, u) => sum + u.active_sessions, 0);
  renderMiniMetrics($('users-metrics'), [
    ['Usuarios', users.length],
    ['Administradores', adminCount],
    ['Sesiones activas', sessionsTotal],
  ]);

  const list = $('user-list');
  list.replaceChildren();
  for (const item of users) {
    const row = element('article', null, 'row catalog-row');
    const lead = element('div', null, 'catalog-lead');
    lead.append(element('span', initialsOf(item.username), 'avatar-badge'));
    const info = element('div', null, 'catalog-info');
    info.append(element('strong', item.username));
    const meta = element('div', null, 'movement-meta');
    meta.append(pill(item.is_admin ? 'Administrador' : 'Usuario', item.is_admin ? 'brand' : ''));
    if (item.id === user.id) meta.append(pill('Sesión actual', 'positive'));
    meta.append(pill(item.active_sessions + (item.active_sessions === 1 ? ' sesión activa' : ' sesiones activas')));
    meta.append(pill(item.operations_count + (item.operations_count === 1 ? ' movimiento' : ' movimientos')));
    info.append(meta);
    lead.append(info);
    row.append(lead);

    const actions = element('div', null, 'actions');
    actions.append(button('Editar / Clave', () => openUserDrawer(item)));
    if (!item.is_admin || adminCount > 1) {
      actions.append(button(item.is_admin ? 'Quitar admin' : 'Hacer admin', async () => {
        const updated = await api('/api/users/' + encodeURIComponent(item.id), 'PATCH', {is_admin: !item.is_admin});
        if (updated.id === user.id) {
          user = updated;
          localStorage.setItem(sessionKey, JSON.stringify(user));
          updateUserBadge();
          renderProfile();
        }
        await loadUsers();
        notice('Permisos de «' + updated.username + '» actualizados.');
      }));
    }
    if (item.id !== user.id) {
      actions.append(button('Eliminar', async () => {
        if (!(await confirmAction('¿Eliminar al usuario «' + item.username + '» y todos sus registros?', 'Eliminar usuario'))) return;
        await api('/api/users/' + encodeURIComponent(item.id), 'DELETE', {});
        await loadUsers();
        notice('Usuario «' + item.username + '» eliminado.');
      }, 'danger-ghost compact'));
    }
    row.append(actions);
    list.append(row);
  }
}

// Command Palette (Ctrl+K)
let cmdItems = [];
let cmdIndex = 0;

function buildCommands() {
  const list = [
    {group: 'Acciones rápidas', label: 'Nuevo movimiento', icon: 'plus', shortcut: 'N', run: () => { tab('entry'); fields('movement-form').amount.focus(); }},
    {group: 'Acciones rápidas', label: 'Nueva cuenta', icon: 'bank', run: () => openAccountDrawer()},
    {group: 'Acciones rápidas', label: 'Nueva tarjeta de crédito', icon: 'card', run: () => openAccountDrawer({kind: 'card'})},
    {group: 'Acciones rápidas', label: 'Nueva categoría', icon: 'sparkles', run: () => openCategoryDrawer()},
    ...(user?.is_admin ? [{group: 'Acciones rápidas', label: 'Nuevo usuario (Admin)', icon: 'user', run: () => openUserDrawer()}] : []),
    {group: 'Acciones rápidas', label: 'Sincronizar ahora', icon: 'trend', run: () => synchronize().catch(showError)},
    {group: 'Apariencia', label: 'Cambiar a tema Claro Alabastro', icon: 'sparkles', run: () => applyTheme('light')},
    {group: 'Apariencia', label: 'Cambiar a tema Obsidiana Oscuro', icon: 'sparkles', run: () => applyTheme('dark')},
    {group: 'Secciones', label: 'Ir a Resumen', icon: 'bars', run: () => tab('summary')},
    {group: 'Secciones', label: 'Ir a Créditos', icon: 'card', run: () => tab('credit')},
    {group: 'Secciones', label: 'Ir a Planificación', icon: 'trend', run: () => tab('planning')},
    {group: 'Secciones', label: 'Ir a Configuración', icon: 'shield', run: () => tab('settings')},
    {group: 'Secciones', label: 'Ir a Importar y exportar', icon: 'copy', run: () => tab('exchange')},
  ];
  for (const a of view?.accounts ?? []) {
    list.push({
      group: 'Cuentas',
      label: 'Filtrar movimientos de ' + a.name,
      icon: a.kind,
      run: () => {
        tab('summary');
        $('history-filter').elements.account.value = a.id;
        currentPage = 1;
        renderBalances();
        renderHistory();
      },
    });
  }
  return list;
}

function renderCommandResults() {
  const q = $('command-input').value.trim().toLocaleLowerCase('es');
  cmdItems = buildCommands().filter(item => !q
    || item.label.toLocaleLowerCase('es').includes(q)
    || item.group.toLocaleLowerCase('es').includes(q));
  cmdIndex = Math.min(cmdIndex, Math.max(0, cmdItems.length - 1));
  const container = $('command-results');
  container.replaceChildren();
  if (!cmdItems.length) {
    container.append(element('p', 'No se encontraron comandos para esa búsqueda.', 'cmd-empty'));
    return;
  }
  let lastGroup = '';
  cmdItems.forEach((item, index) => {
    if (item.group !== lastGroup) {
      lastGroup = item.group;
      container.append(element('div', item.group, 'cmd-group-label'));
    }
    const btn = element('button', null, 'cmd-item');
    btn.type = 'button';
    btn.setAttribute('role', 'option');
    btn.setAttribute('aria-selected', String(index === cmdIndex));
    const left = element('span', null, 'cmd-item-left');
    left.append(icon(item.icon), element('span', item.label));
    btn.append(left);
    if (item.shortcut) btn.append(element('kbd', item.shortcut));
    btn.addEventListener('click', async () => {
      await closeTopLayer($('command-dialog'));
      item.run();
    });
    container.append(btn);
  });
}

function openCommandPalette() {
  if (!user) return;
  $('command-input').value = '';
  cmdIndex = 0;
  renderCommandResults();
  $('command-dialog').showModal();
  $('command-input').focus();
}

function synchronize() {
  if (!store || !navigator.onLine) {
    setConnectionStatus(navigator.onLine ? 'Con conexión' : 'Sin conexión', !navigator.onLine);
    return Promise.resolve(false);
  }
  if (syncRun?.store === store) return syncRun.promise;
  const local = store;
  setConnectionStatus('Sincronizando…', false);
  const promise = (async () => {
    try {
      await syncUser(local, local.userId);
      if (store !== local || !ownsSession(local.userId)) return false;
      $('relogin').hidden = true;
      setConnectionStatus('Conectado', false);
      return true;
    } catch (error) {
      if (store === local) {
        setConnectionStatus(navigator.onLine ? 'Sin sincronizar' : 'Sin conexión', !navigator.onLine);
        if (navigator.onLine) showError(error);
      }
      return false;
    } finally {
      if (syncRun?.store === local) syncRun = null;
      if (store === local) await render();
    }
  })();
  syncRun = {store: local, promise};
  return promise;
}

async function openSession(identity) {
  const local = await openUserStore(identity.id);
  user = identity;
  store = local;
  currentPage = 1;
  lastAvailableBalance = null;
  $('history-filter').reset();
  lastActivity = Date.now();
  localStorage.setItem(sessionKey, JSON.stringify(identity));
  try { localStorage.setItem(activityKey, String(lastActivity)); } catch {}
  updateUserBadge();
  $('login-view').hidden = true;
  $('app-view').hidden = false;
  $('relogin').hidden = false;
  renderProfile();
  resetEntry();
  scheduleIdleLogout();
  updateCountdownDisplay();
  tab(sectionFromHash(), {navigate: false});
  await render();
  await synchronize();
  if (user?.is_admin && navigator.onLine) loadUsers().catch(() => {});
}

function clearSession() {
  ++tabGeneration;
  clearTimeout(idleTimer);
  idleTimer = null;
  lastAvailableBalance = null;
  store?.close();
  user = store = view = confirmed = null;
  queue = [];
  for (const id of [
    'credit-content', 'planning-content', 'data-tools', 'reconciliation',
    'balances', 'movements', 'pending-list', 'conflicts',
    'account-list', 'category-expense-list', 'category-income-list', 'user-list',
    'accounts-metrics', 'users-metrics',
  ]) $(id)?.replaceChildren();
  for (const dlgId of ['account-drawer', 'category-drawer', 'user-drawer', 'command-dialog', 'confirm-dialog']) {
    const dlg = $(dlgId);
    if (dlg?.open) dlg.close();
  }
  $('local-changes').hidden = true;
  $('sync-status').textContent = '';
  $('history-filter').reset();
  $('profile-form').reset();
  $('login-form').reset();
  $('app-view').hidden = true;
  $('login-view').hidden = false;
  $('cancel-login').hidden = true;
  resetEntry();
}

function bindForm(id, run, {onError = showError, onBusy} = {}) {
  const form = $(id);
  const clear = field => {
    if (!field?.hasAttribute?.('aria-errormessage')) return;
    $(field.getAttribute('aria-errormessage'))?.remove();
    field.removeAttribute('aria-errormessage');
    field.removeAttribute('aria-invalid');
  };
  form.addEventListener('invalid', ({target: field}) => {
    clear(field);
    const error = element('span', field.validationMessage, 'field-error');
    error.id = id + '-' + field.name + '-error';
    field.after(error);
    field.setAttribute('aria-invalid', 'true');
    field.setAttribute('aria-errormessage', error.id);
  }, true);
  form.addEventListener('input', ({target: field}) => clear(field));
  form.addEventListener('reset', () => Array.from(form.elements).forEach(clear));
  form.addEventListener('submit', async event => {
    event.preventDefault();
    const submits = form.querySelectorAll('button:not([type="button"])');
    if (form.dataset.busy) return;
    form.dataset.busy = 'true';
    for (const s of submits) s.disabled = true;
    onBusy?.(true);
    try { await run(new FormData(form), form, event.submitter); }
    catch (error) { onError(error); }
    finally {
      delete form.dataset.busy;
      for (const s of submits) s.disabled = false;
      onBusy?.(false);
      if (id === 'movement-form') fillSelectors();
    }
  });
}

// Init visual effects & animated top-layer dialogs
initSpotlight();
for (const dlgId of ['account-drawer', 'category-drawer', 'user-drawer', 'command-dialog', 'confirm-dialog']) {
  const dlg = $(dlgId);
  bindAnimatedDialog(dlg);
  if (dlgId !== 'confirm-dialog') {
    dlg.addEventListener('click', event => {
      if (event.target === dlg) closeTopLayer(dlg);
    });
  }
}
document.querySelectorAll('[data-close-drawer]').forEach(btn =>
  btn.addEventListener('click', () => closeTopLayer($(btn.dataset.closeDrawer)))
);
$('account-cancel').addEventListener('click', () => closeTopLayer($('account-drawer')));
$('category-cancel').addEventListener('click', () => closeTopLayer($('category-drawer')));
$('user-cancel').addEventListener('click', () => closeTopLayer($('user-drawer')));

const loginForm = $('login-form');
const loginPassword = $('login-password');
const togglePassword = $('toggle-password');

function showPassword(visible) {
  loginPassword.type = visible ? 'text' : 'password';
  togglePassword.textContent = visible ? 'Ocultar' : 'Mostrar';
  togglePassword.setAttribute('aria-pressed', String(visible));
  togglePassword.setAttribute('aria-label', visible ? 'Ocultar contraseña' : 'Mostrar contraseña');
}
togglePassword.addEventListener('click', () => showPassword(loginPassword.type === 'password'));
loginForm.addEventListener('input', () => { $('login-error').textContent = ''; });
loginForm.addEventListener('reset', () => {
  showPassword(false);
  $('login-error').textContent = '';
});

bindForm('login-form', async data => {
  const credentials = Object.fromEntries(data);
  if (user && credentials.username.trim().toLowerCase() !== user.username.toLowerCase()) {
    throw new Error('Usa el usuario de los datos guardados en este dispositivo.');
  }
  const identity = await api('/api/login', 'POST', credentials);
  $('login-form').reset();
  if (!store) await openSession(identity);
  else if (identity.id === user.id) {
    $('login-view').hidden = true;
    $('app-view').hidden = false;
    $(activeTab).querySelector('h1')?.focus();
    await synchronize();
  }
}, {
  onError(error) {
    $('login-error').textContent = error instanceof TypeError
      ? 'No se pudo conectar. Revisa tu conexión y vuelve a intentarlo.'
      : error.status === 401
        ? 'El usuario o la contraseña no coinciden.'
        : error.status === 429
          ? 'Hubo demasiados intentos. Espera un minuto y vuelve a intentarlo.'
          : error.message || 'No se pudo iniciar sesión. Vuelve a intentarlo.';
  },
  onBusy(busy) {
    if (busy) $('login-error').textContent = '';
    $('login-submit').textContent = busy ? 'Entrando…' : 'Entrar';
    $('login-progress').textContent = busy ? 'Iniciando sesión…' : '';
    $('cancel-login').disabled = busy;
  },
});

$('relogin').addEventListener('click', () => {
  $('login-view').hidden = false;
  $('app-view').hidden = true;
  $('cancel-login').hidden = false;
  fields('login-form').username.value = user.username;
  fields('login-form').password.focus();
});
$('cancel-login').addEventListener('click', () => {
  $('login-view').hidden = true;
  $('app-view').hidden = false;
  $('login-form').reset();
  $('relogin').focus();
});
$('logout').addEventListener('click', async () => {
  try {
    if (!navigator.onLine) throw new Error('Conéctate para cerrar la sesión.');
    if (!navigator.locks?.request) throw new Error('El navegador no permite cerrar sesión de forma segura entre pestañas.');
    await synchronize();
    const local = store;
    const id = user?.id;
    if (!local || !id) return;
    await navigator.locks.request('misfinanzas-sync:' + id, async () => {
      if (!ownsSession(id) || local !== store) return;
      const remaining = await local.pending();
      if (remaining.length && !(await confirmAction('Hay cambios locales sin sincronizar. ¿Descartarlos y cerrar la sesión?', 'Cerrar sesión'))) return;
      await api('/api/logout', 'POST', {});
      await local.clear({discardPending: remaining.length > 0});
      localStorage.removeItem(sessionKey);
      localStorage.removeItem(activityKey);
      clearSession();
      $('login-username').focus();
    });
  } catch (error) { showError(error); }
});

// Navigation & Subtabs
document.querySelectorAll('[data-tab]').forEach(link => link.addEventListener('click', event => {
  if (event.ctrlKey || event.metaKey || event.shiftKey || event.altKey) return;
  event.preventDefault();
  tab(link.dataset.tab);
}));
$('more-toggle').addEventListener('click', () => showMore($('more-menu').dataset.open !== 'true'));
document.addEventListener('click', event => {
  if (!event.target.closest('#more-menu, #more-toggle')) showMore(false);
});

const settingsTabButtons = [...document.querySelectorAll('#settings-tabs [role="tab"]')];
settingsTabButtons.forEach(btn => btn.addEventListener('click', () => setSettingsTab(btn.dataset.settingsTab)));
$('settings-tabs').addEventListener('keydown', event => {
  const visible = settingsTabButtons.filter(b => !b.hidden);
  const current = visible.indexOf(document.activeElement);
  if (current === -1) return;
  let next = -1;
  if (event.key === 'ArrowRight') next = (current + 1) % visible.length;
  else if (event.key === 'ArrowLeft') next = (current - 1 + visible.length) % visible.length;
  else if (event.key === 'Home') next = 0;
  else if (event.key === 'End') next = visible.length - 1;
  if (next >= 0) {
    event.preventDefault();
    setSettingsTab(visible[next].dataset.settingsTab, {focus: true});
  }
});

document.querySelectorAll('[data-open-category]').forEach(btn =>
  btn.addEventListener('click', () => openCategoryDrawer({kind: btn.dataset.openCategory}))
);
$('account-filter').addEventListener('input', renderCatalog);

// Command Palette shortcuts & keyboard handling
$('open-cmd').addEventListener('click', openCommandPalette);
$('mobile-cmd').addEventListener('click', openCommandPalette);
$('command-input').addEventListener('input', () => { cmdIndex = 0; renderCommandResults(); });
$('command-input').addEventListener('keydown', async event => {
  if (event.key === 'ArrowDown') {
    event.preventDefault();
    if (cmdItems.length) { cmdIndex = (cmdIndex + 1) % cmdItems.length; renderCommandResults(); }
  } else if (event.key === 'ArrowUp') {
    event.preventDefault();
    if (cmdItems.length) { cmdIndex = (cmdIndex - 1 + cmdItems.length) % cmdItems.length; renderCommandResults(); }
  } else if (event.key === 'Enter' && cmdItems[cmdIndex]) {
    event.preventDefault();
    const chosen = cmdItems[cmdIndex];
    await closeTopLayer($('command-dialog'));
    chosen.run();
  }
});

document.addEventListener('keydown', event => {
  if ((event.ctrlKey || event.metaKey) && event.key.toLowerCase() === 'k' && user) {
    event.preventDefault();
    if ($('command-dialog').open) closeTopLayer($('command-dialog'));
    else openCommandPalette();
  }
  if (event.key === 'Escape' && $('more-menu').dataset.open === 'true') {
    showMore(false);
    $('more-toggle').focus();
  }
});

// Movement Studio quick chips, Shift+Enter & pills
document.querySelectorAll('#kind-pills .kind-pill').forEach(btn => btn.addEventListener('click', () => {
  fields('movement-form').kind.value = btn.dataset.kind;
  fillSelectors();
}));
document.querySelectorAll('[data-pyg-add]').forEach(btn => btn.addEventListener('click', () => {
  const input = fields('movement-form').amount;
  const current = Number(input.value) || 0;
  input.value = String(current + Number(btn.dataset.pygAdd));
  previewAmount(input, $('amount-preview'));
  input.focus();
}));
document.querySelectorAll('[data-pyg-clear]').forEach(btn => btn.addEventListener('click', () => {
  const input = fields('movement-form').amount;
  input.value = '';
  previewAmount(input, $('amount-preview'));
  input.focus();
}));
$('movement-form').addEventListener('keydown', event => {
  if (event.key === 'Enter' && event.shiftKey && event.target.tagName !== 'TEXTAREA' && !$('save-another').hidden) {
    event.preventDefault();
    $('movement-form').requestSubmit($('save-another'));
  }
});

// Theme switcher
const themeKey = 'misfinanzas:theme';
function applyTheme(mode) {
  const valid = ['light', 'dark', 'auto'].includes(mode) ? mode : 'light';
  document.documentElement.dataset.theme = valid;
  try { localStorage.setItem(themeKey, valid); } catch {}
  for (const btn of document.querySelectorAll('#theme-switcher [data-theme-choice]')) {
    btn.setAttribute('aria-pressed', String(btn.dataset.themeChoice === valid));
  }
}
applyTheme((() => { try { return localStorage.getItem(themeKey) || 'light'; } catch { return 'light'; } })());
document.querySelectorAll('#theme-switcher [data-theme-choice]').forEach(btn =>
  btn.addEventListener('click', () => applyTheme(btn.dataset.themeChoice))
);

window.addEventListener('hashchange', () => {
  if (user) tab(sectionFromHash(), {navigate: false});
});
for (const name of ['kind', 'account_id']) fields('movement-form')[name].addEventListener('change', fillSelectors);
for (const name of ['date', 'destination_account_id', 'card_account_id']) fields('movement-form')[name].addEventListener('change', cutoffHint);
fields('movement-form').amount.addEventListener('input', () => previewAmount(fields('movement-form').amount, $('amount-preview')));
fields('account-form').initial_balance.addEventListener('input', () => previewAmount(fields('account-form').initial_balance, $('initial-balance-preview'), true));
fields('account-form').kind.addEventListener('change', renderAccountFields);
fields('profile-form').new_password.addEventListener('input', event => updatePasswordStrength(event.target, 'profile-pw-strength'));
fields('user-form').password.addEventListener('input', event => updatePasswordStrength(event.target, 'user-pw-strength'));

$('quick-account').addEventListener('click', () => openAccountDrawer());
document.querySelectorAll('[data-quick-catalog]').forEach(btn => btn.addEventListener('click', () => {
  const mode = btn.dataset.quickCatalog;
  const targetSelect = btn.dataset.targetSelect;
  if (mode === 'category') {
    const k = fields('movement-form').kind.value === 'income' ? 'income' : 'expense';
    openCategoryDrawer({kind: k, targetSelect});
  } else {
    openAccountDrawer({kind: mode === 'card' ? 'card' : 'bank', targetSelect});
  }
}));
$('cancel-edit').addEventListener('click', () => { resetEntry(); tab('summary'); });
$('active-account-chip').addEventListener('click', () => {
  $('history-filter').elements.account.value = '';
  currentPage = 1;
  renderBalances();
  renderHistory();
});
$('dismiss-storage-warning').addEventListener('click', () => { $('storage-banner').hidden = true; });

// Forms
bindForm('movement-form', async (input, form, submitter) => {
  const f = Object.fromEntries(input);
  const amount = Number(f.amount);
  if (!Number.isSafeInteger(amount) || amount <= 0) throw new Error('Ingresa un importe entero válido.');
  const data = {
    kind: f.kind, amount, account_id: f.account_id, date: f.date,
    category_id: f.category_id || null, card_account_id: f.card_account_id || null,
    destination_account_id: f.destination_account_id || null,
    description: f.description || null, notes: f.notes || null,
  };
  if (f.kind === 'refund') {
    const original = confirmed.operations.find(op => !op.deleted && op.id === f.original_operation_id);
    if (!original) throw new Error('Selecciona un gasto sincronizado.');
    data.original_operation_id = original.id;
    data.category_id = original.category_id;
    const previous = view.operations.filter(op => op.kind === 'refund' && !op.deleted && op.id !== f.id
      && op.original_operation_id === original.id).reduce((total, op) => total + BigInt(op.amount), 0n);
    if (f.date < original.date || previous + BigInt(amount) > BigInt(original.amount)) {
      throw new Error('Verifica la fecha y el importe pendiente de devolución.');
    }
  }
  const base = editBase;
  if (f.id && !base) throw new Error('Vuelve a abrir el movimiento para editarlo.');
  await changeQueue(async local => {
    if (base?.rejectedId) {
      await local.retryRejected(base.rejectedId, {data});
      return;
    }
    queue = await local.pending();
    const previous = tail(f.id);
    if (f.id) checkEditBase(base.tail, previous?.id ?? null);
    if (previous && previous.status !== 'pending') throw new Error('Resuelve primero el cambio anterior.');
    await local.enqueue({
      recordId: f.id || crypto.randomUUID(), action: f.id ? 'update' : 'create', data,
      baseVersion: previous ? null : base?.version ?? null, dependsOn: previous?.id ?? null,
    });
  });
  if (submitter?.dataset.stay === 'true' && !f.id) {
    resetEntry({keepContext: true});
    notice('Movimiento guardado. Puedes cargar el siguiente.');
    fields('movement-form').amount.focus();
  } else {
    resetEntry();
    tab('summary');
    notice(f.id ? 'Movimiento actualizado.' : 'Movimiento guardado.');
  }
});

bindForm('account-form', async (input, form) => {
  const f = Object.fromEntries(input);
  const kind = form.elements.kind.value;
  const initial = Number(f.initial_balance);
  if (!Number.isSafeInteger(initial)) throw new Error('El saldo debe ser un número entero válido.');
  const id = f.id || crypto.randomUUID();
  const isEdit = Boolean(f.id);
  const first = !view || setupStep(view) === 'account';
  await api('/api/accounts', 'POST', {
    id, name: f.name.trim(), kind, initial_balance: initial,
    details: {
      institution: (f.institution || '').trim(), notes: f.notes || '', cutoff_date: f.cutoff_date || null,
      closing_day: kind === 'card' && f.closing_day ? Number(f.closing_day) : null,
      due_day: kind === 'card' && f.due_day ? Number(f.due_day) : null,
    },
  });
  await closeTopLayer($('account-drawer'));
  await synchronize();
  if (f.target_select && fields('movement-form')[f.target_select]) {
    fillSelectors();
    const select = fields('movement-form')[f.target_select];
    if ([...select.options].some(opt => opt.value === id)) {
      select.value = id;
      fillSelectors();
    }
  } else if (first && view && setupStep(view) === 'category') {
    openCategoryDrawer({kind: 'expense'});
  }
  notice(isEdit ? 'Cuenta actualizada.' : 'Cuenta creada.');
});

bindForm('category-form', async (input, form) => {
  const f = Object.fromEntries(input);
  const id = f.id || crypto.randomUUID();
  const isEdit = Boolean(f.id);
  await api('/api/categories', 'POST', {id, name: f.name.trim(), kind: form.elements.kind.value});
  await closeTopLayer($('category-drawer'));
  await synchronize();
  if (f.target_select && fields('movement-form')[f.target_select]) {
    fillSelectors();
    const select = fields('movement-form')[f.target_select];
    if ([...select.options].some(opt => opt.value === id)) {
      select.value = id;
      fillSelectors();
    }
  }
  notice(isEdit ? 'Categoría actualizada.' : 'Categoría creada.');
});

bindForm('suggestions-form', async input => {
  const selected = input.getAll('suggestion');
  if (!selected.length) throw new Error('Elige al menos una categoría.');
  await api('/api/catalog/suggest', 'POST', {selected});
  await synchronize();
  notice('Categorías creadas.');
});

bindForm('profile-form', async data => {
  const f = Object.fromEntries(data);
  const updated = await api('/api/me', 'PATCH', {
    current_password: f.current_password,
    username: f.username.trim(),
    new_password: f.new_password || null,
  });
  user = updated;
  localStorage.setItem(sessionKey, JSON.stringify(user));
  updateUserBadge();
  renderProfile();
  if (user.is_admin) await loadUsers();
  notice('Datos de tu cuenta actualizados.');
});

bindForm('user-form', async (data, form) => {
  const f = Object.fromEntries(data);
  const isAdmin = form.elements.is_admin.checked;
  if (f.id) {
    const updated = await api('/api/users/' + encodeURIComponent(f.id), 'PATCH', {
      username: f.username.trim(),
      password: f.password || null,
      is_admin: isAdmin,
    });
    if (updated.id === user.id) {
      user = updated;
      localStorage.setItem(sessionKey, JSON.stringify(user));
      updateUserBadge();
      renderProfile();
    }
    await closeTopLayer($('user-drawer'));
    await loadUsers();
    notice('Usuario actualizado.');
  } else {
    await api('/api/users', 'POST', {username: f.username.trim(), password: f.password, is_admin: isAdmin});
    await closeTopLayer($('user-drawer'));
    await loadUsers();
    notice('Usuario creado.');
  }
});

// History & Sync controls
$('history-filter').addEventListener('submit', event => event.preventDefault());
$('history-filter').addEventListener('input', () => { currentPage = 1; renderBalances(); renderHistory(); });
$('previous-page').addEventListener('click', () => { currentPage--; renderHistory(); });
$('next-page').addEventListener('click', () => { currentPage++; renderHistory(); });
$('retry-sync').addEventListener('click', () => synchronize().catch(showError));
$('local-changes').addEventListener('click', () => {
  tab('summary', {focus: false});
  ($('pending-title').hidden ? $('conflicts-title') : $('pending-title')).focus();
});
$('quick-entry').addEventListener('click', () => { tab('entry'); fields('movement-form').amount.focus(); });
$('sidebar-new').addEventListener('click', () => { tab('entry'); fields('movement-form').amount.focus(); });

// 5-Minute Idle Auto-Logout & Live Countdown
function syncedActivity() {
  try {
    const stored = Number(localStorage.getItem(activityKey));
    if (Number.isFinite(stored) && stored > lastActivity) lastActivity = stored;
  } catch {}
  return lastActivity;
}

function updateCountdownDisplay() {
  if (!user) return;
  const remainingMs = Math.max(0, IDLE_TIMEOUT_MS - (Date.now() - syncedActivity()));
  const totalSec = Math.ceil(remainingMs / 1000);
  const mm = String(Math.floor(totalSec / 60)).padStart(2, '0');
  const ss = String(totalSec % 60).padStart(2, '0');
  const text = `${mm}:${ss}`;
  const pillEl = $('session-countdown');
  if (pillEl) {
    pillEl.textContent = text;
    pillEl.dataset.warn = String(totalSec <= 60);
  }
  const profEl = $('profile-countdown');
  if (profEl) profEl.textContent = text;
}

async function closeIdleSession() {
  if (!user || closingIdle) return;
  closingIdle = true;
  clearTimeout(idleTimer);
  const local = store;
  try {
    if (navigator.onLine) await api('/api/logout', 'POST', {}).catch(() => {});
    const remaining = local ? await local.pending().catch(() => []) : [];
    if (!remaining.length && local) {
      await local.clear({discardPending: false}).catch(() => {});
    }
  } finally {
    localStorage.removeItem(sessionKey);
    localStorage.removeItem(activityKey);
    clearSession();
    closingIdle = false;
    notice('La sesión se cerró tras 5 minutos de inactividad.', 'warning');
  }
}

function scheduleIdleLogout() {
  clearTimeout(idleTimer);
  if (!user) return;
  const elapsed = Date.now() - syncedActivity();
  if (elapsed >= IDLE_TIMEOUT_MS) {
    closeIdleSession();
    return;
  }
  idleTimer = setTimeout(scheduleIdleLogout, IDLE_TIMEOUT_MS - elapsed + 50);
}

function recordActivity() {
  if (!user) return;
  if (Date.now() - syncedActivity() >= IDLE_TIMEOUT_MS) {
    closeIdleSession();
    return;
  }
  if (!document.hidden) {
    lastActivity = Date.now();
    try { localStorage.setItem(activityKey, String(lastActivity)); } catch {}
    scheduleIdleLogout();
    updateCountdownDisplay();
  }
}

for (const eventName of ['pointerdown', 'keydown', 'wheel', 'touchstart', 'input']) {
  window.addEventListener(eventName, recordActivity, {passive: true});
}
window.addEventListener('focus', recordActivity);
document.addEventListener('visibilitychange', () => {
  if (!user) return;
  if (Date.now() - syncedActivity() >= IDLE_TIMEOUT_MS) closeIdleSession();
  else if (!document.hidden) recordActivity();
  else scheduleIdleLogout();
});

setInterval(() => {
  if (!user) return;
  updateCountdownDisplay();
  if (Date.now() - syncedActivity() >= IDLE_TIMEOUT_MS) closeIdleSession();
}, 1000);

window.addEventListener('online', () => synchronize().catch(showError));
window.addEventListener('offline', () => setConnectionStatus('Sin conexión', true));
window.addEventListener('storage', event => {
  if (event.key === sessionKey && user && !ownsSession(user.id)) {
    clearSession();
    notice('La sesión local cambió en otra pestaña.', 'warning');
  }
  if (event.key === activityKey && user) {
    scheduleIdleLogout();
    updateCountdownDisplay();
  }
});

setInterval(() => {
  if (!user) return;
  const idleMs = Date.now() - syncedActivity();
  if (idleMs >= IDLE_TIMEOUT_MS) {
    closeIdleSession();
    return;
  }
  if (!document.hidden && navigator.onLine && idleMs < 60000) synchronize().catch(showError);
}, 15000);

setConnectionStatus(navigator.onLine ? 'Con conexión' : 'Sin conexión', !navigator.onLine);
if ('serviceWorker' in navigator) {
  const hadController = Boolean(navigator.serviceWorker.controller);
  let reloading = false;
  navigator.serviceWorker.addEventListener('controllerchange', () => {
    if (hadController && !reloading) {
      reloading = true;
      location.reload();
    }
  });
  navigator.serviceWorker.register('/sw.js', {updateViaCache: 'none'}).then(registration => {
    const update = () => { $('reload-app').hidden = !registration.waiting || !navigator.serviceWorker.controller; };
    update();
    registration.update().catch(() => {});
    registration.addEventListener('updatefound', () => registration.installing?.addEventListener('statechange', update));
    $('reload-app').addEventListener('click', async () => {
      if (!registration.waiting) return;
      const pending = store ? await store.pending().catch(() => []) : [];
      if (pending.length && !(await confirmAction('Hay cambios sin sincronizar. Se conservarán después de actualizar. ¿Continuar?', 'Actualizar'))) return;
      registration.waiting.postMessage('ACTIVATE_UPDATE');
    });
    setInterval(() => registration.update().catch(() => {}), 60 * 60 * 1000);
  }).catch(() => notice('No se pudo activar el modo sin conexión.', 'warning'));
}
if (navigator.storage?.persist) {
  navigator.storage.persist().then(granted => {
    $('storage-banner').hidden = granted;
    $('storage-warning').textContent = 'El navegador no garantizó el almacenamiento persistente. Sincroniza tus cambios con frecuencia.';
  }).catch(() => {});
}
const previous = lastIdentity();
const lastSeen = Number(localStorage.getItem(activityKey) || 0);
if (previous?.id && previous?.username) {
  if (lastSeen && Date.now() - lastSeen < IDLE_TIMEOUT_MS) {
    try { await openSession(previous); }
    catch (error) { showError(error); }
  } else {
    localStorage.removeItem(sessionKey);
    localStorage.removeItem(activityKey);
  }
}
