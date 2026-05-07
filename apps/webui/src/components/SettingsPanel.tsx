import { For, Show, type Component } from "solid-js";
import {
  FONT_SIZES,
  resetAll,
  setSettingsOpen,
  update,
  useSettings,
  useSettingsOpen,
  type FontSize,
} from "../lib/settings";
import type { Theme } from "./TopBar";

const THEMES: Theme[] = ["crimson", "chalk", "neon", "minimal"];

type Props = {
  theme: Theme;
  onPickTheme: (next: Theme) => void;
};

export const SettingsPanel: Component<Props> = (props) => {
  const open = useSettingsOpen();
  const settings = useSettings();

  return (
    <Show when={open()}>
      <div
        role="dialog"
        aria-modal="true"
        aria-label="Settings"
        style={{
          position: "fixed",
          inset: 0,
          background: "rgba(0,0,0,0.4)",
          "z-index": 200,
          display: "flex",
          "justify-content": "flex-end",
        }}
        onClick={(e) => {
          if (e.target === e.currentTarget) setSettingsOpen(false);
        }}
      >
        <aside
          style={{
            width: "min(420px, 92vw)",
            height: "100%",
            background: "var(--surface-raised)",
            "border-left": "1px solid var(--surface-border)",
            "box-shadow": "-12px 0 40px rgba(0,0,0,0.4)",
            display: "flex",
            "flex-direction": "column",
          }}
        >
          <header
            class="px-4"
            style={{
              display: "flex",
              "align-items": "center",
              "justify-content": "space-between",
              "border-bottom": "1px solid var(--surface-border)",
              height: "48px",
            }}
          >
            <strong>Settings</strong>
            <button
              onClick={() => setSettingsOpen(false)}
              style={{ padding: "4px 10px", "font-size": "0.85em" }}
              title="Close settings"
            >
              ✕
            </button>
          </header>

          <div
            style={{
              flex: 1,
              "overflow-y": "auto",
              padding: "16px",
              display: "flex",
              "flex-direction": "column",
              gap: "20px",
            }}
          >
            <Section title="Theme">
              <div style={{ display: "grid", "grid-template-columns": "1fr 1fr", gap: "8px" }}>
                <For each={THEMES}>
                  {(t) => (
                    <button
                      onClick={() => props.onPickTheme(t)}
                      aria-pressed={props.theme === t}
                      style={{
                        padding: "10px 12px",
                        "border-color":
                          props.theme === t ? "var(--accent-hot)" : "var(--surface-border)",
                        background:
                          props.theme === t
                            ? "var(--surface-overlay)"
                            : "var(--surface-base)",
                        color:
                          props.theme === t ? "var(--accent-hot)" : "var(--text-primary)",
                        "text-align": "center",
                      }}
                    >
                      {t}
                    </button>
                  )}
                </For>
              </div>
            </Section>

            <Section title="Font size">
              <div style={{ display: "grid", "grid-template-columns": "1fr 1fr 1fr", gap: "8px" }}>
                <For each={FONT_SIZES}>
                  {(size) => (
                    <button
                      onClick={() => update({ fontSize: size })}
                      aria-pressed={settings().fontSize === size}
                      style={{
                        padding: "8px 12px",
                        "border-color":
                          settings().fontSize === size
                            ? "var(--accent-hot)"
                            : "var(--surface-border)",
                        background:
                          settings().fontSize === size
                            ? "var(--surface-overlay)"
                            : "var(--surface-base)",
                        color:
                          settings().fontSize === size
                            ? "var(--accent-hot)"
                            : "var(--text-primary)",
                      }}
                    >
                      {fontSizeLabel(size)}
                    </button>
                  )}
                </For>
              </div>
            </Section>

            <Section title="Daemon endpoint">
              <p
                style={{
                  margin: 0,
                  "font-size": "0.8em",
                  color: "var(--text-secondary)",
                }}
              >
                Override the API base URL — leave blank to use the same
                origin (the default when honed serves the SPA at /ui).
              </p>
              <input
                type="text"
                placeholder="https://my-vm:3117"
                value={settings().daemonUrlOverride}
                onInput={(e) =>
                  update({ daemonUrlOverride: e.currentTarget.value.trim() })
                }
                style={{ width: "100%" }}
              />
              <p
                style={{
                  margin: "4px 0 0",
                  "font-size": "0.75em",
                  color: "var(--text-muted)",
                }}
              >
                Note: this Phase 3A slice persists the setting but the
                fetch helpers don't read it yet (next slice). Use
                Vite's dev proxy or a Tauri shell for cross-origin
                workflows today.
              </p>
            </Section>

            <Section title="About">
              <p
                style={{
                  margin: 0,
                  "font-size": "0.8em",
                  color: "var(--text-secondary)",
                  "line-height": 1.55,
                }}
              >
                Hone web UI — Phase 3A. Source at{" "}
                <a
                  href="https://github.com/wojciechkpl/hone"
                  target="_blank"
                  rel="noopener"
                  style={{ color: "var(--accent-hot)" }}
                >
                  github.com/wojciechkpl/hone
                </a>
                .
              </p>
            </Section>

            <button
              onClick={() => {
                if (
                  confirm(
                    "Reset all settings to defaults? Sessions and conversation history are kept.",
                  )
                ) {
                  resetAll();
                }
              }}
              style={{
                "align-self": "flex-start",
                "border-color": "var(--role-error)",
                color: "var(--role-error)",
              }}
            >
              Reset settings
            </button>
          </div>
        </aside>
      </div>
    </Show>
  );
};

const Section: Component<{ title: string; children: any }> = (props) => {
  return (
    <section style={{ display: "flex", "flex-direction": "column", gap: "8px" }}>
      <strong
        style={{
          "font-size": "0.75em",
          "text-transform": "uppercase",
          "letter-spacing": "0.05em",
          color: "var(--text-secondary)",
        }}
      >
        {props.title}
      </strong>
      {props.children}
    </section>
  );
};

function fontSizeLabel(s: FontSize): string {
  switch (s) {
    case "small":
      return "Small";
    case "medium":
      return "Medium";
    case "large":
      return "Large";
  }
}
