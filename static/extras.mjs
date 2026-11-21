export function enrichCatalog(snapshot) {
  for (const [entity, items] of [['account', snapshot.accounts], ['category', snapshot.categories]]) {
    const metadata = new Map((snapshot.catalog_details || []).filter(d => d.entity === entity).map(d => [d.id, d]));
    for (const item of items) Object.assign(item, metadata.get(item.id));
  }
}

const titles = {
  cash: 'Efectivo', bank: 'Banco', wallet: 'Billetera', card: 'Tarjeta de crédito',
};

function element(tag, text) {
  const result = document.createElement(tag);
  if (text !== undefined) result.textContent = text;
  return result;
}

async function fileBase64(file) {
  const bytes = new Uint8Array(await file.arrayBuffer());
  const chunks = [];
  for (let start = 0; start < bytes.length; start += 8192) {
    chunks.push(String.fromCharCode(...bytes.subarray(start, start + 8192)));
  }
  return btoa(chunks.join(''));
}

function money(value) {
  return new Intl.NumberFormat('es-PY').format(BigInt(value)) + ' Gs.';
}

export function renderData(container, {api, getView, today, synchronize, isCurrent}) {
  const content = element('div');
  container.replaceChildren(content);
  const live = () => content.parentNode === container && isCurrent();
  const importSection = element('section');
  const form = element('form');
  const result = element('div');
  const status = element('p');
  status.setAttribute('role', 'status');
  const fileLabel = element('label', 'Libro XLSX de Google Sheets (hasta 5 MiB)');
  const file = element('input');
  file.type = 'file';
  file.accept = '.xlsx,application/vnd.openxmlformats-officedocument.spreadsheetml.sheet';
  file.required = true;
  fileLabel.append(file);
  const cutoffLabel = element('label', 'Incluir movimientos anteriores a la fecha de corte');
  cutoffLabel.className = 'checkbox';
  const cutoff = element('input');
  cutoff.type = 'checkbox';
  cutoffLabel.prepend(cutoff);
  const preview = element('button', 'Ver vista previa');
  form.append(fileLabel, cutoffLabel, preview);
  importSection.append(element('h3', 'Importar cuentas, categorías y movimientos'), form, status, result);
  content.append(importSection);
  let mapping = Object.create(null);
  let review = null;
  let busy = false;

  function invalidate() {
    review = null;
    const confirm = result.querySelector('[data-confirm]');
    if (confirm) confirm.disabled = true;
  }

  function details(title, rows) {
    if (!rows.length) return;
    const group = element('details');
    group.open = true;
    const list = element('ul');
    for (const row of rows.slice(0,100)) list.append(element('li', row));
    if (rows.length > 100) list.append(element('li', 'Se muestran los primeros 100 de ' + rows.length + ' avisos.'));
    group.append(element('summary', title + ' (' + rows.length + ')'), list);
    result.append(group);
  }

  function show(report) {
    result.replaceChildren();
    result.append(element('p', report.added + ' filas para importar · ' + report.duplicates + ' ya importadas · ' + report.skipped + ' omitidas'));
    const existing = getView()?.accounts || [];
    if (report.accounts.length) {
      const accounts = element('details');
      accounts.open = report.errors.length > 0;
      accounts.append(element('summary', 'Vincular cuentas de origen'));
      for (const row of report.accounts) {
        const caption = element('label', row.name + ' · ' + (titles[row.kind] || row.kind) + ' · ' + row.id);
        const select = element('select');
        const fresh = element('option', 'Crear como cuenta nueva');
        fresh.value = '';
        select.append(fresh);
        for (const item of existing.filter(account => account.kind === row.kind)) {
          const option = element('option', item.name);
          option.value = item.id;
          select.append(option);
        }
        select.value = mapping[row.id] || row.linked || '';
        select.disabled = busy;
        select.addEventListener('change', () => {
          if (select.value) mapping[row.id] = select.value;
          else delete mapping[row.id];
          invalidate();
          status.textContent = 'Vuelve a generar la vista previa para revisar los vínculos.';
        });
        caption.append(select);
        accounts.append(caption);
      }
      result.append(accounts);
    }
    details('Errores que debes corregir', report.errors);
    details('Filas y saldos para revisar', report.warnings);
    const names = new Map(existing.map(account => [account.id, account.name]));
    for (const row of report.accounts) names.set(row.linked || row.id, row.name);
    let acked = !report.differences.length;
    if (report.differences.length) {
      const group = element('details');
      group.open = true;
      group.append(element('summary', 'Diferencias de saldo a aceptar (' + report.differences.length + ')'));
      const list = element('ul');
      for (const row of report.differences) {
        list.append(element('li',
          (names.get(row.account_id) || row.account_id)
          + ' · declarado ' + money(row.declared_balance)
          + ' · calculado ' + money(row.calculated_balance)
          + ' · diferencia ' + money(row.difference)));
      }
      group.append(list);
      const label = element('label');
      label.className = 'checkbox';
      const accept = element('input');
      accept.type = 'checkbox';
      accept.addEventListener('change', () => {
        acked = accept.checked;
        save.disabled = !!report.errors.length || !acked;
        status.textContent = accept.checked ? '' : 'Marca la aceptación para poder importar.';
      });
      label.append(accept, document.createTextNode(' He revisado las diferencias de saldo y acepto importar estos movimientos sin ajustar automáticamente los balances.'));
      group.append(label);
      result.append(group);
    }
    details('Saldos después de importar', Object.entries(report.balances).map(([id,balance]) =>
      (names.get(id) || id) + ' · ' + money(balance)));
    const save = element('button', 'Confirmar importación');
    save.type = 'button';
    save.dataset.confirm = '';
    save.disabled = !!report.errors.length || !acked;
    save.addEventListener('click', () => {
      if (review && confirm('¿Importar las filas de esta vista previa?')) run(true);
    });
    result.append(save);
  }

  async function run(committing) {
    if (!live() || busy || committing && !review) return;
    const item = file.files[0];
    if (!item || item.size > 5 * 1024 * 1024) {
      status.textContent = 'Selecciona un XLSX de hasta 5 MiB.';
      return;
    }
    const expected = review;
    if (!committing) invalidate();
    busy = true;
    preview.disabled = cutoff.disabled = file.disabled = true;
    for (const field of result.querySelectorAll('button, select')) field.disabled = true;
    status.textContent = committing ? 'Importando…' : 'Preparando la vista previa…';
    try {
      const encoded = committing ? expected.file : await fileBase64(item);
      if (!live()) return;
      const input = {file:encoded, mapping:{...mapping}, include_before_cutoff:cutoff.checked};
      if (committing) {
        input.preview_token = expected.token;
        if (expected.differences) input.ack_saldo = expected.token;
      }
      const report = await api('/api/import/xlsx', 'POST', input);
      if (!live()) return;
      busy = false;
      review = {file:encoded, token:report.token, differences: !!report.differences?.length};
      if (report.committed) {
        review = null;
        result.replaceChildren();
        status.textContent = 'Importación guardada. Filas aplicadas ' + report.added + ' y ya importadas ' + report.duplicates + '.';
        await synchronize();
      } else {
        show(report);
        status.textContent = '';
      }
    } catch (e) {
      if (live()) {
        invalidate();
        status.textContent = e.message;
      }
    } finally {
      busy = false;
      preview.disabled = cutoff.disabled = file.disabled = false;
      for (const select of result.querySelectorAll('select')) select.disabled = false;
    }
  }

  form.addEventListener('submit', event => { event.preventDefault(); run(false); });
  file.addEventListener('change', () => {
    mapping = Object.create(null);
    review = null;
    result.replaceChildren();
    status.textContent = '';
  });
  cutoff.addEventListener('change', invalidate);

  const exports = element('section');
  const actions = element('div');
  actions.className = 'actions';
  const monthLabel = element('label', 'Mes del informe');
  const month = element('input');
  month.type = 'month';
  month.value = today.slice(0, 7);
  monthLabel.append(month);
  const exportStatus = element('p');
  exportStatus.setAttribute('role','status');
  for (const [kind,label] of [['cuentas','Cuentas'],['movimientos','Movimientos'],['informe','Informe mensual']]) {
    const button = element('button', 'Descargar CSV de ' + label.toLowerCase());
    button.type = 'button';
    button.className = 'secondary';
    button.addEventListener('click', async () => {
      button.disabled = true;
      exportStatus.textContent = '';
      try {
        const date = kind === 'informe' ? '?month=' + encodeURIComponent(month.value) : '';
        const blob = await api('/api/export/' + kind + date, 'GET', null, 'blob');
        if (!live()) return;
        const url = URL.createObjectURL(blob);
        const link = element('a');
        link.href = url;
        link.download = 'finapp-' + kind + (kind === 'informe' ? '-' + month.value : '') + '.csv';
        link.click();
        setTimeout(() => URL.revokeObjectURL(url), 60000);
      } catch (e) {
        if (live()) exportStatus.textContent = e.message;
      } finally { button.disabled = false; }
    });
    actions.append(button);
  }
  exports.append(element('h3','Exportar'), monthLabel, actions, exportStatus);
  content.append(exports);
}
