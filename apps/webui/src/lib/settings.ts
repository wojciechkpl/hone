// Persistent app-level settings — drawer state + user preferences.
//
// Theme is owned by App (it has to apply on mount before paint), so
// it lives separately. Everything else that surfaces in the
// SettingsPanel goes through here.

import { createSignal } from "solid-js";

const STORAGE_KEY = "hone-webui:settings";

export type FontSize = "small" | "medium" | "large";
export const FONT_SIZES: FontSize[] = ["small", "medium", "large"];

const FONT_PX: Record<FontSize, number> = {
  small: 13,
  medium: 14, // matches the base.css default
  large: 16,
};

export type Settings = {
  fontSize: FontSize;
  /** Override the daemon base URL when running outside same-origin
   * (e.g. an external Tauri shell or a standalone Node dev server).
   * Empty string = same-origin (the production default). */
  daemonUrlOverride: string;
};

const DEFAULT: Settings = {
  fontSize: "medium",
  daemonUrlOverride: "",
};

const [settings, setSettings] = createSignal<Settings>(load());
export const useSettings = () => settings;

function load(): Settings {
  try {
    const raw = localStorage.getItem(STORAGE_KEY);
    if (!raw) return DEFAULT;
    const parsed = JSON.parse(raw) as Partial<Settings>;
    return {
      fontSize: FONT_SIZES.includes(parsed.fontSize as FontSize)
        ? (parsed.fontSize as FontSize)
        : DEFAULT.fontSize,
      daemonUrlOverride: typeof parsed.daemonUrlOverride === "string"
        ? parsed.daemonUrlOverride
        : DEFAULT.daemonUrlOverride,
    };
  } catch {
    return DEFAULT;
  }
}

function save(next: Settings): void {
  try {
    localStorage.setItem(STORAGE_KEY, JSON.stringify(next));
  } catch {
    /* private mode etc. */
  }
}

export function update(patch: Partial<Settings>): void {
  const next = { ...settings(), ...patch };
  setSettings(next);
  save(next);
  applyToDocument(next);
}

/** Mutate document-level styles to match the active settings. Called
 * once at boot from main.tsx + on every update. */
export function applyToDocument(s: Settings): void {
  if (typeof document === "undefined") return;
  document.documentElement.style.fontSize = `${FONT_PX[s.fontSize]}px`;
}

/** Wipe everything we own back to defaults. */
export function resetAll(): void {
  setSettings(DEFAULT);
  save(DEFAULT);
  applyToDocument(DEFAULT);
}

// ---------------------------------------------------------------------------
// Drawer open/close
// ---------------------------------------------------------------------------

const [open, setOpen] = createSignal(false);
export const useSettingsOpen = () => open;
export const setSettingsOpen = (v: boolean) => setOpen(v);
export const toggleSettings = () => setOpen(!open());
