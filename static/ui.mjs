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
