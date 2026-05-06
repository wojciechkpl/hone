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
