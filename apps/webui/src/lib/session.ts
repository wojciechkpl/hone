// Active-session signal + IndexedDB-backed metadata.
//
// One source of truth for "which session is the chat panel writing
// into right now." Sessions are buckets of messages stored locally;
// the daemon's POST /run is still per-turn fire-and-forget.

import { createSignal } from "solid-js";
import {
  listSessions,
  putSession,
  type SessionMeta,
} from "./db";

const STORAGE_KEY = "hone-webui:active-session";

const [active, setActive] = createSignal<string | null>(loadActive());

export const useActiveSession = () => active;

export function loadActive(): string | null {
  try {
    return localStorage.getItem(STORAGE_KEY);
  } catch {
    return null;
  }
}

export function setActiveSession(id: string | null): void {
  setActive(id);
  try {
    if (id == null) {
      localStorage.removeItem(STORAGE_KEY);
    } else {
      localStorage.setItem(STORAGE_KEY, id);
    }
  } catch {
    /* private mode etc. — fail open */
  }
}

/**
 * Ensure a session exists locally. If `id` is null we mint a fresh
 * one client-side and persist it. Returns the (possibly new) id.
 */
export async function ensureSession(id: string | null): Promise<string> {
  if (id) return id;
  const newId = (typeof crypto !== "undefined" && crypto.randomUUID)
    ? crypto.randomUUID()
    : `s-${Date.now().toString(36)}-${Math.random().toString(36).slice(2, 10)}`;
  const meta: SessionMeta = {
    id: newId,
    created_at: Date.now(),
    last_used_at: Date.now(),
  };
  await putSession(meta);
  setActiveSession(newId);
  return newId;
}

/**
 * Refresh `last_used_at` and (optionally) `title` for an existing
 * session. The first user prompt gets memoised as the title for the
 * SessionSelector list.
 */
export async function touchSession(id: string, title?: string): Promise<void> {
  const all = await listSessions();
  const existing = all.find((s) => s.id === id);
  const meta: SessionMeta = {
    id,
    created_at: existing?.created_at ?? Date.now(),
    last_used_at: Date.now(),
    title: existing?.title ?? title,
  };
  await putSession(meta);
}
