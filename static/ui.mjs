// Shared presentation helpers: money formatting, DOM nodes, forms and the session key.
export const sessionKey = 'misfinanzas:last-user';

export function lastIdentity() {
  try { return JSON.parse(localStorage.getItem(sessionKey) || 'null'); }
  catch { return null; }
}

export const money = value => new Intl.NumberFormat('es-PY')
  .format(typeof value === 'string' ? BigInt(value) : value) + ' Gs.';

export function element(tag, text, className) {
  const el = document.createElement(tag);
  if (text != null) el.textContent = text;
  if (className) el.className = className;
  return el;
}

export function field(form, caption, name, type, options = {}) {
  const label = element('label', caption);
  const input = element(type === 'select' ? 'select' : 'input');
  if (type === 'select') {
    for (const item of options.items ?? []) {
      // Callers pass either [id, name] pairs or {id, name} objects.
      const [id, text] = Array.isArray(item) ? item : [item.id, item.name];
      const option = element('option', text);
      option.value = id;
      input.append(option);
    }
  } else input.type = type;
  for (const [key, value] of Object.entries(options)) if (key !== 'items') input[key] = value;
  input.name = name;
  input.required = options.required ?? true;
  label.append(input);
  form.append(label);
  return input;
}

// One chunk size for every byte-to-text encoder: the limit keeps
// String.fromCharCode/Array.from from overflowing the call stack.
export function chunkedText(bytes, encode) {
  const parts = [];
  for (let start = 0; start < bytes.length; start += 8192) parts.push(encode(bytes.subarray(start, start + 8192)));
  return parts.join('');
}

// Decorative inline icons: the accessible label is always the text next to them.
const ICONS = {
  trend: '<path d="M3 17l6-6 4 4 8-8"/><path d="M15 7h6v6"/>',
  cash: '<rect x="3" y="7" width="18" height="10" rx="2"/><circle cx="12" cy="12" r="2"/>',
  bank: '<path d="M3 10h18M5 10v8M9 10v8M15 10v8M19 10v8M2 18h20M12 3l9 5H3z"/>',
  wallet: '<path d="M3 7a2 2 0 0 1 2-2h14a2 2 0 0 1 2 2v10a2 2 0 0 1-2 2H5a2 2 0 0 1-2-2z"/><path d="M16 12h5"/>',
  card: '<rect x="3" y="6" width="18" height="13" rx="2"/><path d="M3 10h18"/><path d="M7 15h4"/>',
  bars: '<path d="M5 20v-6M11 20V8M17 20v-10M3 20h18"/>',
  income: '<path d="M7 17L17 7M9 7h8v8"/>',
  expense: '<path d="M7 7l10 10M17 9v8H9"/>',
  transfer: '<path d="M7 8h10l-3-3M17 16H7l3 3"/>',
  payment: '<rect x="3" y="6" width="18" height="13" rx="2"/><path d="M3 10h18"/>',
  refund: '<path d="M9 14l-4-4 4-4"/><path d="M5 10h9a5 5 0 1 1 0 10h-2"/>',
  loan: '<circle cx="12" cy="12" r="9"/><path d="M12 7v5l3 3"/>',
  search: '<circle cx="11" cy="11" r="7"/><path d="M20 20l-3.5-3.5"/>',
  plus: '<path d="M12 5v14M5 12h14"/>',
  shield: '<path d="M12 3l7 3v6c0 5-3.5 8-7 9-3.5-1-7-4-7-9V6l7-3z"/>',
  user: '<circle cx="12" cy="8" r="4"/><path d="M4 20a8 8 0 0 1 16 0"/>',
  copy: '<rect x="9" y="9" width="11" height="11" rx="2"/><path d="M5 15H4a2 2 0 0 1-2-2V4a2 2 0 0 1 2-2h9a2 2 0 0 1 2 2v1"/>',
  sparkles: '<path d="M12 3l1.8 4.2L18 9l-4.2 1.8L12 15l-1.8-4.2L6 9l4.2-1.8L12 3z"/><path d="M19 15l.9 2.1L22 18l-2.1.9L19 21l-.9-2.1L16 18l2.1-.9L19 15z"/>',
};
export function icon(name) {
  const svg = document.createElementNS('http://www.w3.org/2000/svg', 'svg');
  svg.setAttribute('aria-hidden', 'true');
  svg.setAttribute('viewBox', '0 0 24 24');
  svg.innerHTML = ICONS[name] ?? ICONS.bars;
  return svg;
}

export function pill(text, tone = '') {
  return element('span', text, tone ? 'pill pill--' + tone : 'pill');
}

export function progressBar(percent, tone = '') {
  const track = element('div', null, 'progress-track');
  const fill = element('div', null, tone ? 'progress-fill progress-fill--' + tone : 'progress-fill');
  fill.style.width = Math.max(0, Math.min(100, percent)) + '%';
  track.append(fill);
  return track;
}

export function confirmAction(message, acceptLabel = 'Confirmar') {
  const dialog = globalThis.document?.getElementById?.('confirm-dialog');
  if (!dialog?.showModal) return globalThis.confirm ? globalThis.confirm(message) : true;
  document.getElementById('confirm-message').textContent = message;
  document.getElementById('confirm-accept').textContent = acceptLabel;
  dialog.returnValue = '';
  dialog.showModal();
  return new Promise(resolve => dialog.addEventListener('close', () => {
    const accepted = dialog.returnValue === 'confirm';
    dialog.returnValue = '';
    resolve(accepted);
  }, {once: true}));
}
