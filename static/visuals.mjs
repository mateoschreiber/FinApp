// Flagship visual helpers: pure SVG chart calculators, Top-Layer animations, spotlight and toasts.
import {element, money} from './ui.mjs';

const CHART_COLORS = [
  'var(--chart-1, #10b981)',
  'var(--chart-2, #f59e0b)',
  'var(--chart-3, #0ea5e9)',
  'var(--chart-4, #8b5cf6)',
  'var(--chart-5, #f43f5e)',
  'var(--chart-6, #64748b)',
];

export function initialsOf(name = '') {
  const parts = String(name).trim().split(/\s+/).filter(Boolean);
  if (!parts.length) return '?';
  if (parts.length === 1) return parts[0].slice(0, 2).toUpperCase();
  return (parts[0][0] + parts[1][0]).toUpperCase();
}

export function passwordStrength(password = '') {
  const value = String(password);
  if (!value) return {score: 0, label: '', tone: ''};
  let score = 0;
  if (value.length >= 12) score++;
  if (value.length >= 16) score++;
  const classes = [/[a-z]/, /[A-Z]/, /\d/, /[^A-Za-z0-9]/].filter(rx => rx.test(value)).length;
  if (classes >= 2) score++;
  if (classes >= 3 && value.length >= 12) score++;
  if (value.length < 12) return {score: 1, label: 'Mínimo 12 caracteres', tone: 'negative'};
  const labels = ['', 'Básica', 'Aceptable', 'Fuerte', 'Muy fuerte'];
  const tones = ['', 'warning', 'warning', 'positive', 'positive'];
  return {score, label: labels[score], tone: tones[score]};
}

export function formatRelativeDate(isoDate, todayIso) {
  if (!isoDate) return '';
  if (isoDate === todayIso) return 'Hoy';
  const [ty, tm, td] = todayIso.split('-').map(Number);
  const yesterday = new Date(Date.UTC(ty, tm - 1, td - 1)).toISOString().slice(0, 10);
  if (isoDate === yesterday) return 'Ayer';
  const [y, m, d] = isoDate.split('-').map(Number);
  const dt = new Date(Date.UTC(y, m - 1, d, 12));
  const formatted = new Intl.DateTimeFormat('es-PY', {
    weekday: 'short', day: 'numeric', month: 'short', year: y === ty ? undefined : 'numeric', timeZone: 'UTC',
  }).format(dt);
  return formatted.charAt(0).toUpperCase() + formatted.slice(1);
}

export function monthlyCashflowSeries(operations, month, todayIso = '') {
  const [y, m] = month.split('-').map(Number);
  const daysInMonth = new Date(Date.UTC(y, m, 0)).getUTCDate();
  const maxDay = todayIso.startsWith(month) ? Math.min(daysInMonth, Number(todayIso.slice(8, 10)) || daysInMonth) : daysInMonth;
  const byDay = new Map();
  for (const op of operations) {
    if (op.deleted || !op.date?.startsWith(month)) continue;
    const day = Number(op.date.slice(8, 10));
    if (!Number.isInteger(day) || day < 1 || day > daysInMonth) continue;
    let delta = 0n;
    if (op.kind === 'income') delta = BigInt(op.amount);
    else if (op.kind === 'expense') delta = -BigInt(op.amount);
    else if (op.kind === 'refund') delta = BigInt(op.amount);
    if (delta !== 0n) byDay.set(day, (byDay.get(day) ?? 0n) + delta);
  }
  const endDay = Math.max(maxDay, ...byDay.keys(), 1);
  let cumulative = 0n;
  let min = 0n;
  let max = 0n;
  const points = [];
  for (let day = 1; day <= endDay; day++) {
    const daily = byDay.get(day) ?? 0n;
    cumulative += daily;
    if (cumulative < min) min = cumulative;
    if (cumulative > max) max = cumulative;
    points.push({day, date: `${month}-${String(day).padStart(2, '0')}`, daily, cumulative});
  }
  return {points, min, max, total: cumulative, endDay};
}

