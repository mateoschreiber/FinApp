# Fases de MisFinanzas — implementadas y pendientes

Fecha: 2026-11-21. Fuentes: `docs/superpowers/plans/2026-11-14-misfinanzas-fase-1.md`, `AUDITORIA.md` (F8.4–F9.3C) y la auditoría general del 2026-11-21 (140 pruebas Rust + 62 JavaScript en verde, `cargo test --locked` y `npm test` con código de salida 0).

Criterio de hecho de todo el plan: cada cambio lleva primero una prueba que falla, luego se implementa, y `cargo fmt --check`, `clippy -D warnings`, `cargo test --locked` y `npm test` quedan en verde antes de dar la fase por cerrada.

---

## Parte 1 — Fases implementadas

### Fase 1 oficial (plan de diseño, 4 tareas) — COMPLETA

| Tarea | Contenido | Evidencia |
|---|---|---|
| 1. Servidor y acceso | Axum + SQLite con migraciones 001–009, bootstrap del administrador, Argon2, sesiones `__Host-` HttpOnly, rate-limit global de login (atajos documentados). | `src/main.rs`, `src/auth.rs`, `src/db.rs`, `tests/auth.rs` (14). |
| 2. Libro mayor y sincronización | Operaciones versionadas e idempotentes por `request_id`, saldos enteros exactos, corte de tarjeta, reinteglos topados. | `src/finance.rs`, `migrations/003-004`, `tests/finance_*` (14). |
| 3. Cliente PWA offline | IndexedDB por usuario, cola de cambios con dependencias, resolución manual de conflictos, service worker versionado por commit. | `static/app.mjs`, `sync.mjs`, `offline-store.mjs`, `sw.js`; 62 pruebas JS. |
| 4. Despliegue y recuperación | Docker multi-stage, puerto solo en `127.0.0.1`, `FINAPP_ORIGIN` validado, respaldo consistente con checksum y ensayo de restauración. | `Dockerfile`, `compose.yaml`, `scripts/backup.sh`, `scripts/restore-check.sh`, README. |

### Las 6 entregas funcionales — COMPLETAS

1. Primer uso y catálogos (estado vacío verificado en navegador).
2. Modelo financiero y operaciones (reintegros con 13 aserciones nuevas).
3. PWA y sincronización (actualización versionada verificada dos veces en Chrome real).
4. Tarjetas y préstamos (anulaciones atómicas e idempotentes; interés nominal V2 exacto con `i128`, migración 009; V1 conservado).
5. Planificación e informes (digest de revisión v2 que invalida meses posteriores al editar uno anterior).
6. Importación, conciliación y recuperación (25.000 filas medidas, mapeos de categorías y referencias, aceptación explícita de diferencias de saldo).

### Estabilización y renovación — COMPLETAS

F8.4 observabilidad SQLite · Renovación UX/UI (tokens, modo oscuro, contraste 6,7–13,3) · F9.1A idempotencia (defecto descartado con prueba) · F9.1B exactitud de saldos · F9.2A amortización V1/V2 · F9.2B préstamos · F9.2C tarjetas (defecto D05 corregido) · F9.2D cartera y frontend · F9.3A aceptación de saldos · F9.3B mapeos XLSX · F9.3C atomicidad, concurrencia y respaldos.

---

## Parte 2 — Fases pendientes, por prioridad

Criterio de orden fijado por el usuario: primero **funcionalidad**, luego **UX/UI muy premium**, **seguridad al final**. Las estimaciones son horas de trabajo enfocado, incluidas las pruebas (la suite completa tarda ~5–10 min por corrida).

### P0 — Funcionalidad: correcciones de código (8–12 h, 1–2 jornadas)

| # | Tarea | Archivo | Estimación | Criterio de aceptación |
|---|---|---|---|---|
| P0.1 ✅ | KPI «Patrimonio neto»: la deuda de tarjeta se suma como activo y la deuda de préstamos no aparece; KPI «Deuda de tarjetas» en rojo solo cuando no hay deuda. | `static/app.mjs:169-186` | 1–2 h | Prueba nueva con tarjeta y préstamo: el KPI coincide con `net_worth` del informe del servidor; color rojo con deuda > 0. |
| P0.2 ✅ | Importación XLSX de `pago_prestamo`: la `categoria_id` no se resuelve con `category_mapping`, la fila siempre se rechaza. | `src/exchange.rs:472-478` | 2–3 h | Prueba de regresión en `import_integrity.rs`: pago con interés y categoría vinculada se importa y confirma. |
| P0.3 ✅ | Créditos: `JSON.parse` de `localStorage` sin proteger, con la clave duplicada a mano. Reutilizar `lastIdentity()`. | `static/credit.mjs:156,182` | 1 h | Prueba con `localStorage` corrompido: el formulario guarda y muestra error propio, no el `SyntaxError`. |
| P0.4 ✅ | Etiqueta de mes del KPI usa la zona horaria del navegador, no `America/Asuncion` como los datos. | `static/app.mjs:175-177` | 0,5–1 h | Etiqueta y datos siempre del mismo mes. |
| P0.5 ✅ | Endpoint muerto `/api/history`: nadie lo llama (verificado por grep en `static/`, `tests/` y docs). Borrar módulo y `merge`. | `src/history.rs`, `src/lib.rs:129` | 0,5 h | Suite en verde sin el módulo; −170 líneas. |
| P0.6 ✅ | Cinco copias del formateador de dinero y del creador de elementos; `field()`/`section()` duplicadas; dos codificadores por bloques; dos envoltorios de botón. Un helper común. | `static/app.mjs`, `extras.mjs`, `credit.mjs`, `planning.mjs`, `reconciliation.mjs` | 3–4 h | Refactor mecánico, sin cambio de comportamiento; 62 pruebas JS en verde; −120 líneas. |

