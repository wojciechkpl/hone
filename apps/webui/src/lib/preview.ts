// Shared signals for the file preview pane: which file is open, and
// whether the pane itself is visible. Components in TopBar (toggle),
// ChatPanel (auto-open on tool calls), and FilePreviewPanel (consumer)
// all wire through here.

import { createSignal } from "solid-js";

const [previewOpen, setPreviewOpen] = createSignal(false);
export const usePreviewOpen = () => previewOpen;
export const setPreviewVisible = (open: boolean) => setPreviewOpen(open);
export const togglePreview = () => setPreviewOpen(!previewOpen());

/** Path relative to workspace_root, or null when nothing is selected. */
const [activeFile, setActiveFile] = createSignal<string | null>(null);
export const useActiveFile = () => activeFile;

/**
 * Open `path` in the preview pane (also reveals the pane).
 * Pass null to clear without closing.
 */
export function openFile(path: string | null): void {
  setActiveFile(path);
  if (path) setPreviewOpen(true);
}

// ---------------------------------------------------------------------------
// Split ratio (gutter position)
// ---------------------------------------------------------------------------
//
// Stored as the chat panel's fraction of the row width: 0.6 means
// "chat = 60%, preview = 40%". Clamped to [0.25, 0.85] so neither
// pane can be dragged into oblivion. Persisted to localStorage so
// the layout survives reload.

const SPLIT_KEY = "hone-webui:split-ratio";
const MIN_SPLIT = 0.25;
const MAX_SPLIT = 0.85;
const DEFAULT_SPLIT = 0.55;

const [splitRatio, setSplitRatio] = createSignal<number>(loadSplit());
export const useSplitRatio = () => splitRatio;

function loadSplit(): number {
  try {
    const raw = localStorage.getItem(SPLIT_KEY);
    if (!raw) return DEFAULT_SPLIT;
    const v = parseFloat(raw);
    if (Number.isFinite(v) && v >= MIN_SPLIT && v <= MAX_SPLIT) return v;
  } catch {
    /* fall through */
  }
  return DEFAULT_SPLIT;
}

export function applySplitRatio(next: number): void {
  const clamped = Math.max(MIN_SPLIT, Math.min(MAX_SPLIT, next));
  setSplitRatio(clamped);
  try {
    localStorage.setItem(SPLIT_KEY, String(clamped));
  } catch {
    /* private mode etc. */
  }
}
