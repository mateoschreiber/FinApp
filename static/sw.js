const CACHE = 'misfinanzas-shell-v7';
const SHELL = [
  '/', '/index.html', '/style.css', '/app.mjs', '/view-model.mjs',
  '/offline-store.mjs', '/sync.mjs', '/ui.mjs', '/extras.mjs', '/credit.mjs',
  '/planning.mjs', '/reconciliation.mjs', '/manifest.webmanifest', '/icon-192.png', '/icon-512.png',
  '/fonts/ibm-plex-sans.woff2', '/fonts/source-serif-4.woff2', '/login-art.webp',
];

self.addEventListener('install', event => {
  event.waitUntil(caches.open(CACHE).then(cache =>
    cache.addAll(SHELL.map(path => new Request(path, {cache:'reload'})))));
});
self.addEventListener('message', event => {
  if (event.data === 'ACTIVATE_UPDATE') event.waitUntil(self.skipWaiting());
});
self.addEventListener('activate', event => {
  event.waitUntil((async () => {
    const keys = await caches.keys();
    await Promise.all(keys.filter(key => key.startsWith('misfinanzas-shell-') && key !== CACHE)
      .map(key => caches.delete(key)));
    await self.clients.claim();
  })());
});
self.addEventListener('fetch', event => {
  const url = new URL(event.request.url);
  if (event.request.method !== 'GET' || url.origin !== self.location.origin || !SHELL.includes(url.pathname)) return;
  event.respondWith((async () => {
    const cache = await caches.open(CACHE);
    return await cache.match(url.pathname) || fetch(event.request);
  })());
});