### P1 — Funcionalidad: validación en producción (7–14 h)

| # | Tarea | Estimación | Criterio de aceptación |
|---|---|---|---|
| P1.1 | Respaldo previo, desplegar con `FINAPP_VERSION` y comprobar la actualización PWA («Aplicar actualización») en un dispositivo real. | 2–4 h | Respaldo verificado con checksum; la PWA del dispositivo pasa a la versión del commit. |
| P1.2 | Ensayar el XLSX real en instancia de prueba y cuadrar saldos y totales. | 2–4 h | Diferencias de saldo explicadas o aceptadas con la casilla; totales cuadrados contra el origen. |
| P1.3 | Instalar la PWA en un Android físico: uso sin conexión, sincronización al volver y rendimiento del historial (10.003 movimientos sintéticos ya medidos en desktop). | 2–4 h | Alta, baja y edición sin conexión; cola conservada tras recargar. |
| P1.4 | Confirmar una ejecución programada del respaldo (`finapp-backup.timer`) y una copia recuperable **fuera** del servidor (al auditar había 0 ejecuciones registradas). | 1–2 h | `systemctl list-timers` con la última ejecución en verde y un `restore-check.sh` sobre la copia externa. |
| P1.5 | Vigilar el error intermitente de la fila ~19.614 (no reproducido en 6 corridas; ya instrumentado). | 0 h | Si reaparece, usar el registro de etapa/código ya existente; no crear nada nuevo ahora. |

### P2 — UX/UI muy premium (10–21 h)

> Base ya ganada en la renovación anterior: tokens con identidad verde/oro, modo oscuro verificado, `tabular-nums`, guard de `prefers-reduced-motion`, foco verificado. Esta fase es el salto de «correcto» a «premium»: lo que se siente, no lo que se ve.
>
> Nota de método: la guía de esta fase proviene de los valores incorporados de la skill `ui-ux-pro-max` (accesibilidad, interacción táctil, rendimiento, tipografía/color, animación, formularios, navegación) y no de su base de búsqueda, porque Python no está disponible en este equipo; cada ítem se valida de todas formas con pruebas y navegador antes de cerrarse.

| # | Tarea | Estimación | Criterio de aceptación |
|---|---|---|---|
| P2.1 ✅ (ya cubierto) | Micro-interacciones y feedback: respuesta táctil en `:active` (escala 0,98, 100–150 ms), confirmación de guardado no intrusiva, skeleton en lugar de saltos de contenido, transiciones de 150–250 ms solo donde añaden significado. | 0 h | Ya existía: `:active` con `translateY`, transiciones de 120 ms, guard de `prefers-reduced-motion`, feedback por texto sin saltos de diseño (mejor que skeleton para CLS). Verificado en `static/style.css:102-121`. |
| P2.2 ✅ (ya cubierto) | Estados vacíos de nivel premium: cada sección sin datos muestra qué hacer. | 0 h | Ya existía: «Crea una cuenta…», «Elige una categoría…», «Crea una cuenta de tipo Tarjeta…», «Aún no hay líneas de extracto», etc. Verificado en `app.mjs`, `credit.mjs`, `reconciliation.mjs`. |
| P2.3 ✅ | Modo instalado real: `env(safe-area-inset-*)` para notch/barras, `theme-color` dual ya existente, splash y navegación con back predecible. | 1 h | `viewport-fit=cover` en `index.html`; insets en `header`, `main` (móvil/vertical) y `body` (`style.css`). |
| P2.4 ✅ (cerrado) | Rendimiento percibido del historial: virtualizar la lista para >10.000 movimientos en móvil. | 0 h | Premisa inválida: `historyPage()` ya pagina a 25 filas; el listado nunca renderiza más. Nada que hacer. |
| P2.5 ✅ | (Opcional) Informe mensual con un gráfico de barras por categoría, sin dependencias. | 1 h | Barras HTML en `planning.mjs` (proporción sobre el máximo, importe siempre en texto, nunca solo color), estilos en `style.css`; reemplaza la tabla duplicada de categorías. |

### F13 — Pulido visual premium (ampliación de P2, ejecutada)

