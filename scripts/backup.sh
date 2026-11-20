#!/usr/bin/env bash
set -euo pipefail

if (( $# > 1 )); then echo 'Uso: bash scripts/backup.sh [directorio]' >&2; exit 2; fi
project_dir=$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")/.." && pwd -P)
cd -- "${FINAPP_COMPOSE_DIR:-$project_dir}"
service=${FINAPP_SERVICE:-app}
[[ "$service" =~ ^[a-zA-Z0-9][a-zA-Z0-9_-]*$ ]] || { echo 'Nombre de servicio inválido.' >&2; exit 2; }
umask 077
destination=${1:-"$HOME/finapp-backups"}
mkdir -p -- "$destination"
destination=$(cd -- "$destination" && pwd -P)
work=$(mktemp -d "$destination/.finapp-backup.XXXXXXXX")
remote=''
cleanup() {
  if [[ "$remote" =~ ^/data/\.finapp-backup\.[[:alnum:]]{8}\.sqlite$ ]]; then
    docker compose exec -T "$service" rm -f -- "$remote" >/dev/null 2>&1 || echo 'No se pudo eliminar la instantánea temporal del contenedor.' >&2
  fi
  rm -rf -- "$work"
}
trap cleanup EXIT

docker compose exec -T "$service" test -f /data/finapp.sqlite
remote=$(docker compose exec -T "$service" mktemp /data/.finapp-backup.XXXXXXXX.sqlite | tr -d '\r')
if [[ ! "$remote" =~ ^/data/\.finapp-backup\.[[:alnum:]]{8}\.sqlite$ ]]; then echo 'Ruta temporal inesperada.' >&2; exit 1; fi
docker compose exec -T "$service" sqlite3 /data/finapp.sqlite ".backup $remote"
check=$(docker compose exec -T "$service" sqlite3 "$remote" 'PRAGMA integrity_check;' | tr -d '\r')
if [[ "$check" != ok ]]; then echo 'La instantánea no superó integrity_check.' >&2; exit 1; fi
remote_hash=$(docker compose exec -T "$service" sha256sum "$remote" | awk '{print $1}')
[[ "$remote_hash" =~ ^[0-9a-f]{64}$ ]] || { echo 'Checksum remoto inválido.' >&2; exit 1; }
docker compose cp "$service:$remote" "$work/backup.sqlite"
local_hash=$(sha256sum "$work/backup.sqlite" | awk '{print $1}')
[[ "$remote_hash" == "$local_hash" ]] || { echo 'La copia no coincide con la instantánea.' >&2; exit 1; }
name="finapp-$(date -u +%Y%m%dT%H%M%SZ)-${remote:21:8}.sqlite"
printf '%s  %s\n' "$local_hash" "$name" > "$work/backup.sha256"
mv -- "$work/backup.sqlite" "$destination/$name"
mv -- "$work/backup.sha256" "$destination/$name.sha256"
printf 'Respaldo verificado: %s/%s\n' "$destination" "$name"
