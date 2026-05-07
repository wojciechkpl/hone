// Service worker — purely a cache layer over the static assets honed
// serves at /ui/. Network-first for everything API-shaped (anything
// outside /ui/ + the /ui/ index itself); cache-first for /ui/assets/*
// since those are content-hashed and immutable.
//
// Versioned via the build hash baked into the filename — when assets
// change the hashes change and old entries become orphans cleaned up
// by activate.

const CACHE_VERSION = "hone-webui-v1";
const APP_SHELL = ["/ui/", "/ui/index.html", "/ui/manifest.json", "/ui/favicon.svg"];

self.addEventListener("install", (event) => {
  event.waitUntil(
    caches
      .open(CACHE_VERSION)
      .then((cache) => cache.addAll(APP_SHELL).catch(() => {})),
  );
  // Activate immediately — replaces an old SW without needing a tab close.
  self.skipWaiting();
});

self.addEventListener("activate", (event) => {
  event.waitUntil(
    caches
      .keys()
      .then((keys) =>
        Promise.all(
          keys
            .filter((k) => k !== CACHE_VERSION)
            .map((k) => caches.delete(k)),
        ),
      )
      .then(() => self.clients.claim()),
  );
});

self.addEventListener("fetch", (event) => {
  const req = event.request;
  const url = new URL(req.url);

  // Only intercept GETs on the same origin. SSE / POST / cross-origin
  // pass through untouched.
  if (req.method !== "GET" || url.origin !== self.location.origin) {
    return;
  }

  // Hashed assets: cache-first, immutable.
  if (url.pathname.startsWith("/ui/assets/")) {
    event.respondWith(cacheFirst(req));
    return;
  }

  // Any other /ui/ request: stale-while-revalidate. Lets the SPA shell
  // load instantly from cache while we kick off a refresh in the
  // background.
  if (url.pathname.startsWith("/ui/")) {
    event.respondWith(staleWhileRevalidate(req));
    return;
  }

  // Everything else (API routes like /run, /sessions, /files, etc.):
  // pure network. We never want stale agent responses.
});

async function cacheFirst(req) {
  const cache = await caches.open(CACHE_VERSION);
  const hit = await cache.match(req);
  if (hit) return hit;
  const resp = await fetch(req);
  if (resp.ok) cache.put(req, resp.clone());
  return resp;
}

async function staleWhileRevalidate(req) {
  const cache = await caches.open(CACHE_VERSION);
  const hit = await cache.match(req);
  const network = fetch(req)
    .then((resp) => {
      if (resp.ok) cache.put(req, resp.clone());
      return resp;
    })
    .catch(() => null);
  return hit ?? (await network) ?? Response.error();
}
