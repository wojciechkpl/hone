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

export async function postJson(
  path: string,
  body: unknown,
  bearer: string | null,
): Promise<Response> {
  const headers: Record<string, string> = { "content-type": "application/json" };
  if (bearer) headers.authorization = `Bearer ${bearer}`;
  return fetch(`${baseUrl}${path}`, {
    method: "POST",
    headers,
    body: JSON.stringify(body),
  });
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

/** Wire shape of GET /files — see crates/hone-server/src/files_routes.rs */
export type FileEntry = {
  name: string;
  path: string;
  is_dir: boolean;
  size: number;
};

export async function fetchText(path: string): Promise<string> {
  const resp = await fetch(`${baseUrl}${path}`);
  if (!resp.ok) throw new Error(`${path} → ${resp.status}`);
  return resp.text();
}

export async function deleteRequest(path: string): Promise<Response> {
  return fetch(`${baseUrl}${path}`, { method: "DELETE" });
}

// ---- Tunnel (POST/DELETE/GET /tunnel) ----

export type TunnelInfo = {
  url: string;
  local_port: number;
  is_active: boolean;
};

// ---- Team usage / governance ----
// Wire shape mirrors hone-server::governance::TeamUsage.

export type ModelUsage = {
  tokens: number;
  cost_usd: number;
};

export type TeamUsage = {
  total_tokens: number;
  total_cost_usd: number;
  /** keyed by model id */
  by_model: Record<string, ModelUsage>;
};

// ---- Recipes (workflows) ----

/** Wire shape of GET /recipes — see crates/hone-server/src/discovery.rs */
export type RecipeSummary = {
  name: string;
  description: string | null;
  step_count: number;
  has_test_cmd: boolean;
  source: "builtin" | "user" | "project";
  path: string;
};

export type RecipeStep = {
  name: string;
  prompt: string;
};

/** Wire shape of GET /recipes/{name} */
export type RecipeDetail = {
  name: string;
  description: string | null;
  steps: RecipeStep[];
  test_cmd: string | null;
  source: "builtin" | "user" | "project";
  path: string;
};

/** Body shape for POST /recipes */
export type CreateRecipeRequest = {
  name: string;
  description?: string | null;
  steps: RecipeStep[];
  test_cmd?: string | null;
};

// ---- Collaborative sessions ----

export type ParticipantRole = "Owner" | "Editor" | "Viewer";
export type Participant = {
  user_id: string;
  name: string;
  role: ParticipantRole;
  joined_at: number;
};