export function expenseCategorySlices(operations, categories, month, limit = 5) {
  const names = new Map(categories.map(c => [c.id, c.name]));
  const totals = new Map();
  for (const op of operations) {
    if (op.deleted || !op.date?.startsWith(month)) continue;
    if (op.kind !== 'expense' && op.kind !== 'refund') continue;
    const id = op.category_id || '__other__';
    const delta = op.kind === 'refund' ? -BigInt(op.amount) : BigInt(op.amount);
    totals.set(id, (totals.get(id) ?? 0n) + delta);
  }
  const positive = [...totals.entries()]
    .filter(([, amount]) => amount > 0n)
    .map(([id, amount]) => ({id, name: names.get(id) || 'Otros gastos', amount}))
    .sort((a, b) => (a.amount === b.amount ? a.name.localeCompare(b.name, 'es') : a.amount > b.amount ? -1 : 1));
  const total = positive.reduce((sum, item) => sum + item.amount, 0n);
  if (total === 0n) return {total: 0n, slices: []};
  const top = positive.slice(0, limit);
  if (positive.length > limit) {
    const rest = positive.slice(limit).reduce((sum, item) => sum + item.amount, 0n);
    top.push({id: '__rest__', name: 'Otras categorías', amount: rest});
  }
  const slices = top.map((item, index) => ({
    ...item,
    percent: Number((item.amount * 1000n) / total) / 10,
    color: CHART_COLORS[index % CHART_COLORS.length],
  }));
  return {total, slices};
}

const SVG_NS = 'http://www.w3.org/2000/svg';
function svgEl(tag, attrs = {}) {
  const node = document.createElementNS(SVG_NS, tag);
  for (const [k, v] of Object.entries(attrs)) node.setAttribute(k, String(v));
  return node;
}

export function buildSparklineSvg(series) {
  const width = 420;
  const height = 76;
  const padY = 8;
  const svg = svgEl('svg', {viewBox: `0 0 ${width} ${height}`, class: 'sparkline-svg', 'aria-hidden': 'true'});
  const defs = svgEl('defs');
  const grad = svgEl('linearGradient', {id: 'vault-spark-fill', x1: '0', y1: '0', x2: '0', y2: '1'});
  grad.append(
    svgEl('stop', {offset: '0%', 'stop-color': 'var(--spark-fill, #34d399)', 'stop-opacity': '0.38'}),
    svgEl('stop', {offset: '100%', 'stop-color': 'var(--spark-fill, #34d399)', 'stop-opacity': '0'})
  );
  defs.append(grad);
  svg.append(defs);

  const pts = series.points;
  const span = Number(series.max - series.min) || 1;
  const coords = pts.map((p, i) => {
    const x = pts.length === 1 ? width : (i / (pts.length - 1)) * width;
    const ratio = (Number(p.cumulative - series.min)) / span;
    const y = height - padY - ratio * (height - padY * 2);
    return [x, y];
  });
  if (coords.length === 1) coords.unshift([0, coords[0][1]]);

  let lineD = `M${coords[0][0].toFixed(1)},${coords[0][1].toFixed(1)}`;
  for (let i = 1; i < coords.length; i++) {
    const [x0, y0] = coords[i - 1];
    const [x1, y1] = coords[i];
    const cx = ((x0 + x1) / 2).toFixed(1);
    lineD += ` C${cx},${y0.toFixed(1)} ${cx},${y1.toFixed(1)} ${x1.toFixed(1)},${y1.toFixed(1)}`;
  }
  const areaD = `${lineD} L${width},${height} L0,${height} Z`;
  const stroke = series.total < 0n ? '#fda4af' : 'var(--spark-stroke, #6ee7b7)';
  svg.append(
    svgEl('path', {d: areaD, fill: 'url(#vault-spark-fill)'}),
    svgEl('path', {d: lineD, fill: 'none', stroke, 'stroke-width': '2.5', 'stroke-linecap': 'round', class: 'spark-path'})
  );
  const last = coords.at(-1);
  svg.append(
    svgEl('circle', {cx: last[0].toFixed(1), cy: last[1].toFixed(1), r: '5', fill: stroke, opacity: '0.28'}),
    svgEl('circle', {cx: last[0].toFixed(1), cy: last[1].toFixed(1), r: '3', fill: stroke})
  );
  return svg;
}

