import { For, Show, createResource, createSignal, type Component } from "solid-js";
import {
  FONT_SIZES,
  resetAll,
  setSettingsOpen,
  update,
  useSettings,
  useSettingsOpen,
  type FontSize,
} from "../lib/settings";
import {
  deleteRequest,
  getJson,
  postJson,
  type TeamUsage,
  type TunnelInfo,
} from "../lib/api";
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

            <TunnelSection />
            <TeamUsageSection />

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

// ---------------------------------------------------------------------------
// Tunnel section — POST/DELETE/GET /tunnel
// ---------------------------------------------------------------------------

const TunnelSection: Component = () => {
  // Local versioning bump to force the resource to re-fetch after a
  // start/stop click. createResource is keyed on this signal.
  const [version, setVersion] = createSignal(0);
  const [busy, setBusy] = createSignal(false);
  const [error, setError] = createSignal<string | null>(null);
  const [info] = createResource(version, async () => {
    try {
      // GET /tunnel returns the TunnelInfo when active; honed responds
      // 204 No Content when not. We pretend that's null.
      const resp = await fetch("/tunnel");
      if (resp.status === 204) return null;
      if (!resp.ok) throw new Error(`tunnel status: ${resp.status}`);
      return (await resp.json()) as TunnelInfo;
    } catch (e) {
      // Network errors flag it as inactive rather than blowing up the
      // whole panel.
      setError(String(e));
      return null;
    }
  });

  async function start() {
    setBusy(true);
    setError(null);
    try {
      const resp = await postJson("/tunnel", { local_port: 3117 }, null);
      if (!resp.ok) throw new Error(`start ${resp.status}`);
      setVersion(version() + 1);
    } catch (e) {
      setError(String(e));
    } finally {
      setBusy(false);
    }
  }

  async function stop() {
    setBusy(true);
    setError(null);
    try {
      const resp = await deleteRequest("/tunnel");
      if (!resp.ok && resp.status !== 204) {
        throw new Error(`stop ${resp.status}`);
      }
      setVersion(version() + 1);
    } catch (e) {
      setError(String(e));
    } finally {
      setBusy(false);
    }
  }

  async function copyUrl() {
    const url = info()?.url;
    if (!url) return;
    try {
      await navigator.clipboard.writeText(url);
    } catch {
      /* clipboard blocked; user can select manually */
    }
  }

  return (
    <Section title="Tunnel">
      <Show
        when={!info.loading}
        fallback={<p style={{ "font-size": "0.8em", color: "var(--text-secondary)" }}>checking…</p>}
      >
        <Show
          when={info()?.is_active}
          fallback={
            <div style={{ display: "flex", "flex-direction": "column", gap: "6px" }}>
              <p style={{ margin: 0, "font-size": "0.8em", color: "var(--text-secondary)" }}>
                No public tunnel is active. honed is reachable on
                127.0.0.1 only until you start one.
              </p>
              <button onClick={start} disabled={busy()} style={{ "align-self": "flex-start" }}>
                Start tunnel
              </button>
            </div>
          }
        >
          <div style={{ display: "flex", "flex-direction": "column", gap: "6px" }}>
            <div
              style={{
                "font-family": "var(--font-mono)",
                "font-size": "0.85em",
                "word-break": "break-all",
                background: "var(--surface-overlay)",
                border: "1px solid var(--surface-border)",
                "border-radius": "6px",
                padding: "6px 8px",
              }}
            >
              {info()!.url}
            </div>
            <div style={{ display: "flex", gap: "6px" }}>
              <button onClick={copyUrl}>Copy URL</button>
              <button
                onClick={stop}
                disabled={busy()}
                style={{ "border-color": "var(--role-error)", color: "var(--role-error)" }}
              >
                Stop tunnel
              </button>
            </div>
          </div>
        </Show>
      </Show>
      <Show when={error()}>
        <p style={{ "font-size": "0.75em", color: "var(--role-error)", margin: 0 }}>{error()}</p>
      </Show>
    </Section>
  );
};

// ---------------------------------------------------------------------------
// Team usage — GET /team/usage
// ---------------------------------------------------------------------------

const TeamUsageSection: Component = () => {
  const [version, setVersion] = createSignal(0);
  const [usage] = createResource(version, async () => {
    try {
      return await getJson<TeamUsage>("/team/usage", null);
    } catch {
      return null;
    }
  });

  return (
    <Section title="Team usage">
      <Show
        when={!usage.loading}
        fallback={<p style={{ "font-size": "0.8em", color: "var(--text-secondary)" }}>loading…</p>}
      >
        <Show
          when={usage()}
          fallback={
            <p style={{ "font-size": "0.8em", color: "var(--text-secondary)" }}>
              unavailable (no governance data)
            </p>
          }
        >
          <div
            style={{
              "font-family": "var(--font-mono)",
              "font-size": "0.85em",
              background: "var(--surface-overlay)",
              border: "1px solid var(--surface-border)",
              "border-radius": "6px",
              padding: "6px 8px",
            }}
          >
            <div>
              total: {usage()!.total_tokens.toLocaleString()} tok ·
              ${usage()!.total_cost_usd.toFixed(4)}
            </div>
            <Show when={Object.keys(usage()!.by_model).length > 0}>
              <div style={{ "margin-top": "6px", "font-size": "0.85em" }}>
                <For each={Object.entries(usage()!.by_model)}>
                  {([model, m]) => (
                    <div style={{ color: "var(--text-secondary)" }}>
                      {model}: {m.tokens.toLocaleString()} tok · ${m.cost_usd.toFixed(4)}
                    </div>
                  )}
                </For>
              </div>
            </Show>
          </div>
          <button
            onClick={() => setVersion(version() + 1)}
            style={{ "align-self": "flex-start", "font-size": "0.85em", padding: "4px 10px" }}
          >
            Refresh
          </button>
        </Show>
      </Show>
    </Section>
  );
};
