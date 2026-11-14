# MisFinanzas: diseño de la primera fase

## Objetivo y alcance

Crear una aplicación de finanzas personales en español, instalable como PWA en Android y usable en navegadores de escritorio. El servidor, escrito en Rust, se desplegará mediante Docker en un servidor o NAS de la red local. La primera fase parte de datos nuevos y prioriza operaciones financieras confiables, carga sencilla y sincronización entre dispositivos, también sin conexión.

Cada persona tiene sus propios datos financieros. Un administrador crea sus accesos. La moneda de esta fase es el guaraní paraguayo (PYG), con importes enteros; las fechas se presentan con referencia a America/Asuncion.

## Arquitectura y despliegue

El servidor Rust centraliza autenticación, reglas financieras, autorización, sincronización y persistencia en SQLite. La base reside en un volumen persistente de Docker. Una interfaz web ligera ofrece la aplicación en móvil y escritorio y conserva en el navegador una copia local de los datos necesarios y de las operaciones pendientes. La instalación y el arranque sin conexión se apoyan en los mecanismos de PWA del navegador.

La aplicación se sirve por HTTPS confiable en la red local, mediante un nombre del dominio propio del usuario. Los DNS del dominio están administrados en Cloudflare; se documentará una configuración que permita emitir y renovar el certificado mediante validación DNS y resolver el nombre al servidor local. Esta fase no publica la aplicación a Internet ni configura Cloudflare Tunnel. Se deberá verificar que los dispositivos elegidos confíen en el certificado y que el nombre resuelva en la red local.

## Usuarios y datos locales

El administrador crea las cuentas de acceso; no hay registro público. Cada consulta y modificación financiera se limita al usuario autenticado. Antes de trabajar sin conexión, la persona inicia sesión conectada en cada dispositivo y conserva una sesión local. Los dispositivos se consideran personales y protegidos por el bloqueo del sistema operativo. Los datos locales permiten el uso sin conexión, pero no constituyen un respaldo del servidor ni garantizan confidencialidad frente a quien controle el dispositivo.

Al cerrar sesión, la aplicación elimina la copia local de ese usuario. Si existen cambios pendientes, primero pide sincronizarlos o confirma expresamente que se descartarán; no los descarta silenciosamente. Los cambios no sincronizados pueden perderse si se pierde el dispositivo o se eliminan sus datos de navegador.

## Funcionalidad financiera

- Cada usuario administra, mientras está conectado, sus cuentas de efectivo, bancos, billeteras y tarjetas de crédito, sus saldos iniciales y categorías de ingresos y gastos. Los catálogos sincronizados se pueden consultar sin conexión.
- Los ingresos y gastos requieren importe, cuenta, categoría y fecha. La fecha se propone con el día actual; descripción y notas son opcionales. Los importes son positivos e íntegros en PYG y las categorías deben corresponder al tipo de movimiento.
- El saldo de una cuenta se obtiene de su saldo inicial y sus operaciones. Los ingresos lo aumentan y los gastos lo reducen. En una tarjeta, el saldo representa deuda: las compras la aumentan y los pagos la disminuyen.
- Un pago de tarjeta vincula la cuenta de origen y la tarjeta en una sola operación. Reduce el saldo de origen y la deuda de la tarjeta sin crear un gasto adicional. Crear, editar o eliminar esa operación mantiene juntos ambos efectos.
- Los usuarios pueden consultar, crear, editar y eliminar ingresos, gastos y pagos de tarjeta tanto conectados como sin conexión. La interfaz identifica saldos y movimientos con cambios aún pendientes de sincronización.

## Sincronización y errores

La aplicación mantiene los cambios pendientes en almacenamiento local y los sincroniza automáticamente cuando está abierta y recupera conectividad. También muestra el estado y permite reintentar. Cada operación dispone de un identificador estable para que un reintento no la aplique dos veces; una operación financiera vinculada se confirma de forma atómica en el servidor.

Las actualizaciones y eliminaciones identifican la versión del registro que se editó. Si otro dispositivo modificó o eliminó ese registro, la aplicación conserva la propuesta local y la versión del servidor, informa del conflicto y pide al usuario decidir. Resolverlo produce una nueva operación sincronizable; ninguna versión se sobrescribe silenciosamente. Hasta resolverlo se muestran claramente los datos en conflicto y su efecto pendiente, sin presentarlos como saldos definitivamente confirmados.

Las validaciones se ejecutan en el servidor y también se presentan oportunamente en la interfaz. Una solicitud inválida, un fallo de red o una sesión expirada no deben borrar los cambios pendientes: se comunica el motivo y la acción necesaria. La sincronización puede requerir iniciar sesión conectado nuevamente si la sesión del servidor ha vencido.

## Verificación y recuperación

Se verificarán los cálculos de saldos y tarjeta, la atomicidad de los pagos, el aislamiento entre usuarios y la autorización de administración. Las pruebas cubrirán operaciones desconectadas, reintentos, conflictos de edición y eliminación, conexión interrumpida, sesión vencida y sincronización en varios dispositivos. Se comprobarán la instalación y el uso sin red en Android, además de la navegación y sincronización en escritorio.

Se documentará y probará la creación de una copia coherente de la base de datos y su restauración en una instancia de prueba. También se comprobará que los datos persistan tras reiniciar los contenedores. El respaldo manual incluye los datos confirmados en el servidor; los cambios que sigan en dispositivos desconectados no forman parte de él.

## Fuera de alcance

Migración o importación desde Google Sheets; transferencias generales; facturación, conciliación y cuotas de tarjetas; préstamos; comprobantes; presupuestos; informes avanzados; acceso remoto por túnel; y respaldos automáticos. Estas funciones podrán diseñarse por separado una vez validado el núcleo.
