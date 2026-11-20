# MisFinanzas

Aplicación de finanzas personales con servidor Rust, SQLite y una interfaz web pensada para dispositivos móviles y escritorio. El diseño y los límites de la primera fase están en [la especificación](docs/superpowers/specs/2026-11-14-misfinanzas-fase-1-design.md).

## Uso

1. Después de iniciar sesión, elige **Crear mi primera cuenta**. Agrega efectivo, banco, billetera o tarjeta. Al indicar una fecha de corte, carga el saldo al inicio de ese día. Los movimientos previos siguen visibles en el historial.
2. Crea una categoría o elige las sugeridas en **Configuración**. Registra ingresos, gastos, transferencias y pagos de tarjeta desde **Nuevo movimiento**. Para un reintegro, selecciona el gasto original y la cuenta que recibe el dinero.
3. Usa **Créditos** para compras en cuotas, resúmenes y préstamos. FinApp registra el gasto completo de una compra financiada en la fecha de compra. Consulta cuotas y vencimientos por separado.
4. En **Planificación**, selecciona un mes para consultar el resultado, el patrimonio y los vencimientos. Allí puedes gestionar presupuestos, recurrentes, objetivos y comprobantes, y marcar el mes como revisado.
5. En **Importar y exportar**, revisa y confirma un XLSX con las hojas `Cuentas`, `Categorias` y `Movimientos`. La vista previa muestra referencias y diferencias antes de escribir. También puedes conciliar extractos CSV y descargar cuentas, movimientos e informes.

La PWA conserva los movimientos pendientes en el navegador. Si un movimiento se rechaza o entra en conflicto, revisa **Cambios locales** en el resumen. Tras cambiar de dispositivo, sincroniza para descargar las operaciones registradas.

Para trabajar sin conexión, primero inicia sesión y sincroniza en cada dispositivo. La aplicación conserva los movimientos localmente y reintenta mientras está abierta; el botón «Sincronizar ahora» permite volver a intentarlo. El navegador debe admitir IndexedDB, service workers y Web Locks. Si deniega almacenamiento persistente, puede eliminar datos locales bajo presión de almacenamiento: sincroniza con frecuencia. Cerrar sesión requiere conexión y confirma antes de descartar cambios pendientes.

## Despliegue local con Docker

La aplicación corre en el NAS o servidor. Los dispositivos acceden por un proxy HTTPS instalado en el mismo equipo; Docker publica el puerto de la aplicación únicamente en `127.0.0.1:3000`. Para usar la PWA desde otro equipo, **configurar HTTPS y DNS local es un paso necesario**, no basta con abrir la IP del NAS.

### 1. Preparar el servidor

Necesitas acceso a una terminal del NAS, Git, Docker Engine con Compose, una dirección IP local estable, un dominio propio y un proxy HTTPS. El servidor necesita conexión saliente para descargar imágenes, dependencias y emitir el certificado. Los comandos siguientes son para Bash en Linux; adapta las rutas y permisos a tu NAS.

```bash
docker --version
docker compose version
git clone https://github.com/mateoschreiber/FinApp.git
cd FinApp
```

Si ya clonaste el repositorio, actualiza tu copia de `main` en lugar de volver a clonarlo. Comprueba primero que no tengas cambios locales que debas conservar.

### 2. Configurar y arrancar

Elige el nombre que usarás, por ejemplo `finanzas.tudominio.com`, y crea un archivo `.env` en la raíz del proyecto **sustituyendo ese nombre por el tuyo**:

```dotenv
FINAPP_ORIGIN=https://finanzas.tudominio.com
```

La dirección debe coincidir exactamente con la del navegador, incluyendo el puerto si no es el predeterminado y sin barra final. Compose usa este archivo cada vez que ejecutas sus comandos; el archivo está excluido de Git. Si la misma variable existe en tu terminal, tiene prioridad sobre `.env`.

Para probar solo en este equipo puedes usar `FINAPP_ORIGIN=http://localhost:3000`. El servidor solo acepta `http://` para `localhost`, `127.0.0.1` y `[::1]`; cualquier otra dirección HTTP (IP de la red local, IP pública o dominio) se rechaza al arrancar. El origen debe escribirse como lo envía el navegador: en minúsculas, sin ruta, sin credenciales, sin fragmento y sin puerto por defecto (`:80` o `:443`).

Con HTTP, la cookie de sesión lleva `Secure`. Según MDN, los navegadores solo la envían sobre HTTP en `localhost`, y Safari no admite esa excepción. Verificado en Chrome: login, cookie `HttpOnly` no legible desde JavaScript y `/api/me` autenticado. No verificado en Firefox. En Safari, usa HTTPS.

En la primera ejecución crea el administrador. El nombre es un ejemplo y la contraseña debe tener al menos 12 caracteres. Bash solicita la contraseña sin mostrarla en pantalla:

