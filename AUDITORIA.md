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
