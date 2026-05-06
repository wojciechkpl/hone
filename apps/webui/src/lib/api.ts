// Tiny fetch wrapper that picks the right base URL.
//
// In dev (`vite`), requests are issued against `''` and the dev server's
// proxy in `vite.config.ts` forwards them to the locally-running honed.
// In prod (served by honed at `/ui`), requests are issued against the
// same origin so cookies / bearer tokens flow naturally.

export const baseUrl = "";

export async function getJson<T>(path: string, bearer: string | null): Promise<T> {
  const resp = await fetch(`${baseUrl}${path}`, {
    headers: bearer ? { authorization: `Bearer ${bearer}` } : undefined,
  });
  if (!resp.ok) throw new Error(`${path} → ${resp.status}`);
  return (await resp.json()) as T;
}

export type DaemonHealth = { status: string };

/** Wire shape of GET /agents — see crates/hone-server/src/discovery.rs */
export type AgentSummary = {
  name: string;
  description: string;
  model: string;
  tools: string[];
  sub_agents: string[];
  has_sandbox_profile: boolean;
  source: "builtin" | "user" | "project";
};
