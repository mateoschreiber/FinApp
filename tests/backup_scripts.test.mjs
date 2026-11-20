import { test } from 'node:test';
import assert from 'node:assert/strict';
import { mkdtempSync, mkdirSync, writeFileSync, readFileSync, readdirSync, rmSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { join, resolve } from 'node:path';
import { spawnSync } from 'node:child_process';

const bash = process.platform === 'win32' ? 'C:/Program Files/Git/bin/bash.exe' : 'bash';
const script = resolve('scripts/backup.sh').replaceAll('\\', '/');
const restore = resolve('scripts/restore-check.sh').replaceAll('\\', '/');

function fixture(failCopy = false) {
  const base = mkdtempSync(join(tmpdir(), 'finapp-test-'));
  const bin = join(base, 'bin');
  const output = join(base, 'backups');
  mkdirSync(bin);
  const fake = `#!/usr/bin/env bash
set -eu
case "$*" in
  'compose exec -T app test -f /data/finapp.sqlite') exit 0 ;;
  'compose exec -T app mktemp /data/.finapp-backup.XXXXXXXX.sqlite') printf '/data/.finapp-backup.abc12345.sqlite\\n' ;;
  'compose exec -T app sqlite3 /data/finapp.sqlite '*) exit 0 ;;
  'compose exec -T app sqlite3 /data/.finapp-backup.abc12345.sqlite PRAGMA integrity_check;') printf 'ok\\n' ;;
  'compose exec -T app sha256sum /data/.finapp-backup.abc12345.sqlite') printf '%s  /data/.finapp-backup.abc12345.sqlite\\n' "$FAKE_HASH" ;;
  'compose cp app:/data/.finapp-backup.abc12345.sqlite '*) [ "$FAKE_FAIL_COPY" != 1 ] || exit 42; printf 'snapshot' > "\${4}" ;;
  'compose exec -T app rm -f -- /data/.finapp-backup.abc12345.sqlite') exit 0 ;;
  *) printf 'Unexpected docker call: %s\\n' "$*" >&2; exit 1 ;;
esac
`;
  writeFileSync(join(bin, 'docker'), fake, { mode: 0o755 });
  const env = { ...process.env, PATH: `${bin};${process.env.PATH}`, MSYS_NO_PATHCONV: '1',
    FAKE_HASH: '16a0eeb0791b6c92451fd284dd9f599e0a7dbe7f6ebea6e2d2d06c7f74aec112',
    FAKE_FAIL_COPY: failCopy ? '1' : '0' };
  // Git Bash uses POSIX path separators in PATH, including when invoked from Node on Windows.
  env.PATH = `${bin.replaceAll('\\', '/')}:${process.env.PATH.replaceAll('\\', '/').replaceAll(';', ':')}`;
  return { base, output, env };
}

test('a failed copy does not publish a backup or checksum', () => {
  const f = fixture(true);
  try {
    const run = spawnSync(bash, [script, f.output.replaceAll('\\', '/')], { env: f.env, encoding: 'utf8' });
    assert.notEqual(run.status, 0);
    assert.deepEqual(readdirSync(f.output), []);
  } finally { rmSync(f.base, { recursive: true, force: true }); }
});

test('a successful copy publishes its checksum and removes temporary files', () => {
  const f = fixture();
  try {
    const run = spawnSync(bash, [script, f.output.replaceAll('\\', '/')], { env: f.env, encoding: 'utf8' });
    assert.equal(run.status, 0, run.stderr);
    const files = readdirSync(f.output).sort();
    assert.equal(files.length, 2);
    assert.match(files[0], /^finapp-.*\.sqlite$/);
    assert.equal(readFileSync(join(f.output, files[0]), 'utf8'), 'snapshot');
    assert.match(readFileSync(join(f.output, files[1]), 'utf8'), new RegExp(`${files[0].replaceAll('.', '\\.')}$`, 'm'));
  } finally { rmSync(f.base, { recursive: true, force: true }); }
});

test('restore refuses a modified backup before invoking Docker', () => {
  const f = fixture();
  try {
    const path = join(f.base, 'database.sqlite');
    writeFileSync(path, 'changed');
    writeFileSync(`${path}.sha256`, `${'0'.repeat(64)}  database.sqlite\n`);
    const run = spawnSync(bash, [restore, path.replaceAll('\\', '/')], { env: f.env, encoding: 'utf8' });
    assert.notEqual(run.status, 0);
    assert.match(run.stderr, /checksum del respaldo no coincide/);
    assert.doesNotMatch(run.stderr, /Unexpected docker call/);
  } finally { rmSync(f.base, { recursive: true, force: true }); }
});
