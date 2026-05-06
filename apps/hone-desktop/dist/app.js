// Tauri's invoke is exposed at window.__TAURI__.core.invoke when capability is granted.
// We tolerate its absence (e.g. opening the HTML directly in a browser for layout work).
const tauri = (window.__TAURI__ && window.__TAURI__.core) || null;
const invoke = tauri ? tauri.invoke : async () => ({ stub: true });

const STATE_KEY = 'hone.gui.state.v1';

function loadState() {
  try { return JSON.parse(localStorage.getItem(STATE_KEY)) || {}; }
  catch { return {}; }
}

function saveState(patch) {
  const next = { ...loadState(), ...patch };
  localStorage.setItem(STATE_KEY, JSON.stringify(next));
  return next;
}

function show(id) { document.getElementById(id).classList.remove('hidden'); }
function hide(id) { document.getElementById(id).classList.add('hidden'); }

async function refreshDaemon() {
  const dot = document.getElementById('daemon-dot');
  const label = document.getElementById('daemon-label');
  const info = document.getElementById('daemon-info');
  try {
    const status = await invoke('daemon_status');
    if (status && status.healthy) {
      dot.className = 'dot dot--ok';
      label.textContent = `${status.mode} @ ${status.url}`;
    } else {
      dot.className = 'dot dot--bad';
      label.textContent = 'daemon not reachable';
    }
    info.textContent = JSON.stringify(status, null, 2);
  } catch (err) {
    dot.className = 'dot dot--bad';
    label.textContent = `error: ${err}`;
    info.textContent = String(err);
  }
}

document.addEventListener('DOMContentLoaded', async () => {
  const state = loadState();
  if (!state.mode) {
    show('wizard');
  } else {
    show('home');
    refreshDaemon();
  }

  document.querySelectorAll('.wizard-card').forEach((btn) => {
    btn.addEventListener('click', async () => {
      const mode = btn.dataset.mode;
      if (mode === 'sidecar') {
        saveState({ mode: 'sidecar' });
        try { await invoke('daemon_start'); } catch (e) { console.warn(e); }
        hide('wizard'); show('home'); refreshDaemon();
      } else {
        document.getElementById('external-form').classList.remove('hidden');
      }
    });
  });

  document.getElementById('external-form').addEventListener('submit', async (e) => {
    e.preventDefault();
    const url = document.getElementById('external-url').value.trim();
    const token = document.getElementById('external-token').value.trim() || null;
    if (!url) return;
    saveState({ mode: 'external', url, token });
    try { await invoke('daemon_set_external', { url, token }); } catch (err) { console.warn(err); }
    hide('wizard'); show('home'); refreshDaemon();
  });

  document.getElementById('btn-start').addEventListener('click', async () => {
    try { await invoke('daemon_start'); } catch (e) { console.warn(e); }
    refreshDaemon();
  });
  document.getElementById('btn-stop').addEventListener('click', async () => {
    try { await invoke('daemon_stop'); } catch (e) { console.warn(e); }
    refreshDaemon();
  });
  document.getElementById('btn-refresh').addEventListener('click', refreshDaemon);

  // Cmd+K / Ctrl+K palette
  const palette = document.getElementById('palette');
  const palInput = document.getElementById('palette-input');
  const palResults = document.getElementById('palette-results');
  const PALETTE_ITEMS = [
    { id: 'refresh', label: 'Refresh daemon status', run: refreshDaemon },
    { id: 'start', label: 'Start managed honed sidecar', run: async () => { await invoke('daemon_start'); refreshDaemon(); } },
    { id: 'stop',  label: 'Stop managed honed sidecar', run: async () => { await invoke('daemon_stop'); refreshDaemon(); } },
    { id: 'reset', label: 'Reset onboarding (re-show wizard)', run: () => { localStorage.removeItem(STATE_KEY); location.reload(); } },
  ];

  function renderPalette() {
    const q = palInput.value.toLowerCase();
    palResults.innerHTML = '';
    PALETTE_ITEMS
      .filter((it) => it.label.toLowerCase().includes(q))
      .forEach((it) => {
        const li = document.createElement('li');
        li.textContent = it.label;
        li.addEventListener('click', async () => { palette.classList.add('hidden'); await it.run(); });
        palResults.appendChild(li);
      });
  }

  function openPalette() {
    palette.classList.remove('hidden');
    palInput.value = '';
    renderPalette();
    palInput.focus();
  }
  function closePalette() { palette.classList.add('hidden'); }

  document.getElementById('palette-trigger').addEventListener('click', openPalette);
  palInput.addEventListener('input', renderPalette);
  document.addEventListener('keydown', (e) => {
    const meta = e.metaKey || e.ctrlKey;
    if (meta && e.key.toLowerCase() === 'k') {
      e.preventDefault();
      palette.classList.contains('hidden') ? openPalette() : closePalette();
    } else if (e.key === 'Escape' && !palette.classList.contains('hidden')) {
      closePalette();
    }
  });
});
