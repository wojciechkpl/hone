import { createSignal, onMount, type Component } from "solid-js";
import { TopBar, type Theme } from "./components/TopBar";
import { StatusBar } from "./components/StatusBar";
import { ChatPanel } from "./components/ChatPanel";
import { ApprovalModal } from "./components/ApprovalModal";
import { getJson, type DaemonHealth } from "./lib/api";
import { resetCost } from "./lib/cost";
import { ensureSession, useActiveSession } from "./lib/session";

const THEMES: Theme[] = ["crimson", "chalk", "neon", "minimal"];

export const App: Component = () => {
  const [healthy, setHealthy] = createSignal<boolean | null>(null);
  const [theme, setTheme] = createSignal<Theme>("crimson");
  const active = useActiveSession();

  function applyTheme(next: Theme) {
    document.documentElement.setAttribute("data-theme", next);
    setTheme(next);
  }

  function cycleTheme() {
    const cur = theme();
    const idx = THEMES.indexOf(cur);
    const next = THEMES[(idx + 1) % THEMES.length] ?? "crimson";
    applyTheme(next);
  }

  async function probeHealth() {
    try {
      await getJson<DaemonHealth>("/health", null);
      setHealthy(true);
    } catch {
      setHealthy(false);
    }
  }

  onMount(async () => {
    void probeHealth();
    // Make sure we always have a current session so the chat panel
    // has somewhere to persist.
    await ensureSession(active());
  });

  function setThemeByName(name: string): boolean {
    const lower = name.toLowerCase() as Theme;
    if ((THEMES as string[]).includes(lower)) {
      applyTheme(lower);
      return true;
    }
    return false;
  }

  return (
    <>
      <TopBar
        theme={theme()}
        onCycleTheme={cycleTheme}
        healthy={healthy()}
        onRefreshHealth={probeHealth}
        onSwitchSession={() => {
          // ChatPanel reacts to the active-session signal via
          // createEffect — nothing else to do here.
        }}
      />
      <ChatPanel
        onSlashCommand={(cmd, arg) => handleSlashCommand(cmd, arg, { setThemeByName })}
      />
      <StatusBar />
      <ApprovalModal />
    </>
  );
};

function handleSlashCommand(
  cmd: string,
  arg: string,
  ctx: { setThemeByName: (name: string) => boolean },
): { ok: boolean; reply?: string } {
  switch (cmd) {
    case "/theme":
      if (!arg) {
        return { ok: false, reply: `usage: /theme <crimson|chalk|neon|minimal>` };
      }
      return ctx.setThemeByName(arg)
        ? { ok: true, reply: `theme set to ${arg}` }
        : { ok: false, reply: `unknown theme: ${arg}` };
    case "/clear":
      resetCost();
      return { ok: true, reply: "cost meter cleared" };
    case "/help":
      return {
        ok: true,
        reply:
          "available: /theme <name>, /clear (reset cost meter), /help",
      };
    default:
      return { ok: false, reply: `unknown command: ${cmd}` };
  }
}
