// Tiny IndexedDB wrapper.
//
// One object store per concern (sessions, messages). Promise-based,
// no external dep. Keeps the bundle small and the surface minimal —
// real apps that need cursors/migrations/etc. should swap in `idb`,
// but Phase 1B only needs `get`/`put`/`getAll`/`delete` per session.

const DB_NAME = "hone-webui";
const DB_VERSION = 1;
const STORE_SESSIONS = "sessions"; // key: session_id, value: SessionMeta
const STORE_MESSAGES = "messages"; // key: session_id, value: PersistedMessage[]

export type SessionMeta = {
  id: string;
  created_at: number; // ms since epoch
  last_used_at: number;
  title?: string; // first user prompt, truncated
};

export type PersistedMessage =
  | { kind: "user"; text: string; ts: number }
  | { kind: "assistant"; text: string; ts: number }
  | {
      kind: "tool";
      name: string;
      status: "running" | "done" | "error";
      result?: string;
      ts: number;
    }
  | { kind: "error"; message: string; ts: number };

let dbPromise: Promise<IDBDatabase> | null = null;

function openDb(): Promise<IDBDatabase> {
  if (dbPromise) return dbPromise;
  dbPromise = new Promise((resolve, reject) => {
    const req = indexedDB.open(DB_NAME, DB_VERSION);
    req.onupgradeneeded = () => {
      const db = req.result;
      if (!db.objectStoreNames.contains(STORE_SESSIONS)) {
        db.createObjectStore(STORE_SESSIONS, { keyPath: "id" });
      }
      if (!db.objectStoreNames.contains(STORE_MESSAGES)) {
        // No keyPath — we use the session_id as the explicit key.
        db.createObjectStore(STORE_MESSAGES);
      }
    };
    req.onsuccess = () => resolve(req.result);
    req.onerror = () => reject(req.error);
  });
  return dbPromise;
}

function tx<T>(
  store: string,
  mode: IDBTransactionMode,
  run: (s: IDBObjectStore) => IDBRequest<T>,
): Promise<T> {
  return openDb().then(
    (db) =>
      new Promise<T>((resolve, reject) => {
        const t = db.transaction(store, mode);
        const s = t.objectStore(store);
        const req = run(s);
        req.onsuccess = () => resolve(req.result);
        req.onerror = () => reject(req.error);
      }),
  );
}

// ---- Sessions ----

export async function listSessions(): Promise<SessionMeta[]> {
  try {
    return await tx<SessionMeta[]>(STORE_SESSIONS, "readonly", (s) => s.getAll());
  } catch {
    return [];
  }
}

export async function putSession(meta: SessionMeta): Promise<void> {
  try {
    await tx(STORE_SESSIONS, "readwrite", (s) => s.put(meta));
  } catch {
    /* IDB unavailable — ignore */
  }
}

export async function deleteSession(id: string): Promise<void> {
  try {
    await tx(STORE_SESSIONS, "readwrite", (s) => s.delete(id));
    await tx(STORE_MESSAGES, "readwrite", (s) => s.delete(id));
  } catch {
    /* ignore */
  }
}

// ---- Messages (per session) ----

export async function loadMessages(sessionId: string): Promise<PersistedMessage[]> {
  try {
    const result = await tx<PersistedMessage[] | undefined>(
      STORE_MESSAGES,
      "readonly",
      (s) => s.get(sessionId),
    );
    return result ?? [];
  } catch {
    return [];
  }
}

export async function saveMessages(
  sessionId: string,
  msgs: PersistedMessage[],
): Promise<void> {
  try {
    await tx(STORE_MESSAGES, "readwrite", (s) => s.put(msgs, sessionId));
  } catch {
    /* ignore */
  }
}
