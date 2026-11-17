# MisFinanzas

Aplicación de finanzas personales con servidor Rust, SQLite y una interfaz web pensada para dispositivos móviles y escritorio. El diseño y los límites de la primera fase están en [la especificación](docs/superpowers/specs/2026-11-14-misfinanzas-fase-1-design.md).

**Estado de validación:** se probaron las reglas y la sincronización con datos temporales, incluido un recorrido de navegador con el servidor desconectado. La imagen Docker, el HTTPS del NAS, la restauración de un respaldo y la instalación en Android aún deben verificarse en el entorno de destino. No uses datos financieros reales hasta completar esas comprobaciones.

Para trabajar sin conexión, primero inicia sesión y sincroniza en cada dispositivo. La aplicación conserva los movimientos localmente y reintenta mientras está abierta; el botón «Sincronizar ahora» permite volver a intentarlo. El navegador debe admitir IndexedDB, service workers y Web Locks. Si deniega almacenamiento persistente, puede eliminar datos locales bajo presión de almacenamiento: sincroniza con frecuencia. Cerrar sesión requiere conexión y confirma antes de descartar cambios pendientes.

## Despliegue local con Docker

Requisitos: Docker Engine con Compose, un servidor o NAS que permanezca encendido, una dirección de red local estable, un nombre de dominio propio y un proxy HTTPS en el servidor. El acceso desde otros dispositivos y la instalación de la PWA requieren HTTPS con un certificado confiable. No se necesita publicar puertos a Internet.

1. En el servidor, obtén el código y sitúate en esta carpeta. Define `FINAPP_ORIGIN` como la URL HTTPS exacta que usarán los dispositivos, por ejemplo `https://finanzas.ejemplo.com`, sin barra final ni comodines. Su valor debe permanecer definido en cada invocación de Compose. Define también `FINAPP_ADMIN_USERNAME` y `FINAPP_ADMIN_PASSWORD` como variables de entorno para el primer arranque. Elige una contraseña larga y única; evita incluirla en comandos que queden en el historial de la terminal. Estas últimas variables solo crean el primer administrador y no cambian credenciales existentes. Mantén las credenciales fuera del repositorio y restringe el acceso al entorno de ejecución de Docker.
2. Ejecuta `docker compose up -d --build`. La base se guarda en el volumen `finapp_data` del proyecto Compose; no uses `docker compose down -v` si quieres conservarla.
3. Cuando el primer administrador esté creado, elimina ambas variables del entorno y recrea el contenedor con `docker compose up -d --force-recreate`. Restringe el acceso al daemon Docker: los usuarios que pueden inspeccionarlo pueden ver variables de entorno del contenedor.
4. Configura el proxy HTTPS como se indica abajo. Abre el dominio mediante HTTPS desde cada dispositivo de la red. El puerto interno de la aplicación solo se publica en `127.0.0.1` del servidor; no es accesible directamente desde otros equipos.

El contenedor ejecuta la aplicación como usuario sin privilegios, guarda SQLite en `/data/finapp.sqlite` y verifica `GET /health`. Consulta `docker compose ps` y `docker compose logs app` para diagnosticar el arranque, sin compartir datos privados ni credenciales.

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

## Respaldo manual y restauración

La copia solo contiene datos que ya llegaron al servidor; los cambios sin sincronizar siguen en sus dispositivos. Protege los respaldos como datos financieros privados y guárdalos en otro equipo o almacenamiento. La imagen incluye el programa `sqlite3`; su comando `.backup` usa la API de respaldo de SQLite y genera una instantánea consistente incluso si la aplicación está activa. No copies únicamente el archivo de base mientras SQLite esté abierto: en modo WAL podrían faltar transacciones.

Desde la carpeta del proyecto, con el contenedor en ejecución, crea primero una instantánea dentro del volumen y cópiala al servidor (el nombre indicado no debe existir previamente):

```sh
docker compose exec -T app sqlite3 /data/finapp.sqlite '.backup /data/respaldo-manual.sqlite'
docker compose exec -T app sqlite3 /data/respaldo-manual.sqlite 'PRAGMA integrity_check;'
docker compose cp app:/data/respaldo-manual.sqlite ./respaldo-manual.sqlite
```

Comprueba que el resultado de `integrity_check` sea exactamente `ok` y conserva la copia fuera del volumen de la aplicación. No reutilices el nombre de la instantánea para evitar sobrescribir una copia previa. Repite la verificación de integridad en el archivo copiado si tienes `sqlite3` instalado en el host.

Para ensayar la recuperación **sin tocar la base principal**, usa un proyecto Compose con nombre distinto y un volumen nuevo. El siguiente ejemplo asume que `respaldo-manual.sqlite` está en el directorio actual:

```sh
docker compose -p finapp-prueba create app
docker compose -p finapp-prueba cp ./respaldo-manual.sqlite app:/data/finapp.sqlite
docker compose -p finapp-prueba run --rm --no-deps --user 0 --entrypoint sh app -c 'chown 10001:10001 /data/finapp.sqlite'
docker compose -p finapp-prueba run --rm --no-deps --entrypoint sqlite3 app /data/finapp.sqlite 'PRAGMA integrity_check;'
```

El resultado debe ser `ok`. El proyecto de prueba debe usar un volumen nuevo sin archivos `-wal` o `-shm` de una ejecución anterior; no reutilices un volumen existente para esta prueba. Para comprobar además el inicio de la aplicación, inicia la instancia de prueba con otro puerto local y otro dominio o con un proxy temporal aislado, e inicia sesión y revisa los datos. No uses las instrucciones de prueba para reemplazar una base de producción: para una restauración real, detén la aplicación, conserva antes la base y sus archivos WAL/SHM, y restaura en un volumen nuevo con la aplicación detenida.

Fuentes: [respaldo SQLite](https://www.sqlite.org/backup.html), [desafío DNS de Caddy](https://caddyserver.com/docs/automatic-https#dns-challenge) y [publicación de puertos Docker](https://docs.docker.com/reference/compose-file/services/#ports).
