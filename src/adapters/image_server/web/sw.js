// Keeps the shell of the page (the page itself, its manifest and its icons) so that it opens with the network down.
//
// Nothing private is kept here: the pictures and the state are the viewer's, and the page keeps its own last picture (in
// IndexedDB) and forgets it when it signs out. The API is never answered from this cache, so a revoked token is refused as soon
// as the network is back.
'use strict';

// Named for what it keeps (the server puts a fingerprint of the page, manifest, icons and this script where the name ends), so
// that when any of them changes the worker is a new one, its cache is a new one, and the old is deleted when it takes over.
const CACHE = 'hd-shell-__SHELL__';
const SHELL = [
  '/view',
  '/view/manifest.webmanifest',
  '/view/icon.svg',
  '/view/icons/180.png',
  '/view/icons/192.png',
  '/view/icons/512.png',
];

self.addEventListener('install', (event) => {
  event.waitUntil(
    caches.open(CACHE).then((cache) => cache.addAll(SHELL)).then(() => self.skipWaiting())
  );
});

self.addEventListener('activate', (event) => {
  event.waitUntil(
    caches.keys()
      .then((names) => Promise.all(names.filter((name) => name !== CACHE).map((name) => caches.delete(name))))
      .then(() => self.clients.claim())
  );
});

self.addEventListener('fetch', (event) => {
  const request = event.request;
  const url = new URL(request.url);
  if (request.method !== 'GET' || url.origin !== self.location.origin) return;

  // The page, whatever is after its address (`?kiosk`, `?layout=`): from the network when there is one, so that a new version is
  // used at once, and the last one seen when there is not.
  if (url.pathname === '/view') {
    event.respondWith(
      fetch(request)
        .then((response) => {
          if (response.ok) {
            const copy = response.clone();
            caches.open(CACHE).then((cache) => cache.put('/view', copy));
          }
          return response;
        })
        .catch(() => caches.match('/view'))
    );
    return;
  }

  // The rest of the shell does not change: from the cache, and the network for what is not there.
  if (SHELL.includes(url.pathname)) {
    event.respondWith(caches.match(url.pathname).then((hit) => hit || fetch(request)));
  }
  // Everything else (the API) is left to the network.
});