```bash
export FINAPP_ADMIN_USERNAME=admin
read -rsp 'Contraseña del administrador: ' FINAPP_ADMIN_PASSWORD; echo
export FINAPP_ADMIN_PASSWORD
export FINAPP_VERSION=$(git -C . rev-parse --short HEAD 2>/dev/null || echo local)
docker compose config --quiet
docker compose up -d --build
docker compose ps
docker compose logs --tail=50 app
```

Comprueba que el contenedor siga en ejecución y que no informe errores. La primera construcción puede tardar porque compila Rust. La contraseña y el usuario solo se utilizan para crear el primer administrador en una base vacía; cambiarlos después no cambia la cuenta ya creada. Evita compartir la salida de los comandos si contiene información privada.

Una vez creado el administrador, retira las credenciales del entorno y recrea el contenedor sin ellas. Conserva `FINAPP_ORIGIN` en `.env`:

```bash
unset FINAPP_ADMIN_USERNAME FINAPP_ADMIN_PASSWORD
docker compose up -d --force-recreate
docker compose ps
```

Las variables de entorno del contenedor son visibles para quienes pueden administrarlo mediante Docker; restringe ese acceso. La base se guarda en el volumen de Compose `finapp_data`, no dentro de la imagen. **No ejecutes `docker compose down -v` si quieres conservar los datos.**

### 3. Configurar HTTPS y entrar

Configura el proxy y el DNS local como se explica a continuación. Después abre tu URL HTTPS desde un dispositivo conectado a la misma red e inicia sesión con el administrador. En **Configuración**, crea cuentas y categorías antes de registrar movimientos. Para instalar la PWA en Android, usa la opción de instalación del navegador cuando esté disponible; primero debes acceder por HTTPS confiable.

Si la página no abre, comprueba `docker compose ps`, los registros con `docker compose logs --tail=100 app`, que el proxy pueda conectar a `127.0.0.1:3000`, que el nombre resuelva a la IP local correcta y que la URL coincida con `FINAPP_ORIGIN`. El servidor expone `GET /health` para comprobaciones locales. Los cambios aún no sincronizados permanecen solo en el dispositivo y no se incluyen en el respaldo del servidor.

El inicio de sesión admite como máximo 20 intentos cada 60 segundos, sumando los de todos los usuarios, existentes o no. Si se superan, durante ese minuto nadie puede entrar, incluido el administrador. Es un límite deliberado: no hay límite por IP porque el proxy local oculta la dirección real del cliente.

### HTTPS con un dominio de Cloudflare

