# Auditoría general de FinApp

**Las 6 entregas funcionales están completas y la estabilización previa al rediseño visual terminó.** El trabajo se hace en `main`; la rama `estabilizacion-ux` ya no se usa. El contenedor de producción no se ha tocado: pendiente validar en un despliegue real.

## Estado por entrega

| Entrega | Resultado |
| --- | --- |
| 1. Primer uso y catálogos | Completa y cubierta por pruebas; estado vacío verificado en navegador. |
| 2. Modelo financiero y operaciones | Completa; devoluciones cubiertas con 13 aserciones nuevas; `TOTAL`→`SUM` (enteros exactos). |
| 3. PWA y sincronización | Completa; actualización versionada por build (commit estampado en `sw.js`) verificada dos veces en Chrome real (v3 y v4): botón «Aplicar actualización», recarga controlada, cola IndexedDB conservada, caché vieja eliminada. |
| 4. Tarjetas y préstamos | Completa; rectificaciones `cancel_purchase` y `cancel_repayment` atómicas e idempotentes; deuda inicial de tarjeta con vencimiento en el ciclo de corte; corrección = anular y volver a registrar. |
| 5. Planificación e informes | Completa; digest de revisión mensual v2 incluye saldos, patrimonio, flujo y deuda → editar un mes anterior devuelve los meses posteriores a pendiente (invalidación única aceptada). |
| 6. Importación, conciliación y recuperación | Completa; años de dos dígitos rechazados con mensaje claro; importación de 25.000 filas medida: 11,4 s, `/health` nunca bloqueado (peor caso 476 µs), lector concurrente espera ~6 s por la conexión compartida — bloqueo acotado documentado, sin refactor. |

## Correcciones y hallazgos de la renovación UX/UI

- Design system sobre tokens con la identidad verde/oro existente; sin frameworks ni fuentes nuevas.
- Resumen con 4 KPIs: patrimonio neto, disponible, deuda de tarjetas, resultado del mes; misma semántica que el informe del servidor, con prueba.
- Modo oscuro con tokens (`prefers-color-scheme`), contraste verificado 6,7–13,3 en ambos modos; `theme-color` dual para Android.
- Color semántico fijo para importes firmados; `tabular-nums`; acción rápida «Registrar movimiento».
- Favicon 404 corregido; guard de `prefers-reduced-motion`; foco verificado parada por parada.

## Qué falta, en orden de prioridad

1. **Validar en producción:** respaldo previo, `FINAPP_VERSION` en el despliegue y comprobación de la actualización PWA en un dispositivo real.
2. **Importación real:** ensayar el XLSX real en instancia de prueba y cuadrar saldos y totales.
3. **Android físico:** instalación PWA, uso sin conexión y rendimiento del historial (10.003 movimientos sintéticos: 1,96 MB iniciales, sincronización sin cambios de 141 bytes).
4. **Copia automática:** confirmar una ejecución programada del respaldo y una copia recuperable fuera del servidor (al auditar figuraban 0 ejecuciones registradas).
5. **Observado una vez en desarrollo:** error de fila intermitente en la importación masiva (SELECT de `import_sources`, fila ~19.614), no reproducido en 6 corridas; si reaparece, instrumentar el error SQLite que hoy se traga en `src/exchange.rs`.

## Registro de CI

