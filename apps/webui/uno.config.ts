import { defineConfig, presetMini } from "unocss";

// Tiny atomic CSS preset only — no icons, no web fonts, just the
// utilities. The full theme variables are declared in src/styles/themes
// and consumed via CSS custom properties.
export default defineConfig({
  presets: [presetMini()],
});