export function buildDonutSvg(slices) {
  const r = 46;
  const c = 2 * Math.PI * r;
  const svg = svgEl('svg', {viewBox: '0 0 120 120', class: 'donut-svg', 'aria-hidden': 'true'});
  svg.append(svgEl('circle', {cx: 60, cy: 60, r, fill: 'none', stroke: 'var(--line)', 'stroke-width': 13}));
  let offset = 0;
  for (const slice of slices) {
    const length = Math.max(2, (slice.percent / 100) * c - 3);
    svg.append(svgEl('circle', {
      cx: 60, cy: 60, r, fill: 'none', stroke: slice.color, 'stroke-width': 13,
      'stroke-dasharray': `${length.toFixed(2)} ${c.toFixed(2)}`,
      'stroke-dashoffset': (-offset).toFixed(2),
      'stroke-linecap': 'round',
      class: 'donut-segment',
    }));
    offset += (slice.percent / 100) * c;
  }
  return svg;
}

const prefersReducedMotion = () => globalThis.matchMedia?.('(prefers-reduced-motion: reduce)').matches ?? false;

export function initSpotlight() {
  if (typeof document === 'undefined' || prefersReducedMotion()) return;
  if (globalThis.matchMedia?.('(pointer: coarse)').matches) return;
  let raf = 0;
  let lastEvent = null;
  const selector = '.kpi, .balance-card, .spotlight-card';
  document.addEventListener('pointermove', event => {
    lastEvent = event;
    if (raf) return;
    raf = requestAnimationFrame(() => {
      raf = 0;
      const target = lastEvent?.target?.closest?.(selector);
      if (!target) return;
      const rect = target.getBoundingClientRect();
      target.style.setProperty('--mx', (lastEvent.clientX - rect.left).toFixed(1) + 'px');
      target.style.setProperty('--my', (lastEvent.clientY - rect.top).toFixed(1) + 'px');
    });
  }, {passive: true});
}

let supportsDisplayTransition;
function canTransitionDisplay() {
  if (supportsDisplayTransition !== undefined) return supportsDisplayTransition;
  if (!globalThis.CSS?.supports?.('transition-behavior', 'allow-discrete') || !document.body) return false;
  const probe = document.createElement('div');
  probe.style.cssText = 'transition: display 1s allow-discrete !important; display: block;';
  document.body.append(probe);
  getComputedStyle(probe).display;
  probe.style.display = 'none';
  supportsDisplayTransition = getComputedStyle(probe).display === 'block';
  probe.remove();
  return supportsDisplayTransition;
}

function supportsTopLayerExit() {
  return Boolean(globalThis.CSS?.supports?.('overlay', 'auto') && canTransitionDisplay());
}

export async function closeTopLayer(dialog, returnValue = '') {
  if (!dialog?.open) return;
  if (!supportsTopLayerExit() && !prefersReducedMotion()) {
    dialog.setAttribute('data-closing', '');
    const animations = dialog.getAnimations?.({subtree: true}) ?? [];
    if (animations.length) {
      await Promise.race([
        Promise.allSettled(animations.map(a => a.finished)),
        new Promise(r => setTimeout(r, 450)),
      ]);
    }
    if (!dialog.hasAttribute('data-closing')) return;
    dialog.removeAttribute('data-closing');
  }
  dialog.close(returnValue);
}

