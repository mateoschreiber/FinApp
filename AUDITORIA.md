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
