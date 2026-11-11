import { Show, createEffect, createSignal, on, type Component } from "solid-js";
import { usePendingApproval, setPendingApproval, type PendingApproval } from "../lib/step";
import { postJson } from "../lib/api";
import { useWorkers } from "../lib/orchestrator";

/** StepAction wire shape mirrors `hone_proto::StepAction` exactly. */
type StepAction =
  | "Proceed"
  | "Skip"
  | { ApproveAlways: string }
  | { Deny: string };

// The prompt whose answer is currently in flight. Buttons are disabled
// and further clicks ignored until that POST settles, so a double-click
// can't answer the *next* worker's prompt unseen.
const [sendingFor, setSendingFor] = createSignal<PendingApproval | null>(null);

// Settle guard (mirrors the TUI): ignore clicks for SETTLE_MS after a
// prompt appears and after the last click, so a double-click's second
// click can't answer the next worker's prompt unseen.
const SETTLE_MS = 400;
const [settling, setSettling] = createSignal(false);
let settleTimer: ReturnType<typeof setTimeout> | undefined;
function startSettle(): void {
  setSettling(true);
  clearTimeout(settleTimer);
  settleTimer = setTimeout(() => setSettling(false), SETTLE_MS);
}

/** Statuses after which the dialog closes: success, or the prompt/run is gone. */
function closesDialog(status: number): boolean {
  return (status >= 200 && status < 300) || status === 404 || status === 410;
}

async function send(action: StepAction): Promise<void> {
  const p = usePendingApproval()();
  if (!p || sendingFor() === p || settling()) return;
  startSettle();
  setSendingFor(p);
  try {
    const resp = await postJson(`/run/${encodeURIComponent(p.runId)}/step`, action, null);
    if (!closesDialog(resp.status)) {
      // 401 / 429 / 5xx: keep the dialog open and re-enable so the user
      // can retry; closing would leave the run waiting forever.
      console.warn(`step ${resp.status}`);
      if (sendingFor() === p) setSendingFor(null);
      return;
    }
  } catch (err) {
    console.warn(`step failed: ${err}`);
    // Network failure: leave the prompt up and re-enable so the user can retry.
    if (sendingFor() === p) setSendingFor(null);
    return;
  }
  // The next worker's prompt may already have arrived; clear only the
  // one we answered.
  if (usePendingApproval()() === p) setPendingApproval(null);
  if (sendingFor() === p) setSendingFor(null);
}

export const ApprovalModal: Component = () => {
  const pending = usePendingApproval();
  const workers = useWorkers();
  const busy = () => sendingFor() === pending() || settling();
  // Each newly shown prompt starts a settle period.
  createEffect(on(pending, (p) => { if (p) startSettle(); }));
  const description = () => {
    const w = pending()?.worker;
    return w ? workers().find((x) => x.task_id === w.task_id)?.description : undefined;
  };

  return (
    <Show when={pending()}>
      <div
        role="dialog"
        aria-modal="true"
        // Focus the container (never a button) on open and whenever the
        // prompt changes, so Enter/Space can't hit a stale-focused button.
        tabIndex={-1}
        ref={(el) => {
          createEffect(on(pending, (p) => { if (p) el.focus(); }));
        }}
        style={{
          position: "fixed",
          inset: 0,
          background: "rgba(0,0,0,0.55)",
          display: "flex",
          "align-items": "center",
          "justify-content": "center",
          "z-index": 100,
          outline: "none",
        }}
        onClick={(e) => {
          // Click outside the dialog box dismisses with Skip.
          if (e.target === e.currentTarget) send("Skip");
        }}
      >
        <div
          style={{
            background: "var(--surface-raised)",
            border: "1px solid var(--surface-border)",
            "border-radius": "10px",
            padding: "20px",
            "min-width": "min(520px, 90vw)",
            "max-width": "min(720px, 90vw)",
            "max-height": "80vh",
            "overflow-y": "auto",
          }}
        >
          <div style={{ "margin-bottom": "8px", color: "var(--text-secondary)", "font-size": "0.85em" }}>
            agent wants to run a tool
          </div>
          <Show when={pending()!.worker}>
            {(w) => (
              <div
                style={{
                  "margin-bottom": "8px",
                  color: "var(--text-secondary)",
                  "font-size": "0.85em",
                  "overflow-wrap": "anywhere",
                  display: "-webkit-box",
                  "-webkit-line-clamp": "3",
                  "-webkit-box-orient": "vertical",
                  overflow: "hidden",
                }}
              >
                worker <strong>{w().agent}</strong> (subtask {w().task_id})
                <Show when={description()}>{(d) => <>: {d()}</>}</Show>
              </div>
            )}
          </Show>
          <div
            style={{
              "font-family": "var(--font-mono)",
              "font-size": "1.05em",
              color: "var(--role-tool)",
              "margin-bottom": "10px",
            }}
          >
            {pending()!.name}
          </div>
          <pre
            style={{
              background: "var(--surface-overlay)",
              border: "1px solid var(--surface-border)",
              "border-radius": "6px",
              padding: "8px 10px",
              "font-family": "var(--font-mono)",
              "font-size": "0.85em",
              "white-space": "pre-wrap",
              "max-height": "240px",
              "overflow-y": "auto",
              margin: "0 0 14px",
            }}
          >
            {prettyArgs(pending()!.args)}
          </pre>
          <div class="flex" style={{ gap: "8px", "flex-wrap": "wrap", "justify-content": "flex-end" }}>
            <button disabled={busy()} onClick={() => send({ Deny: "user denied" })} title="Reject this tool call">
              Deny
            </button>
            <button disabled={busy()} onClick={() => send("Skip")} title="Skip — agent proceeds without this tool">
              Skip
            </button>
            <button
              disabled={busy()}
              onClick={() => send({ ApproveAlways: pending()!.name })}
              title="Allow this tool name for the rest of the session"
            >
              Always allow
            </button>
            <button
              disabled={busy()}
              onClick={() => send("Proceed")}
              style={{ "border-color": "var(--accent-hot)", color: "var(--accent-hot)" }}
            >
              Allow
            </button>
          </div>
        </div>
      </div>
    </Show>
  );
};

function prettyArgs(args: unknown): string {
  try {
    return JSON.stringify(args, null, 2);
  } catch {
    return String(args);
  }
}