export function bindAnimatedDialog(dialog) {
  if (!dialog || dialog.dataset.animatedBound) return;
  dialog.dataset.animatedBound = 'true';
  dialog.addEventListener('cancel', event => {
    if (!supportsTopLayerExit() && !prefersReducedMotion()) {
      event.preventDefault();
      closeTopLayer(dialog);
    }
  });
}

let toastContainer = null;
function ensureToastRegion() {
  if (toastContainer && document.body.contains(toastContainer)) return toastContainer;
  toastContainer = document.getElementById('toast-stack');
  if (!toastContainer) {
    toastContainer = element('div', null, 'toast-stack');
    toastContainer.id = 'toast-stack';
    toastContainer.setAttribute('role', 'region');
    toastContainer.setAttribute('aria-label', 'Notificaciones');
    document.body.append(toastContainer);
  }
  return toastContainer;
}

export function showToast(message, tone = 'success', {duration = 4200} = {}) {
  if (!message || typeof document === 'undefined') return;
  const region = ensureToastRegion();
  const toast = element('div', null, `toast toast--${tone}`);
  if (Object.hasOwn(HTMLElement.prototype, 'popover')) {
    toast.setAttribute('popover', 'manual');
  }
  const dot = element('span', null, 'toast-dot');
  dot.setAttribute('aria-hidden', 'true');
  const text = element('span', message, 'toast-text');
  const close = element('button', '✕', 'toast-close');
  close.type = 'button';
  close.setAttribute('aria-label', 'Cerrar notificación');
  toast.append(dot, text, close);
  region.append(toast);

  if (typeof toast.showPopover === 'function') {
    try { toast.showPopover(); } catch {}
  }
  // Restack visible popovers vertically from bottom-right without overlapping.
  const restack = () => {
    const items = [...region.querySelectorAll('.toast')].reverse();
    let offset = 16;
    for (const item of items) {
      item.style.setProperty('--toast-offset', offset + 'px');
      offset += (item.offsetHeight || 48) + 10;
    }
  };
  requestAnimationFrame(restack);

  const live = document.getElementById('toast-announcer');
  if (typeof document.ariaNotify === 'function') {
    try { document.ariaNotify(message); } catch {}
  } else if (live) {
    live.textContent = '';
    setTimeout(() => { live.textContent = message; }, 30);
  }

  let timer = null;
  let paused = false;
  const dismiss = () => {
    clearTimeout(timer);
    if (!toast.isConnected) return;
    if (typeof toast.hidePopover === 'function') {
      try { toast.hidePopover(); } catch {}
    }
    toast.remove();
    restack();
  };
  const arm = () => {
    clearTimeout(timer);
    if (duration > 0 && !paused) timer = setTimeout(dismiss, duration);
  };
  close.addEventListener('click', dismiss);
  toast.addEventListener('pointerenter', () => { paused = true; clearTimeout(timer); });
  toast.addEventListener('pointerleave', () => { paused = false; arm(); });
  toast.addEventListener('focusin', () => { paused = true; clearTimeout(timer); });
  toast.addEventListener('focusout', () => { paused = false; arm(); });
  arm();
}

export function animateMoney(el, value, formatter = money) {
  if (!el) return;
  const target = Number(value);
  const formatted = formatter(value);
  if (prefersReducedMotion() || !Number.isSafeInteger(target) || typeof requestAnimationFrame !== 'function') {
    el.textContent = formatted;
    el.dataset.value = String(value);
    return;
  }
  const previous = Number(el.dataset.value);
  el.dataset.value = String(value);
  if (!Number.isSafeInteger(previous) || previous === target) {
    el.textContent = formatted;
    return;
  }
  const start = performance.now();
  const duration = 360;
  const step = now => {
    const t = Math.min(1, (now - start) / duration);
    const eased = 1 - Math.pow(1 - t, 3);
    const current = Math.round(previous + (target - previous) * eased);
    el.textContent = formatter(current);
    if (t < 1) requestAnimationFrame(step);
    else el.textContent = formatted;
  };
  requestAnimationFrame(step);
}
