import { createSignal, onMount, type Component } from "solid-js";
import { TopBar } from "./components/TopBar";
import { StatusBar } from "./components/StatusBar";
import { ChatPanel } from "./components/ChatPanel";
import { getJson, type DaemonHealth } from "./lib/api";

export const App: Component = () => {
  const [healthy, setHealthy] = createSignal<boolean | null>(null);
  const [theme, setTheme] = createSignal<"crimson" | "chalk">("crimson");

  function applyTheme(next: "crimson" | "chalk") {
    document.documentElement.setAttribute("data-theme", next);
    setTheme(next);
  }

  async function probeHealth() {
    try {
      await getJson<DaemonHealth>("/health", null);
      setHealthy(true);
    } catch {
      setHealthy(false);
    }
  }

  onMount(() => {
    probeHealth();
  });

  return (
    <>
      <TopBar
        theme={theme()}
        onToggleTheme={() => applyTheme(theme() === "crimson" ? "chalk" : "crimson")}
        healthy={healthy()}
        onRefreshHealth={probeHealth}
      />
      <ChatPanel />
      <StatusBar />
    </>
  );
};
