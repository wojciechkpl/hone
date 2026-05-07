// Lazy Shiki integration with explicit, curated language set.
//
// `createHighlighter` from the full Shiki bundle pulls EVERY language +
// theme into vite's chunk graph (~9 MB raw across 250+ chunks even when
// most are never loaded). We use `createHighlighterCore` plus dynamic
// per-language imports so only the languages we actually support get
// into the build.
//
// First-paint impact: zero. The Shiki module + WASM are fetched on
// first call to highlightCode, NOT at app boot.

import type { HighlighterCore, ThemeRegistration } from "shiki/core";

const DARK_THEME_NAME = "github-dark-default";
const LIGHT_THEME_NAME = "github-light";

let highlighterPromise: Promise<HighlighterCore> | null = null;
const langPromises = new Map<string, Promise<boolean>>();

/** Lazy-load Shiki + the two themes. Subsequent calls reuse the same
 * instance. Languages are loaded separately via `ensureLanguage`. */
async function getHighlighter(): Promise<HighlighterCore> {
  if (highlighterPromise) return highlighterPromise;
  highlighterPromise = (async () => {
    const [{ createHighlighterCore }, { createOnigurumaEngine }, dark, light] =
      await Promise.all([
        import("shiki/core"),
        import("shiki/engine/oniguruma"),
        import("shiki/themes/github-dark-default.mjs") as Promise<{ default: ThemeRegistration }>,
        import("shiki/themes/github-light.mjs") as Promise<{ default: ThemeRegistration }>,
      ]);
    const wasmModule = await import("shiki/wasm");
    return createHighlighterCore({
      themes: [dark.default, light.default],
      langs: [],
      engine: createOnigurumaEngine(wasmModule.default),
    });
  })();
  return highlighterPromise;
}

/** Whitelist of languages we'll load on demand. Each maps to the Shiki
 * langs/*.mjs path. Anything outside this set falls back to plain
 * text. */
const LANG_LOADERS: Record<string, () => Promise<unknown>> = {
  rust: () => import("shiki/langs/rust.mjs"),
  typescript: () => import("shiki/langs/typescript.mjs"),
  javascript: () => import("shiki/langs/javascript.mjs"),
  tsx: () => import("shiki/langs/tsx.mjs"),
  jsx: () => import("shiki/langs/jsx.mjs"),
  python: () => import("shiki/langs/python.mjs"),
  go: () => import("shiki/langs/go.mjs"),
  ruby: () => import("shiki/langs/ruby.mjs"),
  java: () => import("shiki/langs/java.mjs"),
  c: () => import("shiki/langs/c.mjs"),
  cpp: () => import("shiki/langs/cpp.mjs"),
  csharp: () => import("shiki/langs/csharp.mjs"),
  html: () => import("shiki/langs/html.mjs"),
  css: () => import("shiki/langs/css.mjs"),
  scss: () => import("shiki/langs/scss.mjs"),
  json: () => import("shiki/langs/json.mjs"),
  jsonc: () => import("shiki/langs/jsonc.mjs"),
  yaml: () => import("shiki/langs/yaml.mjs"),
  toml: () => import("shiki/langs/toml.mjs"),
  xml: () => import("shiki/langs/xml.mjs"),
  markdown: () => import("shiki/langs/markdown.mjs"),
  bash: () => import("shiki/langs/bash.mjs"),
  shell: () => import("shiki/langs/bash.mjs"),
  sql: () => import("shiki/langs/sql.mjs"),
  graphql: () => import("shiki/langs/graphql.mjs"),
  diff: () => import("shiki/langs/diff.mjs"),
  dockerfile: () => import("shiki/langs/docker.mjs"),
  makefile: () => import("shiki/langs/make.mjs"),
};

/** Best-effort mapping from a file extension to a language we know. */
export function langFromPath(path: string): string {
  const dot = path.lastIndexOf(".");
  if (dot < 0) {
    if (/(^|\/)Dockerfile$/i.test(path)) return "dockerfile";
    if (/(^|\/)Makefile$/i.test(path)) return "makefile";
    return "text";
  }
  const ext = path.slice(dot + 1).toLowerCase();
  const map: Record<string, string> = {
    rs: "rust", ts: "typescript", js: "javascript",
    tsx: "tsx", jsx: "jsx", py: "python", go: "go", rb: "ruby",
    java: "java",
    c: "c", h: "c", cc: "cpp", cpp: "cpp", hpp: "cpp", cxx: "cpp",
    cs: "csharp",
    html: "html", htm: "html", css: "css", scss: "scss",
    json: "json", jsonc: "jsonc", yaml: "yaml", yml: "yaml",
    toml: "toml", xml: "xml",
    md: "markdown", markdown: "markdown",
    sh: "bash", bash: "bash",
    sql: "sql", graphql: "graphql", gql: "graphql",
    diff: "diff", patch: "diff",
  };
  return map[ext] ?? "text";
}

export function isHighlightable(lang: string): boolean {
  return Object.hasOwn(LANG_LOADERS, lang);
}

async function ensureLanguage(lang: string, h: HighlighterCore): Promise<boolean> {
  if (!isHighlightable(lang)) return false;
  const cached = langPromises.get(lang);
  if (cached) return cached;
  const p = (async () => {
    try {
      const loader = LANG_LOADERS[lang];
      if (!loader) return false;
      const mod = (await loader()) as { default: unknown };
      await h.loadLanguage(mod.default as Parameters<HighlighterCore["loadLanguage"]>[0]);
      return true;
    } catch {
      return false;
    }
  })();
  langPromises.set(lang, p);
  return p;
}

export async function highlightCode(
  code: string,
  lang: string,
): Promise<string | null> {
  if (!isHighlightable(lang)) return null;
  try {
    const h = await getHighlighter();
    const ok = await ensureLanguage(lang, h);
    if (!ok) return null;
    const theme = pickTheme();
    return h.codeToHtml(code, { lang, theme });
  } catch {
    return null;
  }
}

function pickTheme(): string {
  if (typeof document === "undefined") return DARK_THEME_NAME;
  const t = document.documentElement.getAttribute("data-theme");
  return t === "chalk" || t === "minimal" ? LIGHT_THEME_NAME : DARK_THEME_NAME;
}
