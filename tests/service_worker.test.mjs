import { test } from 'node:test';
import assert from 'node:assert/strict';
import { readFileSync } from 'node:fs';
import { runInNewContext } from 'node:vm';

// A deploy only updates installed PWAs if the Docker build rewrites the cache name
// the service worker declares. These assertions fail when that wiring drifts.
test('the docker build stamps the cache name the service worker declares', () => {
  const sw = readFileSync('static/sw.js', 'utf8');
  const declared = sw.match(/const CACHE = '(misfinanzas-shell-[a-z0-9-]+)'/);
  assert.ok(declared, 'static/sw.js must declare a misfinanzas-shell-* CACHE constant');
  const dockerfile = readFileSync('Dockerfile', 'utf8');
  assert.ok(
    dockerfile.includes(declared[1]),
    'Dockerfile must rewrite the CACHE constant declared in static/sw.js'
  );
  assert.match(dockerfile, /ARG FINAPP_VERSION/);
  assert.match(readFileSync('compose.yaml', 'utf8'), /FINAPP_VERSION/);
});

test('the service worker only serves the shell and cleans older cache generations', () => {
  const sw = readFileSync('static/sw.js', 'utf8');
  assert.match(sw, /caches\.keys\(\)/);
  assert.match(sw, /cache\.match\(url\.pathname\) \|\| fetch/);
  // The queue lives in IndexedDB, never in the shell cache; the update flow must not clear it.
  for (const forbidden of ['caches.delete("misfinanzas")', 'indexedDB.databases()']) {
    assert.ok(!sw.includes(forbidden), `static/sw.js must not contain ${forbidden}`);
  }
});

test('the offline shell installs the local fonts and login artwork', async () => {
  const handlers = new Map();
  const installed = new Set();
  runInNewContext(readFileSync('static/sw.js','utf8'), {
    self:{addEventListener:(type, handler) => handlers.set(type, handler)},
    Request:class { constructor(path) { this.path = path; } },
    caches:{open:async () => ({addAll:async requests => {
      for (const {path} of requests) {
        if (path !== '/') assert.ok(readFileSync('static' + path).length);
        installed.add(path);
      }
    }})},
  });
  let installation;
  handlers.get('install')({waitUntil:promise => { installation = promise; }});
  await installation;
  assert.ok(installed.has('/fonts/ibm-plex-sans.woff2'));
  assert.ok(installed.has('/fonts/source-serif-4.woff2'));
  assert.ok(installed.has('/login-art.webp'));
});
