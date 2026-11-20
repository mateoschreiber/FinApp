# Auditoría general de FinApp

**FinApp llegó a la entrega 6 de 6 y está en validación funcional en producción.** Al momento de la auditoría, la rama `main` de GitHub y la versión desplegada en `server1` coincidían en el commit `f411b88`. El contenedor estaba saludable y SQLite pasó las comprobaciones de integridad. Quedan correcciones y pruebas de aceptación antes de cerrar la funcionalidad y pasar al diseño visual.

## Estado por entrega

| Entrega | Resultado de la auditoría |
| --- | --- |
| 1. Primer uso y catálogos | Se probó en navegador la creación de la primera cuenta, las categorías sugeridas y el primer gasto. |
| 2. Modelo financiero y operaciones | Se verificó la migración de la base anterior y se comprobó con datos de prueba que una transferencia y un reintegro afectan las cuentas y el informe correctamente. |
| 3. PWA y sincronización | Se probó la recarga sin conexión, la conservación de la cola tras vencer la sesión y la sincronización con un segundo perfil. Se detectó un problema en las actualizaciones de la PWA. |
| 4. Tarjetas y préstamos | Las pruebas cubren cuotas, amortización francesa y alemana, pagos parciales y asignación de pagos de tarjeta. La gestión de correcciones y deuda previa sigue incompleta. |
| 5. Planificación e informes | Están disponibles presupuestos, recurrentes, objetivos, comprobantes y revisión mensual. Se detectó una inconsistencia al recalcular un mes revisado. |
| 6. Importación, conciliación y recuperación | XLSX, conciliación y exportación tienen pruebas con datos sintéticos. Se verificó el checksum del respaldo en el servidor y que el cron estaba activo. Al auditar figuraban **0 ejecuciones automáticas registradas**. |

## Qué falta, en orden de prioridad

1. **Corregir la revisión mensual.** Al editar un gasto de octubre, el patrimonio de noviembre bajó de **900 a 800 Gs.**, pero noviembre siguió marcado como revisado. Cualquier cambio que altere los saldos de un mes revisado debe devolverlo a pendiente. [Cálculo y revisión del informe](src/planning.rs#L341)
2. **Versionar la PWA en cada despliegue.** Se reprodujo en Edge que, si cambia `app.mjs` pero `sw.js` sigue igual, el navegador continúa mostrando el código anterior incluso al recargar. La actualización debe instalar los archivos nuevos y conservar la cola local. [Service worker](static/sw.js#L24)
3. **Completar la corrección de operaciones de crédito.** Faltan acciones para rectificar una compra financiada o un pago de préstamo registrado por error. También falta asignar un vencimiento a la deuda inicial de una tarjeta para que aparezca en la proyección. [Operaciones de crédito](src/credit.rs#L17) · [Proyección de tarjeta](static/credit.mjs#L53)
4. **Validar con datos y dispositivos reales.** Falta probar la instalación en un Android físico, importar el XLSX real en una instancia de ensayo y cuadrar saldos y totales con el archivo. También queda confirmar una ejecución programada del respaldo y una copia recuperable fuera de `server1`.
5. **Comprobar el rendimiento en móvil y automatizar las pruebas.** Con **10.003 movimientos** sintéticos, la carga inicial fue de **1,96 MB** y una sincronización sin cambios devolvió **141 bytes**. El servidor pagina el historial y la pantalla lo filtra en el dispositivo. Falta medir ese flujo en Android y ejecutar las suites automáticamente al actualizar `main`. [Historial en la interfaz](static/app.mjs#L284)

## Verificación

Pasaron las **62 pruebas automatizadas** de Rust y JavaScript. Los problemas de revisión mensual y actualización de la PWA se reprodujeron en comprobaciones adicionales. Esas dos correcciones tienen prioridad sobre el rediseño visual.