- Commit `4d31b59` en `main`: workflow `CI` (#1) completado con éxito en GitHub Actions, 1 min 39 s. Comprobado el 2026-11-20 en la página pública de Actions. Los jobs `rust`, `web` y `docker` no se revisaron por separado.

## F8.4 — Observabilidad SQLite e importación

### Instrumentación

Los errores SQLite de la importación ya no se descartan. Se registran con etapa estática, lugar, código primario y código extendido, sin mensajes de SQLite ni datos de filas. Puntos cubiertos: `source_lookup`, `source_exists`, `source_save`, `finance_write`, `savepoint`, `savepoint_commit`, `balances`, `import_begin`, `import_run`, `import_commit`, `db`, `import_lock` y `hash`. Al final de cada importación se registra un resumen con contadores y duración, sin importes ni nombres.

El mensaje público y los códigos HTTP no cambiaron. Las pruebas confirman que un error de esquema sigue devolviendo el texto «No se pudo consultar el historial de importación.» y que una confirmación con errores de fila sigue devolviendo 409 con «La vista previa cambió…».

### Códigos que ahora pueden identificarse

- `code=DatabaseBusy extended=5` (SQLITE_BUSY): contención de escritura.
- `code=Unknown extended=1` (SQLITE_ERROR): error de esquema, por ejemplo una tabla eliminada.
- `error=non_sqlite`: errores que no son de SQLite, sin texto libre.

El registro no contiene nombres de cuentas, importes, saldos, contraseñas, tokens, cookies ni parámetros SQL. Una prueba unitaria construye un error SQLite con un mensaje que contiene datos sensibles y verifica que no aparece en el registro.

### busy_timeout y bloqueo

`rusqlite 0.37` configura `sqlite3_busy_timeout(db, 5000)` al abrir cada conexión (`inner_connection.rs:119`). Una prueba consulta `PRAGMA busy_timeout` y confirma 5000 ms. No se añadió ningún PRAGMA ni se habilitó WAL.

Hallazgo concreto: con una transacción `DEFERRED`, al promover una lectura a escritura mientras otra conexión mantiene `RESERVED`, SQLite devuelve `SQLITE_BUSY` en aproximadamente 1,4 ms. El `busy_timeout` no se aplica en esa promoción, para evitar un deadlock. Por eso la prueba de contención no exige espera de 5 s.

### Experimentos

- Importación de 25.000 movimientos: 11,23 s para vista previa y confirmación; peor `/health` durante la importación: 154 µs; `/api/snapshot` concurrente: 200. Una ejecución.
- Contención controlada entre dos conexiones: `BEGIN IMMEDIATE` en la segunda conexión durante la confirmación produce `code=DatabaseBusy extended=5`, sin datos de filas. El mensaje público sigue siendo el anterior y la integridad (`PRAGMA integrity_check`) da `ok`.
- Error de esquema controlado: al eliminar `import_sources` desde otra conexión, se registran cuatro fallos `source_lookup` con `extended=1`; la confirmación no se ejecuta y no queda ninguna operación escrita.
- Reimportación del mismo archivo: segunda pasada con 4 duplicados, 0 añadidos, 0 errores y 2 movimientos en la base. Sin duplicados.
- Pruebas repetidas de carga: no se hicieron 10 corridas en esta fase. Una sola corrida no demuestra ausencia de fallos de concurrencia.

### Hipótesis y límites

- El fallo intermitente de la fila 19.614 no se reprodujo. Sigue como observación pendiente, pero ahora existe instrumentación para diagnosticarlo si reaparece.
- No se ejecutó `scripts/backup.sh` en paralelo con una importación real. La contención entre conexiones se simuló con una segunda conexión que toma `BEGIN IMMEDIATE`; no es una prueba del script de respaldo.
- No se cambió el modelo de concurrencia: sigue siendo una conexión compartida por `Arc<Mutex<Connection>>`.

## Verificación

70 pruebas automatizadas pasando (34 Rust + 36 JavaScript), `cargo fmt --check` y `clippy -D warnings` limpios. Flujo de actualización PWA verificado en navegador real dos veces consecutivas. Pruebas visuales y de accesibilidad ejecutadas en Chrome (landmarks, foco, contraste, targets táctiles, estados vacío/sin conexión/oscuro); capturas en el equipo de trabajo.

## F9.1A — Identificadores de operación e idempotencia

### Resultado: defecto DESCARTADO (no reproducible)

El hallazgo de la auditoría F9.0 (dos operaciones legítimas idénticas compartirían `request_id` y la segunda se descartaría en silencio) **no se confirmó**. No se aplicó ninguna corrección a código de producción; `static/credit.mjs`, `sync.mjs` y `offline-store.mjs` quedan intactos.

### Causa del hallazgo y comportamiento real

- `request_id` se genera con `crypto.randomUUID()` en `static/credit.mjs:158` (formularios) y `:184` (anulaciones).
- `last` se memoiza por `key = JSON.stringify(entry)` **dentro del cierre de cada formulario/botón**, no globalmente.
- Tras un POST confirmado, `credit.mjs:162` vuelve a renderizar la sección (`renderCredit`), por lo que el siguiente formulario tiene un `last` nuevo y, por tanto, un `request_id` distinto. Dos pagos idénticos legítimos se registran por separado.
- `submit.disabled` (`credit.mjs:151-152`, `:178`) bloquea el doble clic mientras la petición está en curso.
- `synchronize()` (`static/app.mjs:526-555`) captura sus propios errores y nunca rechaza, así que el re-render posterior al éxito siempre ocurre.
- La deduplicación del servidor está dentro de la misma transacción: `src/credit.rs:290-304,509-514` y `src/finance.rs:299-312,505-509`. Un replay idéntico devuelve la respuesta guardada sin escribir de nuevo; un mismo ID con otro contenido devuelve 409.

### Reproducción ejecutada

Se añadió `tests/credit-idempotency.test.mjs` con un DOM mínimo que ejecuta el `renderCredit` real (no una copia):

- A/E: dos pagos idénticos, cada uno en el formulario generado tras el éxito anterior → dos POST con `request_id` distinto.
- B: el servidor guarda la respuesta y no duplica el efecto; comprobado además en SQLite con `src/finance.rs` (un solo `finance_operations` y un solo delta −100).
- C: doble clic con petición pendiente → un solo POST.
- D: fallo de red y reintento → el mismo `request_id`, sin mensaje de error persistente.
- F: un POST rechazado nunca muestra «Guardado.».

Mutación temporal: al comentar el re-render de `credit.mjs:162`, la prueba A falla porque el segundo pago reutiliza el mismo UUID (`9e9…99`). La mutación se revirtió y la prueba volvió a pasar. Esto confirma que el re-render es lo que separa operaciones lógicas y que la prueba detecta la regresión.

### Pruebas incorporadas

- `tests/credit-idempotency.test.mjs` (4 pruebas, DOM shim sin dependencias nuevas).
- `src/credit.rs`: `two_identical_legitimate_repayments_are_both_persisted` — dos reembolsos idénticos con IDs distintos → 2 pagos, deuda 600, 7 operaciones, `integrity_check=ok`, `foreign_key_check=0`.
- `src/finance.rs`: `replaying_a_request_keeps_a_single_balance_delta` — replay con el mismo `request_id` → una operación, delta −100, `integrity_check=ok`, `foreign_key_check=0`.

### Garantías de idempotencia

- Un identificador por operación lógica; se conserva durante los reintentos de esa misma operación mientras la página siga abierta.
- Deduplicación del backend transaccional; no depende del estado del navegador.
- Cola offline (`sync.mjs:19`, `offline-store.mjs`) sigue usando el ID de la operación local como `request_id`, sin cambios.

### Riesgos residuales

1. Si el POST se confirmó pero se perdió la respuesta **y se recarga la página**, `last` desaparece: un reenvío manual con los mismos datos genera un ID nuevo y podría duplicar el pago. Mitigación actual: el usuario debe revisar el historial antes de reenviar. Corregirlo exigiría sacar los comandos de crédito de la cola offline o una clave natural en el servidor; no se hizo en esta subfase.
2. El mensaje «Guardado.» se limpia al re-renderizar (comportamiento previo, solo visual); el resultado persistido es correcto.
3. No se usó chrome-devtools: la reproducción es determinista en Node sobre el módulo real y la mutación demostró que la prueba falla. No se probó en navegador real.

### Verificación F9.1A

`cargo fmt --check`, `cargo clippy --all-targets --locked -- -D warnings`, `cargo test --locked` (53 Rust), `npm test` (40 JS), reconstrucción de `Dockerfile.test` y suite completa dentro de Docker: todo en verde. Sin cambios en migraciones ni en reglas financieras.

## F9.1B — Exactitud de saldos, movimientos y patrimonio

Nuevo archivo `tests/finance_invariants.rs` (6 pruebas). Todas usan `Database::open_memory()`, datos sintéticos, las rutas reales (`finance::apply`) y HTTP para `/api/planning/month/{month}`. Los valores esperados son **literales calculados a mano**; nunca se llama a `report()` ni a `balances_in()` como oráculo. El estado se comprueba con SQL directo sobre `finance_balance_postings` y `finance_postings`, más `PRAGMA integrity_check` y `PRAGMA foreign_key_check` en cada prueba.

### Escenarios verificados (esperado = observado)

| Escenario | Operación | Valores literales comprobados | Resultado |
|---|---|---|---|
| A — Ingresos y gastos | ingreso 500 banco, gasto 300 efectivo; iniciales 1000 y 200 | banco 1500, efectivo −100, patrimonio 1400, income 500, expense 300, net 200, cash_flow 200 | OK |
| B — Transferencia | 400 de `a` (1000) a `b` (0) | origen 600, destino 400, patrimonio 1000, income/expense/net/cash_flow 0; postings −400/+400 | OK |
| C — Reintegro | gasto 500 y reembolso 200; reembolso extra 301 | saldo y patrimonio −300, gasto neto 300, cash_flow −300; 301 rechazado sin escrituras; variante a otra cuenta: −500/+200, patrimonio −300 | OK |
| D — Anulación | ingreso 700, borrado con versión 1, replay idéntico, versión obsoleta | efectos a 0; 1 operación tombstoned v2; historial con inserción + modificación; replay sin duplicar; versión obsoleta → Conflict sin cambios | OK |
| E — Fechas de corte | cortes 15-Nov, 01-Nov, sin corte, y cruce 31-Dic/01-Ene | cut15 1150, cut01 540, nocut 90, patrimonio 1780, income 240, expense 160, net 80, cash_flow 80; año: Dic 300, Ene 370 | OK |
| F — Pago de tarjeta | deuda inicial 300, banco 1000, pago 300 | banco 700, tarjeta 0, patrimonio 700 antes y después, income/expense/net 0, cash_flow −300 | OK |

### Oráculos

- Saldo = `initial_balance` de `finance_accounts` + `SUM(delta)` de `finance_balance_postings` (vista con filtro de corte), calculado con SQL crudo.
- `finance_postings` se consulta directamente para verificar signos y la naturaleza interna de las transferencias y los pagos.
- Informes: JSON de `/api/planning/month/...` comparado contra literales. `net_worth`, `net` y `cash_flow` se comprueban por separado; no se asume que coincidan.
- Integridad SQLite: `integrity_check=ok` y `foreign_key_check=0` en las 6 pruebas.

### Mutaciones temporales

| Mutación | Prueba que falló | Restauración |
|---|---|---|
| `planning.rs`: reintegro con signo positivo | C (`expense` 700 en vez de 300) | revertida; `git diff` sin cambios en producción |
| `planning.rs`: quitar inversión de signo de tarjeta | F (`net_worth` inicial 1300 en vez de 700) | revertida |
| `finance.rs`: quitar `cutoff_date<=?2` al incluir el saldo inicial | E (cut15 150 en vez de 1150) | revertida |

Cada mutación se ejecutó por separado con `cargo test --locked --test finance_invariants` (5 pasan / 1 falla) y se restauró antes de continuar. El diff final contiene solo el archivo de pruebas nuevo.

### Reglas verificadas

Saldo inicial con fecha de corte (`>=` incluye el día del corte), ingresos, gastos con signo de tarjeta, transferencias internas sin efecto patrimonial, reintegros topados y permisivos hacia otra cuenta no-tarjeta, anulaciones con reversión exacta e historial, y pago de tarjeta como cancelación de obligación (no gasto).

### Ambigüedad conocida [AMB]

`report.income/expense/net` y `cash_flow` **incluyen** movimientos anteriores a la fecha de corte, mientras que `balances` los excluye. En el escenario E, `expense=160` incluye el gasto de 100 previo al corte y `cash_flow=80` también, pero el saldo de cut15 solo cambia 150. Por eso `net` (80), `cash_flow` (80) y la variación patrimonial (180) no coinciden. Se documenta el comportamiento observado; **no se modificó la regla**.

### Defectos

Ninguno confirmado. El comportamiento de saldos negativos, reintegros a otra cuenta y la diferencia entre corte e informes son decisiones existentes, no errores nuevos.

### Riesgos pendientes

- Heredado de F9.1A: reenvío manual tras pérdida de respuesta y recarga de página puede duplicar un pago de crédito; sigue mitigado solo por revisión del historial.
- La ambigüedad [AMB] de fechas de corte requiere decisión de negocio antes de cambiarla.
- No se probaron informes con préstamos (deuda y amortización): queda para F9.2.

### Verificación F9.1B

`cargo test --locked --test finance_invariants` (6/6), `cargo fmt --check`, `cargo clippy --all-targets --locked -- -D warnings`, `cargo test --locked` (59 Rust: 24 lib + 14 auth + 8 finance_api + 6 finance_invariants + 3 full_flow + 4 import_load), `npm test` (40 JS), reconstrucción de `Dockerfile.test` y suite completa en Docker: todo en verde. Sin cambios de producción, migraciones ni credenciales.

## F9.2 — Correcciones al plan aprobado

- 2A-03: se conserva la anualidad redondeada de **407.615 Gs.** y la tasa efectiva mensual `(1,12)^(1/12)−1 = 0,009488792934582935` (verificada por bisección de `(1+r)^12 = 1,12`).
- 2A-11: **no** se atribuye la discrepancia a la conversión del principal. El principal 7.000.000.000.000.003 es exactamente representable en f64 (`< 2^53`). El error está en la tasa y en el producto.
- 2B-02: con banco inicial 0, tras desembolsar 1.000 y pagar 221 el saldo absoluto es **779**; la variación debida al pago es **−221**.
- 2B-03: pago ordinario 300 + 300 + 300 y extraordinario 100 liquidan exactamente un préstamo de 1.000.
- 2C-02: la compra de 300 reduce el patrimonio en 300; su pago posterior **no lo restaura**, solo cancela la obligación.
- 2D-02: con pagos de 200 y 100 exclusivamente de capital, el patrimonio permanece en 500.

Escenarios 2B, 2C y 2D **no implementados** en esta entrega.

## F9.2A — Amortización V1/V2 (CORREGIDA)

`schedule()` es privada, así que las pruebas se añadieron como unitarias en `#[cfg(test)]` sin ampliar la API. Los esperados son literales calculados fuera del código, salvo el oráculo racional `i128` de 2A-11.

### Causa raíz y decisión

- El principal 7.000.000.000.000.003 es exacto en f64. El error estaba en `rate = (7000/10000)/12 = 0,05833333333333332732` frente al exacto `7/120 = 0,05833333333333333333`; el producto cae en `…333,4375` y redondea a `…333` en vez de `…334`.
- Un barrido de 21 principales × 11 tasas encontró 12 errores de ±1 con la fórmula anterior.
- **V1** conserva esa fórmula para préstamos preexistentes. **V2** calcula el interés nominal como `round_half_up(principal·rate_bps, 120000)` con `i128`, empate hacia arriba, y conversión comprobada a `i64`. La anualidad francesa y la tasa efectiva siguen en f64 en ambas versiones.
- Alternativa descartada: cambiar solo `rate = bps/120000.0` corrige 2A-11 pero deja 11 de 12 casos del barrido. La columna con `DEFAULT 1` es más simple que una tabla aparte de versiones: no requiere JOIN ni FK nueva.

### Resultados V1 y V2

| ID | Esperado | V2 observado | V1 observado | Estado |
|---|---|---|---|---|
| 2A-01 francesa nominal 1.200.000 @12% 3 | 396.027/399.987/403.986 + 12.000/8.040/4.040 | idéntico | idéntico | OK |
| 2A-02 alemana nominal | 400.000 ×3 + 12.000/8.000/4.000 | idéntico | idéntico | OK |
| 2A-03 francesa efectiva (anualidad 407.615) | 396.228/399.988/403.784 + 11.387/7.627/3.831 | idéntico | idéntico | OK |
| 2A-04 tasa cero | 34/33/33 | idéntico | idéntico | OK |
| 2A-05 principal 100 | 33+1 / 33+1 / 34+0 | idéntico | idéntico | OK |
| 2A-06 medio guaraní | 50 + 1 | idéntico | idéntico | OK |
| 2A-07 una cuota | 1.000 + 10 | idéntico | idéntico | OK |
| 2A-08 600 cuotas | 600 capitales de 1, 6→0 | idéntico | idéntico | OK |
| 2A-09 bisiesto | 2024-02-29 / 2024-03-29 | idéntico | idéntico | OK |
| 2A-10 mes/año | 04-30 y 2027-01-31 | idéntico | idéntico | OK |
| **2A-11** | **408.333.333.333.334** | **408.333.333.333.334** | 408.333.333.333.333 | **OK (V2)** |
| Límites y desbordamiento SAFE | rechazo | rechazo | rechazo | OK |

Además: capital conservado, última cuota = remanente, alemana y efectiva sin regresiones en V2, medio guaraní y límites SAFE, y préstamos nuevos creados con `schedule_version=2` y leídos por `portfolio()` según su versión almacenada.

### Migración 009

`ALTER TABLE credit_loans ADD COLUMN schedule_version INTEGER NOT NULL DEFAULT 1 CHECK (schedule_version IN (1,2))`. No se reescribió 006. Prueba con esquema 001-008 y un préstamo antiguo: tras abrir con el código nuevo el préstamo sigue en V1 con todos sus importes, el pago histórico 200/20/1 permanece, `schema_versions=9`, una segunda apertura no reaplica nada, `integrity_check=ok` y `foreign_key_check=0`. Un préstamo nuevo se inserta explícitamente en V2; la versión 3 es rechazada.

### Mutaciones (todas restauradas)

| Mutación | Prueba que falló |
|---|---|
| V2 vuelve al interés f64 | A11 V2 (`…333` vs `…334`) |
| `portfolio()` ignora la versión guardada | `new_loans_use_v2...` |
| DEFAULT de la migración cambiado a 2 | `migration_009_preserves_legacy_loans...` |
| V1 pasa a usar el interés i128 | A11 histórico (`…334` vs `…333`) |
| Anteriores (redondeo, capital, última cuota, fechas, tasa) | A01/A03/A05/A06, A04, seis pruebas, A10, seis pruebas |

### Verificación y riesgo

- Pruebas focalizadas: 12 de amortización + 3 V2 + 1 préstamo V2/V1 + 2 de migración.
- No queda ninguna prueba ignorada; la reproducción exacta de 2A-11 se ejecuta normalmente.
- `cargo fmt --check`, `clippy -D warnings`, `cargo test --locked` (**78 Rust**: 41 lib, 14 auth, 2 credit_migration, 8 finance_api, 6 finance_invariants, 3 full_flow, 4 import_load), `npm test` (**40 JS**), build y suite Docker: todo en verde.
- **Riesgo residual:** la anualidad francesa y la tasa efectiva siguen usando f64, por lo que no se afirma que toda la amortización sea matemáticamente exacta. V2 corrige solo el interés nominal.
- Antes de cualquier despliegue: hacer `bash scripts/backup.sh <destino>`, verificar el checksum y ensayar la restauración con `scripts/restore-check.sh`; la migración 009 es aditiva y no modifica datos, pero altera el esquema.

## F9.2B — Préstamos, desembolsos y pagos

Nuevo archivo `tests/credit_flows.rs` (7 pruebas de integración sobre el router Axum con sesión real). Contrato confirmado: `POST /api/credit` devuelve **204** en éxito, **400** en validaciones, **404** si el préstamo no pertenece al usuario y **409** si un `request_id` se reutiliza con otro payload; `GET /api/credit` devuelve la cartera. Los esperados son literales; el estado se comprueba con SQL directo (`credit_loans`, `credit_repayments`, `finance_operations`, `finance_postings`, `finance_balance_postings`, `credit_requests`, historial).

| ID | Esperado | Observado | Estado |
|---|---|---|---|
| 2B-01 desembolso 1.000 | banco 1.000, deuda 1.000, patrimonio 0, income/expense/net 0, cash_flow +1.000, loan_debt 1.000 | idéntico; 1 posting `loan_draw`, 0 operaciones de ingreso/gasto | OK |
| 2B-02 pago 200+20+1 | banco 779, deuda 800, amortizado 200, interés 20, comisión 1, expense 21, net −21, patrimonio −21, cash_flow 779, Δ efectivo −221 | idéntico; 4 postings exactos, principal como `loan_repayment` no gasto | OK |
| 2B-03 cuatro pagos 300+300+300+100 | capital pagado 1.000, deuda 0, rechazo de 1 adicional, sin intereses | idéntico; plazo 2 y cuotas [500,500] inalterados; expense 0; cash_flow 0 | OK |
| 2B-04 anulación de pago | banco 1000, deuda 1000, expense/net/patrimonio 0, cash_flow 1000, 3 tombstones, historial 7 filas (3 con `previous`), sin pagos | idéntico; replay del mismo `request_id` sin efectos nuevos | OK |
| 2B-05 rechazos | 400 para interés/comisión sin categoría, capital > deuda, fecha previa, cuenta tarjeta, todo en cero, total > SAFE; 404 para préstamo ajeno/inexistente | idéntico; 0 pagos nuevos, 1 posting (el desembolso), saldo 1000, `credit_requests` sin la solicitud rechazada; el ID rechazado se reutiliza con éxito después | OK |
| 2B-06 idempotencia | A 204 y 1 pago; B 204 sin duplicar; C 409; deuda 800; 1 fila en `credit_requests` | idéntico | OK |
| 2B-07 V1/V2 | V1 interés 408.333.333.333.333 y V2 …334; ambos aceptan pago y anulación; versiones 1 y 2 intactas; sin pagos tras anular | idéntico | OK |

### Reglas verificadas y comprobaciones contables

- `loan_draw` aumenta efectivo y crea deuda, pero no es ingreso ni gasto; el patrimonio se mantiene restando `loan_debt`.
- El pago separa capital (`loan_repayment`, no gasto), interés y comisión (`expense` con categoría); el patrimonio baja solo por interés y comisión.
- El pago extraordinario reduce saldo sin recalcular cuotas ni plazo; un pago adicional sobre deuda cero se rechaza.
- La anulación revierte exactamente los tres componentes, conserva el historial y no duplica efectos al repetirse.
- Las nueve validaciones rechazadas no dejan postings parciales ni consumen un `request_id` válido.
- Cada escenario ejecuta `PRAGMA integrity_check=ok` y `PRAGMA foreign_key_check=0`, además de las invariantes contables (capital inicial − pagado = pendiente; Σ postings = saldo).

### Mutaciones (restauradas; diff limpio)

| Mutación | Pruebas que fallaron |
|---|---|
| `outstanding = principal` (ignorar pagos) | B02, B03, B06 (3) |
| Publicar capital amortizado como `expense` | B02, B03, B04, B05, B06, B07 (6) |
| No revertir el interés al anular | B04 (1) |

### Resultado y riesgos

No se detectaron defectos: el backend ya aplica estas reglas. No se modificó código de producción, fórmulas de amortización ni `creditProjection` del frontend (queda para F9.2D). Riesgos para F9.2C y F9.2D: compra en cuotas con cargo al resumen, asignación de pagos por mes y coherencia entre `creditProjection`, `cardStatement` y la cartera del servidor.

Validación: `cargo test --locked --test credit_flows` (7/7), `cargo fmt --check`, `cargo clippy --all-targets --locked -- -D warnings`, `cargo test --locked` (**85 Rust**: 41 lib, 14 auth, 7 credit_flows, 2 credit_migration, 8 finance_api, 6 finance_invariants, 3 full_flow, 4 import_load), `npm test` (**40 JS**), build y suite Docker: todo en verde. Cero pruebas ignoradas.

## F9.2C — Tarjetas de crédito

Se amplió `tests/credit_flows.rs` con 7 escenarios (total 14 en el archivo) y se creó `tests/card_statement.test.mjs` (6 pruebas). Contratos confirmados: `POST /api/credit` → 204/400/404/409; `POST /api/operations` → 200 al aplicar; `GET /api/credit` → 200. Oráculos literales y SQL directo; cero cambios de producción.

| ID | Esperado = observado | Estado |
|---|---|---|
| 2C-01 pago de deuda 300 | banco 700, tarjeta 0, patrimonio 700, income/expense/net 0, cash_flow −300; postings banco −300 y tarjeta −300 | OK |
| 2C-02 compra 300 + pago | tras compra: deuda 300, patrimonio 700, expense 300, cash_flow 0; tras pago: banco 700, deuda 0, patrimonio 700, expense 300 (una vez), cash_flow −300 | OK |
| 2C-03 compra en 3 cuotas | 1 sola operación, 1 compra, cuotas [100,100,100] en 2026-01-15/02-15/03-15, suma 300; pago 100 → deuda 200; sin operaciones ficticias por cuota | OK |
| 2C-04 anulación de compra | deuda 0, operación tombstoned, compra fuera de cartera, historial 2 filas, replay sin efectos, usuario ajeno 404; con reembolso de 50 → 400 y gasto 250 | OK |
| 2C-05 asignaciones | 70 ene + 30 feb; exceso 40 → 400, versión obsoleta → 409; editar pago a 60 borra solo sus asignaciones (queda la del otro pago) | OK |
| 2C-06 cierre y vencimiento | ciclo `(día de cierre del mes anterior, día de cierre del mes]`; vencimiento mes siguiente si `due_day <= closing_day`; bisiesto, días 29/30/31 y paso de diciembre a enero | OK |
| 2C-07 saldo inicial | backend: saldo de tarjeta positivo = deuda (patrimonio −300); negativo = crédito (patrimonio +300) | OK (backend) |

### Semántica verificada

- Deuda total = saldo contable de la tarjeta en el backend (`finance_postings` invierte el signo de las cuentas `card`).
- Las cuotas de una compra financiada son **solo proyección** en `credit_purchases`; no generan operaciones ni deudas separadas.
- El extracto (`cardStatement`) carga cada cuota cuyo `first_due + índice` cae en el ciclo; no es igual a la deuda total ni al pendiente del extracto.
- `paid` del extracto solo descuenta asignaciones con `payment_version` igual a la versión actual del pago.
- `first_due` es la fecha de la primera cuota projected, **no** el cierre ni el vencimiento del extracto (calculados desde `closing_day`/`due_day`).

### Ambigüedad confirmada (sin cambio de código)

El backend trata `initial_balance > 0` en tarjeta como deuda y `< 0` como crédito; `cardStatement` (`static/credit.mjs:78-80`) invierte ese signo al atravesar un `cutoff_date`: con `-500` muestra deuda de apertura 500 y con `+500` muestra `-500` de cargos. Es una inconsistencia contable/visual heredada de importaciones históricas. Se dejó documentada y cubierta por pruebas de ambos signos; corregirla cambiaría extractos ya mostrados y requiere decisión de negocio. Propuesta compatible: usar el saldo positivo como deuda de apertura en el ciclo del corte y migrar solo la presentación, nunca los datos.

### Mutaciones (restauradas; diff limpio)

| Mutación | Prueba que falló |
|---|---|
| Doble contabilización de compra financiada en `cardStatement` | `financed purchase charges one installment per cycle` |
| Signo del pago invertido en el extracto | `only allocations with the current payment version...` |
| Cuota asignada a período incorrecto | `financed purchase...` |
| Asignación por encima del total del pago | `c05_monthly_allocations_follow_payment_version` |

### Resultado y riesgos

No se detectaron defectos nuevos en backend. Riesgo pendiente para F9.2D: `creditProjection` mezcla deuda de tarjeta, cuotas de préstamos y proyecciones; aún no se compara contra la cartera del servidor. Riesgo histórico: la ambigüedad de signo del saldo inicial de tarjetas importadas.

Validación: `cargo test --locked --test credit_flows` (14/14), `node --test tests/card_statement.test.mjs` (6/6), `cargo fmt --check`, `clippy -D warnings`, `cargo test --locked` (**92 Rust**: 41 lib, 14 auth, 14 credit_flows, 2 credit_migration, 8 finance_api, 6 finance_invariants, 3 full_flow, 4 import_load), `npm test` (**46 JS**), build y suite Docker: todo en verde. Cero pruebas ignoradas. README sin cambios: no hay regla funcional nueva para el usuario.

## F9.2D — Cartera, proyecciones y frontend

Fixture compartido `tests/fixtures/f92d.json` (versión 1): Rust lo consume con `include_str!` para verificar SQLite/API y JavaScript con `readFileSync` para verificar `creditProjection()` y `cardStatement()`. Los esperados son literales e independientes; se usan `BigInt` para cálculos intermedios y se comprueba `Number.MAX_SAFE_INTEGER`.

| ID | Regla | Esperado = observado | Fuente | Estado |
|---|---|---|---|---|
| D01 | Préstamo V2 1.200.000 @12% nominal, 3 cuotas, primer pago | cuotas 396.027+12.000 / 399.987+8.040 / 403.986+4.040; deuda 803.973; informe ene net −12.000, cash_flow 791.973, patrimonio −12.000; proyecciones 0 / 408.027 / 408.026 | fixture + SQL directo | OK |
| D02 | 300+300+300+100 sobre préstamo de 1.000 | deuda 0, total amortizado 1.000, tabla original [334,333,333], proyección 0; con 900 pagados, proyección 100 | fixture + FIFO independiente | OK |
| D03 | Pagos anticipados, parciales y tardíos | la proyección usa pagos con `date <= fin_del_mes` y FIFO; ene 0, feb 408.027, mar 408.026 aunque el pago de la 2ª cuota sea 5-mar | fechas literales | OK |
| D04 | V1 vs V2 con principal 7.000.000.000.000.003 | interés V1 …333 y V2 …334; `creditProjection` usa la tabla devuelta sin recalcular; ambas cifras < MAX_SAFE_INTEGER | fixture + backend | OK |
| D05 | Tarjetas importadas | backend: +300 deuda / −300 crédito; extracto y proyección usan el mismo signo tras la corrección; cutoff en el límite inclusive; sin cutoff no muestra apertura | fixture + SQL + `cardStatement` | **DEFECTO CORREGIDO** |
| D06 | Compra financiada 300 en 3×100 | gasto 300 una vez, cuotas 100 por ciclo, ningún gasto ficticio; primera cuota posterior al cierre pasa al ciclo siguiente; dos compras suman 200; cruce de año | `cardStatement` + F9.2C | OK |
| D07 | Pago 100 asignado 70 ene + 30 feb | deuda total baja 100; sin asignación al mes, el extracto sigue mostrando pendiente 100; con asignación vigente, 0 | `cardStatement` + `creditProjection` | OK |
| D08 | Operaciones pendientes/conflicto/eliminadas | no se contabilizan como pagos confirmados; la proyección filtra `pending`/`conflict` y el extracto ignora `deleted` | `view-model` + `credit.mjs` | OK |

### Defecto D05 — corrección mínima

`cardStatement()` (`static/credit.mjs:78-82`) invertía `initial_balance` en el ciclo del `cutoff_date`: con `+300` (deuda en el backend) mostraba `−300` de cargos y `outstanding −300`, y con `−300` (crédito) mostraba deuda 300. **Confirmado y corregido**: ahora `charges += initial_balance` y `opening = initial_balance` solo si es positivo. No se tocaron saldos, migraciones, `finance_postings` ni los algoritmos V1/V2. Contrato de retorno sin cambios. Riesgo documentado: los extractos históricos mostrados antes de la corrección pueden cambiar al recalcularse; no se altera ningún dato persistido.

### Matriz de conceptos (no equivalentes)

Deuda total = saldo de tarjeta; saldo bancario = efectivo; cuotas contractuales = tabla V1/V2; cuotas pagadas = `credit_repayments`; vencido = obligación de meses anteriores; importe del extracto = cargos del ciclo menos asignaciones vigentes; proyección = obligaciones del mes seleccionado al cierre de ese mes; resultado mensual y patrimonio = informe del servidor. El fixture mantiene estos conceptos separados.

### Mutaciones (restauradas, diff limpio)

| Mutación | Prueba que falló |
|---|---|
| No tratar préstamos liquidados (`paidCapital=0` si `settled`) | D02 |
| Ignorar pagos parciales (`paidCapital=0n`) | D01, D02, D03 |
| Invertir el signo del saldo inicial de tarjeta | D05 + 3 pruebas de extracto |
| Desplazar una cuota a otro período (quitar filtro de mes) | D01 |

### Verificación

`cargo test --locked --test credit_flows` (16/16), `npm test` (55 JS: 46 previas + 7 de `creditProjection` + 2 nuevas de extracto), `cargo fmt --check`, `clippy -D warnings`, `cargo test --locked` (**94 Rust**: 41 lib, 14 auth, 16 credit_flows, 2 credit_migration, 8 finance_api, 6 finance_invariants, 3 full_flow, 4 import_load), build y suite Docker: todo en verde. Cero pruebas ignoradas. `PRAGMA integrity_check` y `foreign_key_check` en los escenarios Rust.

**Sin verificación en navegador**: no se usó Chrome DevTools; la correspondencia backend/frontend se comprobó con el fixture compartido y pruebas deterministas. Queda sin comprobar el render real de la tabla de proyección en `planning.mjs`.

Riesgos para F9.3: la proyección no modela intereses futuros sobre saldos pendientes (usa la tabla contractual original) y `creditProjection` ignora la deuda de tarjetas sin `cutoff_date`; ambos son comportamientos actuales documentados.

## F9.3A — Saldos importados y aceptación explícita

### Política aprobada (Opción 2)

Toda discrepancia entre `saldo_actual` y el saldo calculado requiere aceptación explícita vinculada a la vista previa revisada. La aceptación nunca ajusta saldos ni crea operaciones; solo autoriza el commit de la importación. Sin discrepancias, el flujo queda igual que antes.

### Implementación

- `Upload` incorpora `ack_saldo: Option<String>` con `#[serde(default)]` (`src/exchange.rs`).
- `Preview` incorpora `differences: Vec<BalanceDiscrepancy>` con `account_id`, `declared_balance`, `calculated_balance` y `difference` en enteros exactos; los `warnings` de texto se conservan por compatibilidad.
- La confirmación exige `ack_saldo == preview_token` cuando hay diferencias; los errores siguen bloqueando aunque exista aceptación. El token ya incluye archivo, usuario, mapeos, flag de corte y el reporte completo, por lo que una aceptación no puede autorizar otra vista previa.
- `static/extras.mjs` muestra una sección «Diferencias de saldo a aceptar» con importes formateados, una casilla inicialmente desmarcada y el botón de confirmación deshabilitado hasta aceptar. Cambiar archivo, mapeo o flag de corte invalida la aceptación. No se envía `ack_saldo` cuando no hay diferencias.

### Oráculos corregidos

El escenario A del plan se corrigió: saldo inicial 1.000 con gasto 100 produce saldo calculado 900; con `saldo_actual=900` no hay diferencia. Los casos con `saldo_actual` 1.000 y 800 cubren diferencias positivas y negativas.

| Caso | Esperado = observado | Ack requerido |
|---|---|---|
| A coincidencia | calculado 900, declarado 900, `differences=[]` | No |
| B declarado superior | declarado 1000, calculado 900, diferencia 100 | Sí; sin ack o con ack incorrecto → 409 y 0 escrituras |
| C declarado inferior | declarado 800, calculado 900, diferencia −100 | Sí; el saldo queda 900 |
| D tarjeta positiva | declarado 300, calculado 300 (deuda) | No |
| E tarjeta negativa | declarado −300, calculado −300 (crédito) | No |
| F fecha de corte | movimiento anterior excluido del saldo; aviso de histórico | No |
| G cuenta existente vinculada | declarado 800, calculado 900 por operaciones previas | Sí |
| H múltiples cuentas | solo la discrepante aparece en `differences` | Sí |
| I saldo_actual ausente | `differences=[]` | No |
| J historial parcial | declarado 500, calculado 900, diferencia −400 | Sí; no se crea ajuste |

Casos adicionales verificados: ack de otra vista previa → 409; archivo o mapeo modificado → 409; errores con ack → 409 y 0 escrituras; confirmación repetida tras commit → 409 sin duplicar; usuario ajeno con token de otro → 409; vista previa sin diferencias confirmada sin ack; peticiones directas a la API sin interfaz.

### Compatibilidad

- Campos aditivos: clientes antiguos pueden seguir enviando `Upload` sin `ack_saldo`; solo verán 409 si intentan confirmar una vista previa con diferencias, que es la política aprobada. `Preview` agrega `differences` sin quitar campos.
- No hay migraciones ni cambios en `import_sources`, `finance_postings` o V1/V2.

### Mutaciones (todas detectadas y restauradas)

| Mutación | Prueba que falló |
|---|---|
| Quitar la validación de `ack_saldo` en el servidor | B (confirmó sin aceptación) |
| Habilitar la confirmación sin aceptación en la interfaz | Prueba JS de diferencias |
| Aceptar cualquier ack no vacío (reutilizar una vista vieja) | `stale_ack_from_another_preview_is_rejected` |
| Invertir el signo de `difference` | C (−100 vs 100) |

### Atomicidad verificada

Preview no persiste; confirmaciones rechazadas dejan 0 operaciones; los errores bloquean el commit aunque exista ack; la confirmación autorizada persiste exactamente las filas previstas; los savepoints por fila se confirman definitivamente con la transacción externa. `integrity_check=ok` y `foreign_key_check=0` en cada escenario.

### Validación

`cargo test --locked --test import_integrity` (15/15), `node --test tests/import_ui.test.mjs` (5/5), `cargo fmt --check`, `clippy -D warnings`, `cargo test --locked` (**109 Rust**: 41 lib, 14 auth, 16 credit_flows, 2 credit_migration, 8 finance_api, 6 finance_invariants, 3 full_flow, 15 import_integrity, 4 import_load), `npm test` (**60 JS**), build y suite Docker: todo en verde. Cero pruebas ignoradas.

### Riesgos residuales

- La causa intermitente de `SQLITE_BUSY` de F8.4 sigue abierta; F9.3C la medirá.
- Un cliente antiguo que intente confirmar diferencias recibirá 409 hasta actualizarse: es el comportamiento aprobado.
- F9.3B (mapeos de categorías y referencias) y F9.3D (conciliación CSV) quedan pendientes.

## F9.3B — Referencias externas, mapeos e idempotencia XLSX

### Reglas implementadas

- Tres espacios de identificadores separados: cuentas (`mapping`), categorías (`category_mapping`) y operaciones (`operation_mapping`), todos con `serde(default)`.
- `import_sources(user_id, entity, source_id)` sigue siendo el registro de vínculos; no se crearon tablas ni migraciones.
- Un `category_mapping` valida que la categoría interna exista, pertenezca al usuario y tenga el tipo esperado; un vínculo persistido no puede redirigirse silenciosamente.
- Un `operation_mapping` valida que la operación interna exista y pertenece al usuario; el vínculo usado se persiste en `import_sources` para que archivos posteriores lo resuelvan sin repetir el mapeo.
- `movimiento_relacionado_id` se resuelve en este orden: `import_sources` → `operation_mapping` → operación interna directa del usuario; si no existe, la fila se rechaza y el preview expone `references` estructuradas.
- `preview_token` ahora incluye `category_mapping` y `operation_mapping`, además de archivo, usuario, mapeo de cuentas, flag de corte y reporte. `ack_saldo` de F9.3A se conserva.
- La UI ofrece selectores para vincular categorías externas y referencias sin resolver (máximo 50 gastos candidatos); cambiar cualquier vínculo invalida la vista previa y la aceptación de saldos.

### Casos B01–B20 (esperado = observado)

| Caso | Resultado |
|---|---|
| B01 cuenta externa vinculada | 1 cuenta interna, movimiento en `banco`, vínculo en `import_sources` |
| B02 categoría externa vinculada | 1 categoría interna, movimiento con `category_id=food`, vínculo persistido |
| B03 categoría externa nueva | categoría `cat-food` creada y usada |
| B04 gasto + reintegro en un archivo | 2 operaciones, patrimonio −300, relación `r1→e1` |
| B04b reintegro antes del gasto | el importador ordena reintegros al final; sin error |
| B05 reintegro a operación importada antes | resuelve vía `import_sources` |
| B06 `operation_mapping` explícito | `r1→expense-existing` |
| B06b vínculo persistido sin mapeo repetido | nuevo reintegro `r2` resuelve `e1→expense-existing` |
| B07 referencia inexistente | error + `references` estructurada; 0 escrituras |
| B08 referencia de otro usuario | error; 0 escrituras para el usuario importador |
| B09 mismo `source_id` en 3 entidades | 3 filas `import_sources`, sin colisión |
| B10 reimportación idéntica | `duplicates=3`, `added=0`, sin duplicar movimientos |
| B11 fila modificada | error de huella distinta; historial intacto |
| B12 redirección de vínculo persistido | rechazada; vínculo original conservado |
| B13 duplicados internos | error; confirmación bloqueada; 0 escrituras |
| B14 categoría archivada vinculada | permitido por las reglas actuales de importación (documentado) |
| B15 movimiento anterior al corte | registrado pero fuera del saldo |
| B16 categoría después de cuenta vinculada | ambos mapeos resuelven en el mismo archivo |
| B17 dos reintegros del mismo gasto | 500−200−200, patrimonio −100 |
| B18 reintegros que exceden el gasto | error; confirmación bloqueada; 0 escrituras |
| B19 mapeo distinto al preview | 409; 0 escrituras |
| B19b `operation_mapping` no usado pero distinto | el token cambia; 409 |
| B20 25.000 movimientos con mapeos | `added=25002`, 0 errores, 25.000 operaciones con `category_id=food` |

`added` sigue contando filas procesadas de cualquier entidad: 25.000 movimientos + 1 cuenta + 1 categoría = 25.002. No se añadió un desglose nuevo para no romper compatibilidad; los conteos por entidad se verifican por SQL en las pruebas.

### Mutaciones (todas detectadas y restauradas)

| Mutación | Prueba que falló |
|---|---|
| Aplicar el mapping de cuentas a categorías | B02 |
| Ignorar `import_sources` al resolver referencias | B06b |
| Permitir redirigir un vínculo persistido | B12 |
| Omitir los nuevos mappings del `preview_token` | B19b |

### Validación

`cargo test --locked --test import_integrity` (38/38), `node --test tests/import_ui.test.mjs` (7/7), `cargo fmt --check`, `clippy -D warnings`, `cargo test --locked` (**132 Rust**: 41 lib, 14 auth, 16 credit_flows, 2 credit_migration, 8 finance_api, 6 finance_invariants, 3 full_flow, 38 import_integrity, 4 import_load), `npm test` (**62 JS**), build y suite Docker: todo en verde. Cero pruebas ignoradas; `integrity_check=ok` y `foreign_key_check=0` en cada escenario.

### Riesgos residuales

- La causa intermitente de `SQLITE_BUSY` de F8.4 sigue abierta; F9.3C la medirá con respaldo concurrente.
- Vincular una categoría archivada sigue permitido porque las reglas de importación actuales no aplican el filtro de archivadas; se documenta como decisión pendiente, no como defecto.
- La UI de referencias muestra hasta 50 gastos candidatos; un gasto más antiguo requiere usar la API con `operation_mapping` explícito.

## F9.3C — Atomicidad, concurrencia SQLite y respaldos

### Modelo de concurrencia verificado

- FinApp usa **una conexión SQLite** compartida por `Arc<Mutex<Connection>>` (`src/db.rs:9`); todas las peticiones HTTP se serializan por ese mutex.
- La única contención SQLite real viene de **conexiones externas**: el script de respaldo, un segundo proceso o una herramienta administrativa.
- La transacción de importación es DEFERRED (`src/exchange.rs:698-700`); cada fila usa un savepoint y el commit definitivo es el de la transacción externa.
- `busy_timeout` efectivo: 5 s (rusqlite). En promoción de lectura a escritura DEFERRED, SQLite devuelve `SQLITE_BUSY` inmediatamente y no aplica ese timeout (confirmado en F8.4 y reproducido en C08/C09).
- El respaldo usa `sqlite3 .backup` dentro del contenedor (`scripts/backup.sh:26`) y verifica `integrity_check` antes de copiar; la restauración aislada usa `scripts/restore-check.sh`.

### Matriz C01–C12

| Caso | Resultado | Observado |
|---|---|---|
| C01 importación normal | preview sin persistencia; confirmación exacta; 0 escrituras parciales | OK |
| C02 fila inválida | error en preview; confirmación bloqueada; 0 operaciones | OK |
| C03 error dentro de un savepoint | fila revertida; transacción externa sin commit; 0 operaciones | OK |
| C04 error SQLite después de varias filas | trigger de prueba aborta la fila 4; confirmación bloqueada; 0 operaciones | OK |
| C05 error de commit | **no reproducible de forma determinista** sin mockear SQLite; se documenta la limitación y se cubre el caso equivalente con C04 | LIMITACIÓN |
| C06 confirmación repetida | 409; 1 operación; sin duplicados | OK |
| C07 cambio entre preview y confirmación | token recalculado distinto → 409; 0 escrituras | OK |
| C08 bloqueos entre conexiones | lector + escritor: 5,54 s hasta `SQLITE_BUSY`; escritor externo + importador: `SQLITE_BUSY` inmediato en promoción DEFERRED; IMMEDIATE espera 5,53 s | OK |
| C09 DEFERRED vs IMMEDIATE | DEFERRED falla en 164 µs en promoción; IMMEDIATE espera 5,53 s y falla con `SQLITE_BUSY` tras el timeout | OK |
| C10 25.000 movimientos | 25.000 operaciones, 25.002 `import_sources`, `integrity_check=ok`, `foreign_key_check=0`; suite en 20,8 s | OK |
| C11 respaldo concurrente | respaldo verificado con SHA-256 e `integrity_check=ok`; importación confirmada en 0,997 s; el respaldo capturó el estado anterior (7.000 operaciones) y la importación añadió 5.000 más | OK |
| C12 restauración aislada | `restore-check.sh` verificó integridad y `/health` con la imagen `finapp:local` | OK |

### Comparativa DEFERRED / IMMEDIATE

| Estrategia | Bajo escritor externo | Tiempo | Conclusión |
|---|---|---|---|
| DEFERRED | falla inmediatamente en la promoción lectura→escritura | 164 µs | actual, predecible pero sin espera |
| IMMEDIATE | espera `busy_timeout` y falla con `SQLITE_BUSY` | 5,53 s | más tolerante, pero cambia el contrato público (409 → 500) y retiene el bloqueo más tiempo |

**Decisión:** se mantiene DEFERRED. El cambio a IMMEDIATE mejoraría la tolerancia, pero alteraría el contrato HTTP actual (409 «La vista previa cambió» → 500 «No se pudo consultar la base de datos») y aumentaría la retención del bloqueo durante importaciones largas. El incidente histórico sigue sin reproducirse; no hay evidencia suficiente para cambiar el contrato.

### Respaldo y restauración (stack sintético aislado)

- Compose proyecto exclusivo `finapp-f93c-20261121003908`, volumen `finapp-f93c-20261121003908_f93c_data`, red propia, sin puertos publicados, imagen `finapp:local`.
- Respaldo 1 (estado 2.000 movimientos): `finapp-20261121T034357Z-4Flx0e00.sqlite`, 4.308.992 bytes, SHA-256 verificado, `integrity_check=ok`, `foreign_key_check=0`, `restore-check.sh` OK.
- Respaldo 2 (concurrente con importación de 5.000 movimientos): `finapp-20261121T034639Z-jf2fTZkQ.sqlite`, 14.524.416 bytes, SHA-256 verificado, `integrity_check=ok`, `foreign_key_check=0`, `restore-check.sh` OK; el respaldo contiene 7.000 operaciones (estado anterior al commit concurrente).
- El respaldo se ejecutó dentro de un contenedor `docker:cli` con el socket de Docker montado, porque Git Bash en Windows traduce `/data` a una ruta local y rompe `backup.sh`. El stack sintético se destruyó con `docker compose down --volumes` y el directorio temporal se eliminó.

### Incidente histórico (fila ~19.614)

**No reproducido.** Se repitieron importaciones de 25.000 movimientos y escenarios de contención controlada; el fallo intermitente no reapareció. Se mantiene clasificado como hipótesis compatible con `SQLITE_BUSY` por promoción DEFERRED bajo un escritor externo, pero sin confirmación.

### Mutaciones (restauradas; diff limpio)

| Mutación | Prueba que falló | Nota |
|---|---|---|
| Confirmar el savepoint de una fila fallida | ninguna | la transacción externa sigue revirtiendo; evidencia de doble barrera |
| Ignorar los errores de fila | C02 | detectada |
| Aceptar un `preview_token` desactualizado | C07 | detectada |
| Ignorar un error real de commit | no ejecutada | no reproducible sin mockear SQLite; documentado como limitación |

### Validación

`cargo test --locked --test import_concurrency` (8/8), `cargo fmt --check`, `clippy -D warnings`, `cargo test --locked` (**140 Rust**: 41 lib, 14 auth, 16 credit_flows, 2 credit_migration, 8 finance_api, 6 finance_invariants, 3 full_flow, 8 import_concurrency, 38 import_integrity, 4 import_load), `npm test` (**62 JS**), build y suite Docker: todo en verde. Cero pruebas ignoradas.

### Riesgos residuales

- `SQLITE_BUSY` bajo escritor externo sigue produciendo 409 inmediato; es el contrato actual y no se cambió.
- El fallo histórico de la fila ~19.614 sigue sin reproducción.
- No se activó WAL ni se cambió `busy_timeout`; la contención con herramientas externas sigue siendo posible.

## F10.1 — Correcciones funcionales de la fase 10

### Defectos corregidos

- **F10.1A** KPI «Patrimonio neto»: la deuda de tarjeta se sumaba como activo y «Deuda de tarjetas» se coloreaba en rojo solo sin deuda. Ahora el KPI usa `netWorth()` (`static/view-model.mjs`), que aplica la regla del servidor (saldo positivo de tarjeta = deuda, se resta). Atajo documentado en el código: la deuda de préstamos no está en la instantánea offline y no se resta.
- **F10.1B** Importación XLSX `pago_prestamo`: la `categoria_id` no pasaba por `category_mapping`, por lo que la parte de interés usaba el id externo y la fila siempre se rechazaba («Datos inválidos…»). Ahora `loan_repayment` se resuelve como gasto esperado (`src/exchange.rs`).
- **F10.1C** Créditos: dos `JSON.parse` de `localStorage` sin protección mostraban el `SyntaxError` crudo con un valor corrupto. Ahora usan `lastIdentity()` de `static/ui.mjs` y degradan al mensaje de sesión.
- **F10.1D** Etiqueta de mes del KPI: se formateaba con la zona horaria del navegador; ahora usa `America/Asuncion` desde el mes ya calculado, siempre coincide con los datos.
- **F10.1E** Endpoint muerto `/api/history` eliminado: grep previo del árbol completo sin llamadores. Borrados `src/history.rs` y su `merge` en `src/lib.rs` (−170 líneas).
- **F10.1F** Duplicados del frontend unificados en `static/ui.mjs`: formateador de dinero (5 copias), creador de elementos (5), `field()` de credit/planning (acepta pares `[id,nombre]` y objetos), bucle de bloques de 8192 bytes (base64/hex) y lectura de identidad (3). `section()` no se unificó porque difiere (`details` vs `section`); los botones no se unificaron porque difieren en manejo de errores y clase CSS. `static/sw.js` ahora incluye `/ui.mjs` en el shell.

### Pruebas incorporadas

- `tests/import_integrity.rs`: `f10_loan_repayment_interest_uses_the_linked_category` — pago con interés 21 y categoría vinculada se previsualiza sin errores, se confirma, crea el capital (`loan_repayment` 200) y el gasto con la categoría interna, saldo −221, `integrity_check` ok.
- `tests/view-model.test.mjs`: `netWorth` resta deuda de tarjeta, trata un crédito de tarjeta (saldo negativo) como activo y devuelve 0 sin cuentas.
- `tests/credit-idempotency.test.mjs`: `corrupted localStorage degrades to the session message` — sin POST, mensaje de sesión en lugar del SyntaxError.

### Mutaciones (detectadas y revertidas)

| Mutación | Prueba que falló |
|---|---|
| `netWorth` sin invertir el signo de tarjeta | patrimonio del servidor (view-model) |
| `lastIdentity` sin try/catch | localStorage corrupto (credit-idempotency G) |
| (pre-corrección) `loan_repayment` sin mapeo de categoría | `f10_loan_repayment_interest_uses_the_linked_category` falló antes del fix |

### Validación

`cargo fmt --check` limpio, `clippy --all-targets --locked -- -D warnings` exit 0, `cargo test --locked` **140 Rust** exit 0 (41→40 lib por el módulo borrado, 38→39 import_integrity), `npm test` **64 JS** exit 0. Diff neto −100 líneas. Sin cambios en migraciones, reglas financieras ni contratos HTTP existentes.

### Pendiente de F10

F10.2 (validación en producción: despliegue con `FINAPP_VERSION`, XLSX real, Android físico, copia programada fuera del servidor, vigilancia del error ~19.614) es operativo y queda a cargo del administrador del despliegue.

## F11 — UX/UI premium

### Evaluación previa

Antes de escribir código se verificó cada ítem contra el estado real de la interfaz:

- **P2.1 micro-interacciones**: ya cubierto — `:active` con desplazamiento, transiciones de 120 ms, guard de `prefers-reduced-motion`, feedback de guardado por texto (`role="status"`) sin saltos de diseño. Sin cambios.
- **P2.2 estados vacíos**: ya cubierto — cada sección vacía nombra la acción siguiente. Sin cambios.
- **P2.4 virtualización del historial**: premisa inválida — `historyPage()` pagina a 25 filas y el listado nunca renderiza más. Ítem cerrado sin código.

### Cambios aplicados

- **P2.3 modo instalado**: `viewport-fit=cover` en `index.html`; `env(safe-area-inset-top)` en la barra de marca, insets laterales en `main` (móvil) e inferior en `body` (`style.css`). El contenido ya no queda bajo notch ni indicador de inicio.
- **P2.5 gráfico por categoría**: el informe mensual reemplaza la tabla duplicada de categorías por barras HTML (`planning.mjs` + `.bars/.bar-row/.bar` en `style.css`): proporción sobre el gasto máximo, importe siempre en texto (nunca solo color), sin dependencias.

### Validación

`npm test` **64 JS** exit 0 (sin regresiones); `npm audit` **0 vulnerabilidades** (P3.3 parcial); sin cambios en Rust (no se reejecutó la suite completa por ausencia de cambios en `src/` y `tests/*.rs` — la última corrida completa de F10.1 quedó en 140 exit 0).

### Sin verificar en navegador

Las barras y los safe-areas no se abrieron en Chrome real; corresponden a la lista de F10.2 (comprobación en dispositivo) y al chequeo visual pendiente.

### Pendiente de seguridad (P3)

P3.1 (cifrar la copia externa), P3.4 (revisión de accesos y rotación de bootstrap) son operativos; P3.2 (purga de tablas de idempotencia) queda condicionada a que la base pase de ~100 MB; P3.3 (cargo audit) requiere instalar `cargo-audit`, no instalado sin autorización.

## F12 — Cierre de seguridad y corrección de borrados

### F12.0 — Borrados inexistentes responden 404

`RemoveGoal`, `RemoveRecurring`, `RemoveBudget` y el DELETE de comprobantes devolvían 204 aunque no existiera el ítem. Ahora el helper `removed()` de `src/planning.rs` comprueba las filas afectadas y devuelve `Error::Missing` (404), igual que `reconciliation.rs`. Prueba `missing_deletes_answer_404` en `tests/full_flow.rs` (4 pasadas: los tres planes y el comprobante con id fantasma, más el ciclo 204→404 de un objetivo real). Mutación con la lógica invertida: la prueba falla; revertida.

### F12.1 — Auditoría de dependencias (P3.3)

`cargo audit` inicial: **10 vulnerabilidades** — calamine 0.31 (OOM/panic en el parser, alta), quick-xml 0.38 (dos altas, lector XML de los XLSX subidos), libsqlite3-sys 0.35 (CVE de SQLite C, media) y 6 avisos de zip 4.6 (todos sobre `extract()` a disco, **no alcanzable**: FinApp nunca extrae, valida el tamaño en streaming y calamine lee en memoria). Corregido subiendo:

| Crate | Antes | Ahora |
|---|---|---|
| calamine | 0.31 | 0.38 |
| rusqlite (libsqlite3-sys) | 0.37 (0.35) | 0.43 (0.39+) |
| zip | =4.6.0 | 9.8.1 |

Únicos cambios de código: `usize` dejó de implementar `FromSql` en rusqlite 0.43 (una lectura de tamaño de comprobante pasa por `i64`) y calamine 0.38 resuelve el libro por `_rels/.rels`, que los cinco libros sintéticos de pruebas ahora incluyen (`PACKAGE_RELS`). `cargo audit` y `npm audit`: **0 vulnerabilidades**; único restante el aviso `chrono` sin mantener (transitorio, sin corrección disponible). No se añadió `cargo audit` a CI; queda como decisión pendiente si se quiere automático.

### F12.2 — Copia externa cifrada (P3.1, documentación)

El README documenta el flujo `gpg --symmetric` para la copia externa (cifrado, verificación del checksum y ensayo con `restore-check.sh`) y cómo encadenarlo al timer systemd. El ensayo real en el equipo externo queda a cargo del administrador.

### Validación F12

`cargo fmt --check` limpio, `clippy --all-targets --locked -- -D warnings` exit 0, `cargo test --locked` **141 Rust** exit 0 (140 + la nueva de borrados), `npm test` sin cambios ni reejecución necesaria (sin cambios en JS), `cargo audit` exit 0.

### Pendiente tras F12

F10.2 (validación en producción: despliegue con `FINAPP_VERSION`, XLSX real, Android físico, copia programada verificada, ensayo de descifrado de la copia cifrada) y P3.4 (revisión de accesos Docker y rotación de credenciales de bootstrap) son operativos. P3.2 sigue condicionada a la base por encima de ~100 MB.

## F13 — Pulido visual premium

### Alcance (elegido por el usuario)

Solo visual: iconos SVG propios, micro-movimiento con significado y jerarquía refinada. **No incluido** (decisión del usuario): errores junto al campo, instalación in-app e interruptor de tema.

### Cambios

- Seis iconos SVG inline en las pestañas (`static/index.html`) y cuatro KPIs (`icon()` en `static/ui.mjs`, llamado desde `renderKpis`): trazo `currentColor`, 1,1 em, `aria-hidden`, texto como etiqueta accesible. Sin dependencias.
- `.kpi .label` en mayúsculas con `letter-spacing: 0.05em`; pestañas con icono+texto alineados.
- Movimiento solo con significado: entrada de vista (160 ms, `view-in`) al cambiar de pestaña y tres pulsos de atención en filas `.conflict`; todo dentro de `@media (prefers-reduced-motion: no-preference)`.
- Corrección encontrada en navegador real: a 360 px los seis botones envolvían en 5 filas desalineadas (45/69 px); cuadrícula de 2 columnas con `align-items: stretch` → 3 filas de 162 px de ancho y altura pareja por fila, `scrollWidth = 360 = viewport`.

### Evidencia en navegador real (Chrome DevTools, servidor local)

- Escritorio: 6 pestañas con 6 SVG (`aria-hidden=true`), 4 KPIs con 4 SVG, etiquetas correctas e importes exactos («Patrimonio neto 1.000.000 Gs.» tras crear una cuenta de prueba).
- Modo claro: trazo del icono `rgb(83, 107, 98)` = `--ink-2`; modo oscuro: `rgb(163, 181, 172)`. Sin errores de consola (solo avisos de `autocomplete` en campos no sensibles).
- `prefers-reduced-motion: reduce` emulado → `animationName: none`; sin emulación → `view-in` 0,16 s.
- Móvil 360 px: 3 filas de pestañas, sin scroll horizontal.
- `viewport-fit=cover` activo en el meta viewport.

### Validación

`npm test` 64/64 exit 0 tras todos los cambios. Sin cambios en Rust. LIMITACIÓN: el modelo no puede inspeccionar píxeles de una captura; la verificación se hizo sobre DOM, estilos computados y geometría del navegador real. El aspecto estético final depende del ojo del usuario en su dispositivo.

## F14 — Rediseño de MisFinanzas

- Navegación lateral y barra inferior con historial, enlaces a secciones y acceso a cambios locales; renovación de acceso, Resumen, registro de movimientos, Créditos, Planificación, Configuración, importación y conciliación.
- Tipografía Source Serif 4 e IBM Plex Sans e ilustración optimizada servidas localmente e incluidas en la caché offline (`sw.js`).
- Verificado en Chromium local en cinco anchos (360–1440 px), modo oscuro, `prefers-reduced-motion`, actualización de service worker con cola pendiente, alta/edición/borrado, conflictos, importación XLSX y conciliación CSV.

## F15 — Optimización de rendimiento del backend

### Cambios aplicados

- **Configuración SQLite (`src/db.rs`)**: se habilitaron `PRAGMA journal_mode=WAL`, `PRAGMA synchronous=NORMAL`, `PRAGMA temp_store=MEMORY` y `set_prepared_statement_cache_capacity(64)` (actualizando la decisión previa de F8.4 donde no se había activado WAL).
- **Sentencias preparadas cacheadas y lectura de columnas (`src/finance.rs`, `src/catalog.rs`, `src/planning.rs`, `src/credit.rs`, `src/auth.rs`)**: reemplazo de `prepare`/`query_row`/`execute` por `prepare_cached` en rutas críticas; lectura directa de columnas de `finance_operations` en `apply_change`, `snapshot_since` y `credit::portfolio` evitando construir y parsear la columna virtual `data`; reemplazo de `json_extract(o.data, ...)` por `o.date` y `o.deleted` en `planning.rs`; omisión de `invalid_refunds` cuando la operación no es `refund` ni actualiza un `expense`; consulta puntual por `(user_id, entity, id)` en `catalog::archive`.
- **Importación XLSX (`src/exchange.rs`)**: `prepare_cached` en consultas por fila, cálculo único de `fingerprint(row)` por fila, caché de `catalog::cutoff` por cuenta durante la importación y lectura directa de `DataRef::String`/`SharedString`.
- **Desbloqueo del runtime asíncrono (`src/auth.rs`, `src/finance.rs`, `src/planning.rs`, `src/credit.rs`, `src/reconciliation.rs`)**: hashing/verificación `Argon2` y handlers que bloquean `Mutex<Connection>` delegados a `tokio::task::spawn_blocking`.

### Validación F15

- `cargo fmt --check` y `cargo clippy --all-targets --locked -- -D warnings` limpios.
- `cargo test --locked`: **141 pruebas Rust** en verde. En perfil `test` (debug), `tests/import_integrity.rs` (39 pruebas) bajó de **31,31 s** a **8,80 s** (~3,5×) y `tests/import_load.rs` (25.000 filas) de **29,15 s** a **7,00 s** (~4,1×).
- Verificación en Chrome real (`http://127.0.0.1:3099/`): carga inicial (`17/17` activos en 200), login, navegación por `Resumen`, `Nuevo`, `Créditos`, `Planificación` y `Configuración`, creación de cuenta y sincronización incremental (`33/33` peticiones HTTP 200, 0 errores en consola).
