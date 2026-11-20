#!/usr/bin/env bash
set -euo pipefail

if (( $# != 1 )); then echo 'Uso: bash scripts/restore-check.sh respaldo.sqlite' >&2; exit 2; fi
backup=$1
image=${FINAPP_RESTORE_IMAGE:-finapp:local}
[[ "$image" =~ ^[a-zA-Z0-9][a-zA-Z0-9_./:@-]*$ ]] || { echo 'Nombre de imagen inválido.' >&2; exit 2; }
[[ -f "$backup" && -f "$backup.sha256" ]] || { echo 'Falta el respaldo o su archivo .sha256.' >&2; exit 1; }
read -r expected recorded < "$backup.sha256"
[[ "$expected" =~ ^[0-9a-f]{64}$ && "$recorded" == "$(basename -- "$backup")" ]] || { echo 'Archivo de checksum inválido.' >&2; exit 1; }
actual=$(sha256sum "$backup" | awk '{print $1}')
[[ "$expected" == "$actual" ]] || { echo 'El checksum del respaldo no coincide.' >&2; exit 1; }
volume=''
container=''
cleanup() {
  if [[ "$container" =~ ^[0-9a-f]{64}$ ]]; then docker rm -f "$container" >/dev/null 2>&1 || echo 'No se pudo eliminar el contenedor temporal.' >&2; fi
  if [[ "$volume" =~ ^[0-9a-f]{64}$ ]]; then docker volume rm "$volume" >/dev/null 2>&1 || echo 'No se pudo eliminar el volumen temporal.' >&2; fi
}
trap cleanup EXIT
volume=$(docker volume create)
[[ "$volume" =~ ^[0-9a-f]{64}$ ]] || { echo 'Docker devolvió un volumen temporal inesperado.' >&2; volume=''; exit 1; }
docker run --rm --network none -v "$volume:/data" --user 0 --entrypoint sh -i "$image" -c 'cat > /data/finapp.sqlite && chown 10001:10001 /data/finapp.sqlite' < "$backup"
check=$(docker run --rm --network none -v "$volume:/data" --entrypoint sqlite3 "$image" /data/finapp.sqlite 'PRAGMA integrity_check;')
[[ "$check" == ok ]] || { echo 'La restauración no superó integrity_check.' >&2; exit 1; }
container=$(docker run -d --network none -v "$volume:/data" -e FINAPP_ORIGIN=https://restore.invalid "$image")
[[ "$container" =~ ^[0-9a-f]{64}$ ]] || { echo 'Docker devolvió un contenedor temporal inesperado.' >&2; container=''; exit 1; }
healthy=0
for (( attempt=0; attempt<30; attempt++ )); do
  if docker exec "$container" curl --fail --silent --max-time 2 http://127.0.0.1:3000/health >/dev/null; then healthy=1; break; fi
  sleep 1
done
[[ "$healthy" == 1 ]] || { echo 'La aplicación restaurada no respondió a /health.' >&2; exit 1; }
check=$(docker exec "$container" sqlite3 /data/finapp.sqlite 'PRAGMA integrity_check; PRAGMA foreign_key_check;')
[[ "$check" == ok ]] || { echo 'El esquema migrado no superó los controles de integridad.' >&2; exit 1; }
printf 'Restauración aislada verificada con la imagen %s. SQLite íntegro y /health operativo.\n' "$image"
