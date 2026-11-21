# Auditoría general de FinApp

**Las 6 entregas funcionales están completas y la estabilización previa al rediseño visual terminó.** El trabajo se hace en `main`; la rama `estabilizacion-ux` ya no se usa. El contenedor de producción no se ha tocado: pendiente validar en `server1`.

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

1. **Validar en `server1`:** respaldo previo, `FINAPP_VERSION` en el despliegue y comprobación de la actualización PWA en un dispositivo real.
2. **Importación real:** ensayar el XLSX real en instancia de prueba y cuadrar saldos y totales.
3. **Android físico:** instalación PWA, uso sin conexión y rendimiento del historial (10.003 movimientos sintéticos: 1,96 MB iniciales, sincronización sin cambios de 141 bytes).
4. **Copia automática:** confirmar una ejecución programada del respaldo y una copia recuperable fuera de `server1` (al auditar figuraban 0 ejecuciones registradas).
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
