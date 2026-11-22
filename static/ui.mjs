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
  card: '<rect x="3" y="6" width="18" height="13" rx="2"/><path d="M3 10h18"/>',
  bars: '<path d="M5 20v-6M11 20V8M17 20v-10M3 20h18"/>',
};
export function icon(name) {
  const svg = document.createElementNS('http://www.w3.org/2000/svg', 'svg');
  svg.setAttribute('aria-hidden', 'true');
  svg.setAttribute('viewBox', '0 0 24 24');
  svg.innerHTML = ICONS[name];
  return svg;
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
