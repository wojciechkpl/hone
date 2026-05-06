import { Show, type Component } from "solid-js";
import { usePendingApproval, setPendingApproval } from "../lib/step";
import { postJson } from "../lib/api";

/** StepAction wire shape mirrors `hone_proto::StepAction` exactly. */
type StepAction =
  | "Proceed"
  | "Skip"
  | { ApproveAlways: string }
  | { Deny: string };

async function send(action: StepAction): Promise<void> {
  const p = usePendingApproval()();
  if (!p) return;
  try {
    const resp = await postJson(`/run/${encodeURIComponent(p.runId)}/step`, action, null);
    if (!resp.ok) {
      console.warn(`step ${resp.status}`);
    }
  } catch (err) {
    console.warn(`step failed: ${err}`);
  }
  setPendingApproval(null);
}

export const ApprovalModal: Component = () => {
  const pending = usePendingApproval();

  return (
    <Show when={pending()}>
      <div
        role="dialog"
        aria-modal="true"
        style={{
          position: "fixed",
          inset: 0,
          background: "rgba(0,0,0,0.55)",
          display: "flex",
          "align-items": "center",
          "justify-content": "center",
          "z-index": 100,
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
            <button onClick={() => send({ Deny: "user denied" })} title="Reject this tool call">
              Deny
            </button>
            <button onClick={() => send("Skip")} title="Skip — agent proceeds without this tool">
              Skip
            </button>
            <button
              onClick={() => send({ ApproveAlways: pending()!.name })}
              title="Allow this tool name for the rest of the session"
            >
              Always allow
            </button>
            <button
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
