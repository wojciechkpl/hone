import { defineConfig } from "vite";
import solid from "vite-plugin-solid";
import unocss from "@unocss/vite";

export default defineConfig({
  // Honed serves the SPA at /ui — match it so absolute asset URLs resolve
  // both in dev (vite serves at /) and in prod (rust-embed serves at /ui/).
  base: "/ui/",
  plugins: [unocss(), solid()],
  build: {
    outDir: "dist",
    emptyOutDir: true,
    target: "es2022",
    sourcemap: false,
  },
  server: {
    port: 5174,
    // The dev server proxies /run, /sessions, /health to the locally-running
    // honed instance so EventSource / fetch from the SPA work without CORS.
    proxy: {
      "/run": "http://127.0.0.1:3117",
      "/sessions": "http://127.0.0.1:3117",
      "/health": "http://127.0.0.1:3117",
      "/agents": "http://127.0.0.1:3117",
      "/recipes": "http://127.0.0.1:3117",
      "/memory": "http://127.0.0.1:3117",
      "/provenance": "http://127.0.0.1:3117",
      "/evals": "http://127.0.0.1:3117",
      "/sandbox": "http://127.0.0.1:3117",
      "/mcp": "http://127.0.0.1:3117",
      "/files": "http://127.0.0.1:3117",
    },
  },
});
