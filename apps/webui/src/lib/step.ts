// Step-mode (per-tool approval) state.
//
// When `enabled()` is true, ChatPanel sends `approval_policy: AskRisky`
// in the run request and waits for `tool_call_paused` events to pop a
// modal. The modal POSTs StepActions to `/run/{request_id}/step`.

import { createSignal } from "solid-js";

const [enabled, setEnabled] = createSignal(false);
export const useStepMode = () => enabled;
export const setStepMode = (b: boolean) => setEnabled(b);

export type PendingApproval = {
  runId: string;
  toolId: string;
  name: string;
  args: unknown;
};

const [pending, setPending] = createSignal<PendingApproval | null>(null);
export const usePendingApproval = () => pending;
export const setPendingApproval = (p: PendingApproval | null) => setPending(p);
