// Workflow drawer state — toggled from the topbar button or the
// `/workflows` slash command. Mirrors the simple open-flag pattern in
// `lib/settings.ts`.

import { createSignal } from "solid-js";

const [open, setOpen] = createSignal(false);
export const useWorkflowsOpen = () => open;
export const setWorkflowsOpen = (v: boolean) => setOpen(v);
export const toggleWorkflows = () => setOpen(!open());

/**
 * Extract the unique `{{var}}` placeholders from a recipe step's
 * prompt. Used to render an input field per template variable.
 *
 * Returns the names in first-seen order; duplicates are de-duped.
 */
export function extractTemplateVars(steps: Array<{ prompt: string }>): string[] {
  const seen = new Set<string>();
  const out: string[] = [];
  const re = /\{\{\s*([A-Za-z_][A-Za-z0-9_]*)\s*\}\}/g;
  for (const step of steps) {
    let m: RegExpExecArray | null;
    re.lastIndex = 0;
    while ((m = re.exec(step.prompt)) !== null) {
      const name = m[1];
      if (!name) continue;
      if (!seen.has(name)) {
        seen.add(name);
        out.push(name);
      }
    }
  }
  return out;
}

/**
 * Bridge signal: `<WorkflowsPanel>` writes a fully-substituted prompt
 * here when the user clicks Run, and `<ChatPanel>` watches for it via a
 * `createEffect` and dispatches to the daemon. Drained by
 * `takePendingWorkflowRun`.
 */
const [pendingRun, setPendingRun] = createSignal<string | null>(null);
export const usePendingWorkflowRun = () => pendingRun;

export function setPendingWorkflowRun(prompt: string): void {
  setPendingRun(prompt);
}

export function takePendingWorkflowRun(): string | null {
  const v = pendingRun();
  if (v) setPendingRun(null);
  return v;
}

/** Substitute `{{var}}` occurrences in `text`. Unknown vars left as-is. */
export function substituteVars(text: string, vars: Record<string, string>): string {
  let out = text;
  for (const [k, v] of Object.entries(vars)) {
    out = out.replaceAll(`{{${k}}}`, v);
  }
  return out;
}