Usa un proxy que corra en el **mismo host** y pueda acceder a `127.0.0.1:3000`. Por ejemplo, instala Caddy con el [módulo DNS de Cloudflare](https://github.com/caddy-dns/cloudflare), que debe estar compilado en el binario: el Caddy estándar no lo incluye. Crea en Cloudflare un token limitado a la zona con permisos `Zone.Zone:Read` y `Zone.DNS:Edit` y entrégalo al proceso Caddy como variable `CF_API_TOKEN`, sin incluirlo en Git ni en el Caddyfile.

Ejemplo de Caddyfile; cambia `finanzas.ejemplo.com` por tu nombre real:

```caddyfile
finanzas.ejemplo.com {
    tls {
        dns cloudflare {env.CF_API_TOKEN}
        resolvers 1.1.1.1
    }
    reverse_proxy 127.0.0.1:3000
}
```

Configura el DNS local o el router para resolver ese nombre a la dirección privada del servidor y verifica que **todos los dispositivos** usan ese DNS. Caddy necesita acceso saliente a Cloudflare y a la autoridad certificadora para crear el registro TXT temporal y renovar el certificado; la validación DNS no requiere puertos entrantes desde Internet. Conserva la configuración y el almacenamiento de certificados de Caddy entre reinicios. Limita el puerto 443 del proxy a la interfaz y a la red local, sin NAT ni reenvío desde el router. Confirma el alcance efectivo de las reglas de red del NAS; el enlace de puertos de Docker puede tener interacciones particulares con el cortafuegos.

Si el proxy corre en otro contenedor, `127.0.0.1` de ese contenedor no apunta al servidor de la aplicación: utiliza una red Docker privada compartida y el nombre del servicio, y no publiques el puerto de la aplicación. Conserva la misma dirección HTTPS para evitar separar los datos locales de la PWA por origen.

## Respaldo y ensayo de restauración

Los comprobantes se guardan dentro de SQLite y forman parte de la copia. Solo se incluyen datos ya sincronizados con el servidor; los cambios pendientes siguen en los dispositivos. Protege los respaldos como datos financieros privados y copia ambos archivos generados a otro equipo o almacenamiento. El checksum detecta alteraciones accidentales, pero no cifra ni autentica la copia.

Con el servicio `app` iniciado, en el servidor ejecuta:

```bash
bash scripts/backup.sh "$HOME/finapp-backups"
```

El script usa `.backup` de SQLite para crear una instantánea coherente incluso con WAL, comprueba `integrity_check` y compara SHA-256 antes y después de copiarla. Publica un archivo `.sqlite` y otro `.sqlite.sha256` con el mismo nombre y muestra su ruta. Una ejecución fallida devuelve un código distinto de cero; revisa el mensaje y corrígelo. No copies solamente el archivo activo de SQLite y no uses una copia sin su correspondiente checksum.

Si FinApp está definido en otro proyecto Compose, indica su directorio, archivos y servicio. En la instalación de `server1` el comando es:

```bash
FINAPP_COMPOSE_DIR=/srv/media/homelab \
COMPOSE_FILE=/srv/media/homelab/compose.yaml:/srv/media/homelab/finapp-cloudflare.override.yaml \
FINAPP_SERVICE=finapp bash scripts/backup.sh "$HOME/respaldos-finapp"
```

Para ensayar la restauración de un archivo específico, con la imagen `finapp:local` ya construida:

```bash
bash scripts/restore-check.sh "$HOME/finapp-backups/EL_ARCHIVO.sqlite"
```

El script comprueba el checksum, restaura en un volumen Docker nuevo, verifica `integrity_check` e inicia un contenedor **sin red ni puertos publicados** para consultar `/health`. El volumen y el contenedor temporales se eliminan al terminar; no se modifica el volumen de la aplicación. El ensayo verifica el arranque y la integridad estructural, pero no sustituye revisar manualmente que los movimientos y comprobantes esperados sean los correctos. Para restaurar producción, detén el servicio y conserva una copia previa, incluidos los archivos WAL/SHM; trabaja sobre un volumen nuevo y verifica los datos antes de ponerlo en uso.

Para ensayar con una imagen candidata, define `FINAPP_RESTORE_IMAGE` con su etiqueta.

### Actualizar FinApp en server1

Después de verificar un respaldo de la base actual y ensayar la imagen candidata en una instancia aislada, actualiza el código local y reconstruye solo FinApp:

```bash
git -C ~/FinApp pull --ff-only
export FINAPP_VERSION=$(git -C ~/FinApp rev-parse --short HEAD)
cd /srv/media/homelab
docker compose -f compose.yaml -f finapp-cloudflare.override.yaml up -d --build --no-deps finapp
curl --fail --silent http://127.0.0.1:3000/health
```

El comando reconstruye desde `~/FinApp` y conserva el volumen `homelab_finapp_data`. `FINAPP_VERSION` estampa el commit dentro del service worker: sin ese paso, los navegadores con la aplicación instalada conservan la versión anterior de la interfaz aunque el servidor ya tenga la nueva. Los dispositivos detectan el cambio al abrir la aplicación y muestran «Aplicar actualización»; la cola de cambios pendientes en IndexedDB se conserva. Prueba el dominio con tu sesión de Cloudflare Access y comprueba los saldos y la sincronización. Si debes volver a una versión anterior a una migración, ensaya esa imagen con el respaldo anterior y monta el volumen restaurado tras verificarlo.

### Programar una copia diaria en Ubuntu

Estos ejemplos usan systemd en el servidor. Sustituye `/srv/FinApp`, `/srv/finapp-backups` y `USUARIO_DOCKER` por rutas absolutas y un usuario autorizado a ejecutar Docker. El acceso a Docker equivale a privilegios elevados; limita quién puede usarlo. El directorio de destino debe estar protegido y tener espacio suficiente. El servicio necesita acceso al `.env` del proyecto, incluido `FINAPP_ORIGIN`; no incluyas contraseñas en los archivos de unidad.

`/etc/systemd/system/finapp-backup.service`:

```ini
[Unit]
Description=Respaldo SQLite de MisFinanzas
After=docker.service
Requires=docker.service

[Service]
Type=oneshot
User=USUARIO_DOCKER
WorkingDirectory=/srv/FinApp
ExecStart=/usr/bin/bash /srv/FinApp/scripts/backup.sh /srv/finapp-backups
```

`/etc/systemd/system/finapp-backup.timer`:

```ini
[Unit]
Description=Respaldo diario de MisFinanzas

[Timer]
OnCalendar=daily
Persistent=true

[Install]
WantedBy=timers.target
```

Tras crear las unidades, ejecuta `sudo systemctl daemon-reload` y `sudo systemctl enable --now finapp-backup.timer`. Comprueba `systemctl list-timers finapp-backup.timer`, ejecuta una prueba con `sudo systemctl start finapp-backup.service` y revisa `journalctl -u finapp-backup.service -n 50`. `Persistent=true` recupera una ejecución perdida cuando se activa el temporizador. El script no elimina copias antiguas: establece una retención y supervisa el espacio disponible. Sin supervisión y sin copias fuera del servidor, una programación no garantiza recuperación.

Fuentes: [respaldo SQLite](https://www.sqlite.org/backup.html), [desafío DNS de Caddy](https://caddyserver.com/docs/automatic-https#dns-challenge) y [publicación de puertos Docker](https://docs.docker.com/reference/compose-file/services/#ports).
