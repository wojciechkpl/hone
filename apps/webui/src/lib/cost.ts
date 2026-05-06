// Shared cost / token accumulator for the active session.
//
// Single source of truth so the StatusBar (read-only) and ChatPanel
// (writes on each `done` event) stay in sync without prop drilling.
// Phase 1 keeps this module-scoped; if multi-session lands in Phase
// 1B this becomes per-session state.

import { createSignal } from "solid-js";

export type Cost = { tokens: number; usd: number };

const [cost, setCost] = createSignal<Cost>({ tokens: 0, usd: 0 });

export const useCost = () => cost;

export function addUsage(usage: {
  input?: number;
  output?: number;
  cache_read?: number;
  cache_creation?: number;
} | undefined, usd: number | undefined): void {
  const u = usage ?? {};
  const tokens =
    (u.input ?? 0) + (u.output ?? 0) + (u.cache_read ?? 0) + (u.cache_creation ?? 0);
  const cur = cost();
  setCost({ tokens: cur.tokens + tokens, usd: cur.usd + (usd ?? 0) });
}

/** Reset the meter — wired to the `/clear` slash command. */
export function resetCost(): void {
  setCost({ tokens: 0, usd: 0 });
}
