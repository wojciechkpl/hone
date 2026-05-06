// Shared "active agent" selection.
//
// The `model` field on `RunRequest` only takes a model id today, not an
// agent name — `/agents` is a discovery surface, not yet a dispatch
// surface in honed (TUI handles agent routing locally). Phase 1 mirrors
// that: the picker is informational + sets the default model the
// chosen agent declares. A future PR can propagate the agent name to
// the daemon once it exposes an `/agents/{name}/run` endpoint.

import { createSignal } from "solid-js";
import type { AgentSummary } from "./api";

const [active, setActive] = createSignal<AgentSummary | null>(null);

export const useActiveAgent = () => active;
export const setActiveAgent = (a: AgentSummary | null) => setActive(a);
