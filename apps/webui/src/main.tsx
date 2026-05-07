/* @refresh reload */
import "uno.css";
import "./styles/base.css";
import "./styles/themes/crimson.css";
import "./styles/themes/chalk.css";
import "./styles/themes/neon.css";
import "./styles/themes/minimal.css";

import { render } from "solid-js/web";
import { App } from "./App";
import { applyToDocument, useSettings } from "./lib/settings";

// Apply persisted user settings (font size etc.) before the first
// paint so the layout doesn't flash to defaults and back.
applyToDocument(useSettings()());

const root = document.getElementById("root");
if (!root) {
  throw new Error("#root not found in index.html");
}
render(() => <App />, root);

// Register the service worker (production only). Dev keeps everything
// hot-reloaded by Vite without a SW intercepting fetches.
if ("serviceWorker" in navigator && import.meta.env.PROD) {
  window.addEventListener("load", () => {
    navigator.serviceWorker
      .register("/ui/sw.js", { scope: "/ui/" })
      .catch((err) => {
        // Service worker is purely a cache layer — register failure is
        // not fatal; the app still works in network-only mode.
        console.warn("service worker registration failed:", err);
      });
  });
}