Iconos SVG propios e inline (6 pestañas en `index.html`, 4 KPIs vía `icon()` en `ui.mjs`), todos `aria-hidden` y con la etiqueta de texto intacta; jerarquía refinada en `.kpi .label` (mayúsculas, tracking 0.05em) y botones de pestaña con icono+texto; micro-movimiento con significado (entrada de vista 160 ms y tres pulsos de atención en filas `.conflict`), siempre dentro de `@media (prefers-reduced-motion: no-preference)`. Corrección visual detectada en navegador: a 360 px las pestañas envolvían en 5 filas desalineadas → cuadrícula de 2 columnas con `align-items: stretch` → 3 filas parejas, sin scroll horizontal.

Reglas de oro de la fase (de la skill): nunca quitar el anillo de foco; objetivos táctiles ≥ 44×44 px; nunca confiar solo en el color; iconos SVG, no emoji; validar formularios junto al campo.

### F14 — Rediseño de MisFinanzas

- Nueva navegación lateral y barra inferior con historial, enlaces a secciones y acceso a cambios locales. Renovación del acceso, Resumen, registro de movimientos, Créditos, Planificación, Configuración, importación y conciliación.
- Tipografía Source Serif 4 e IBM Plex Sans e ilustración optimizada, servidas desde la propia aplicación e incluidas en la caché offline. Los importes del Resumen conservan la precisión entera hasta su presentación.
- Chromium local con datos sintéticos. Se verificaron cinco anchos entre 360 y 1440 px, modo oscuro, movimiento reducido y actualización de F13 a F14 con un movimiento pendiente que después se sincronizó. También se probaron el alta, la edición y el borrado, los rechazos y conflictos simulados, la importación XLSX con diferencias y la conciliación CSV.
- Con 10.000 movimientos, ambas versiones muestran 25 filas por página. En una corrida local, el CLS del acceso fue 0,000 en F13 y 0,014 en F14. El del Resumen fue 0,003 en ambas versiones.
- La instalación y el teclado en un Android físico quedan pendientes de un dispositivo conectado.
- El acceso pasó a ser una pantalla propia con una composición distinta en escritorio y móvil, formulario con estado de envío y error, y opción de mostrar la contraseña. El panel principal comparte la misma tipografía, paleta y controles, con una tabla de movimientos más fácil de recorrer.

### P3 — Seguridad (5,5–10 h, parcialmente condicional) — al final por decisión del usuario

> Estado real: la base ya está endurecida (CSP estricta, cookies `__Host-` HttpOnly, control de `Origin` en mutaciones, Argon2, rate-limit de login, sin secretos en el código). Nada de lo que sigue es urgente; son cierres de ciclo.

| # | Tarea | Estimación | Criterio de aceptación |
|---|---|---|---|
| P3.1 🔶 | Cifrar la copia externa de respaldos (gpg de archivo o `restic` a destino remoto). El checksum detecta alteraciones, pero no cifra ni autentica. | 2–4 h | Copia externa cifrada y un ensayo de descifrado + `restore-check.sh` documentado. **Parcial:** flujo `gpg` documentado en el README (cifrado, verificación y ensayo de restauración); falta el ensayo real en tu equipo externo. |
| P3.2 | Purga de tablas de idempotencia (`finance_requests`, `credit_requests`) que solo crecen y no tienen columna de fecha. **Condicional: solo cuando la base pase de ~100 MB.** | 2–3 h | Migración con `created_at` + purga piggyback en login, estilo sesiones. No hacerlo antes. |
| P3.3 ✅ | Auditoría periódica de dependencias (`cargo audit`, `cargo deny`) y actualización puntual de `Cargo.lock`. | 3 h reales | Informe limpio o CVEs con decisión escrita. **Completada:** 10 CVEs encontrados y corregidos subiendo calamine 0.38, rusqlite 0.43 (libsqlite3-sys 0.39+) y zip 9.8; `cargo audit` y `npm audit` en 0; único restante el aviso `chrono` sin mantener (transitorio, sin corrección disponible). Suite completa revalidada tras las subidas. |
| P3.4 | Revisión de acceso: quién puede administrar Docker en el NAS, rotación de la contraseña del administrador si sigue siendo la de bootstrap, y `CF_API_TOKEN` de Caddy con alcance mínimo ya documentado. | 0,5–1 h | Lista de acceso acotada y anotada; credenciales de bootstrap retiradas del entorno. |

---

## Resumen de cálculo

| Fase | Horas estimadas | Tipo |
|---|---|---|
| P0 Funcionalidad — código **✅ completada (F10.1)** | 8–12 | Corrección |
| P1 Funcionalidad — validación en producción | 7–14 | Operativa |
| P2 UX/UI premium **✅ completada (F11)** | 4 h reales | Producto |
| P3 Seguridad **🔶 parcial (P3.3 ✅, P3.1 documentado; F12.0 ✅)** | 3 h reales + ensayos | Cierre |
| **Total pendiente** | **≈ 30–57 h** | |

Orden recomendado de ejecución: P0 → P1 → P2 → P3. P0.1 y P0.2 son los únicos defectos que muestran información financiera errónea hoy y van primero dentro de P0. Ningún ítem requiere ramas nuevas: todo se trabaja en `main`, sin `git push` sin autorización.
