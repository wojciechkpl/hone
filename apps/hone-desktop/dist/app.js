// Tauri 2 exposes invoke + event listening at window.__TAURI__.
const tauri = window.__TAURI__ || null;
const invoke = (tauri && tauri.core && tauri.core.invoke) ? tauri.core.invoke : async () => ({ stub: true });
const listen = (tauri && tauri.event && tauri.event.listen) ? tauri.event.listen : async () => () => {};

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

// ---------------------------------------------------------------------------
// Daemon status
// ---------------------------------------------------------------------------

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
    if (info) info.textContent = JSON.stringify(status, null, 2);
  } catch (err) {
    dot.className = 'dot dot--bad';
    label.textContent = `error: ${err}`;
    if (info) info.textContent = String(err);
  }
}

// ---------------------------------------------------------------------------
// Cost / token meter
// ---------------------------------------------------------------------------

const cost = {
  tokens: 0,
  usd: 0.0,
  reset() { this.tokens = 0; this.usd = 0.0; this.render(); },
  add(usage, usd) {
    if (usage && typeof usage === 'object') {
      const total = (usage.input || 0) + (usage.output || 0)
                  + (usage.cache_read || 0) + (usage.cache_creation || 0);
      this.tokens += total;
    }
    if (typeof usd === 'number') this.usd += usd;
    this.render();
  },
  render() {
    document.getElementById('cost-tokens').textContent = `${this.tokens.toLocaleString()} tok`;
    document.getElementById('cost-usd').textContent = `$${this.usd.toFixed(3)}`;
  },
};

// ---------------------------------------------------------------------------
// Chat surface
// ---------------------------------------------------------------------------

const chatState = {
  history: [],            // [{role, content}]
  activeRunId: null,
  activeAssistantEl: null,
  toolCards: new Map(),   // tool_id -> DOM node
};

function appendUserMessage(text) {
  const el = document.createElement('div');
  el.className = 'message user';
  el.textContent = text;
  document.getElementById('messages').appendChild(el);
  scrollMessages();
  return el;
}

function startAssistantBubble() {
  const el = document.createElement('div');
  el.className = 'message assistant';
  el.dataset.streaming = '1';
  document.getElementById('messages').appendChild(el);
  scrollMessages();
  chatState.activeAssistantEl = el;
  return el;
}

function appendToolCard(id, name) {
  const el = document.createElement('div');
  el.className = 'tool-card';
  el.dataset.toolId = id;
  el.innerHTML = `<span class="name"></span><span class="status">running…</span>`;
  el.querySelector('.name').textContent = name;
  document.getElementById('messages').appendChild(el);
  scrollMessages();
  chatState.toolCards.set(id, el);
  return el;
}

function finalizeToolCard(id, payload) {
  const el = chatState.toolCards.get(id);
  if (!el) return;
  el.querySelector('.status').textContent = `${payload.duration_ms || 0} ms`;
  if (payload.is_error) el.classList.add('error');
  if (payload.cache_hit) el.classList.add('cache');
  if (payload.result !== undefined) {
    const pre = document.createElement('pre');
    pre.textContent = typeof payload.result === 'string'
      ? payload.result
      : JSON.stringify(payload.result, null, 2);
    el.appendChild(pre);
  }
}

function appendErrorBubble(message) {
  const el = document.createElement('div');
  el.className = 'error-bubble';
  el.textContent = message;
  document.getElementById('messages').appendChild(el);
  scrollMessages();
}

function scrollMessages() {
  const m = document.getElementById('messages');
  m.scrollTop = m.scrollHeight;
}

function handleAgentEvent(event) {
  switch (event.type) {
    case 'token': {
      if (!chatState.activeAssistantEl) startAssistantBubble();
      chatState.activeAssistantEl.appendChild(document.createTextNode(event.text || ''));
      scrollMessages();
      break;
    }
    case 'tool_call_start': {
      appendToolCard(event.id, event.name);
      break;
    }
    case 'tool_call_end': {
      finalizeToolCard(event.id, event);
      break;
    }
    case 'done': {
      if (chatState.activeAssistantEl) {
        const meta = document.createElement('span');
        meta.className = 'turn-meta';
        const u = event.usage || {};
        const total = (u.input || 0) + (u.output || 0) + (u.cache_read || 0) + (u.cache_creation || 0);
        meta.textContent = `${total} tok · $${(event.cost_usd || 0).toFixed(4)}`;
        chatState.activeAssistantEl.appendChild(meta);
        // Push the rendered text into history so subsequent turns include it.
        const content = Array.from(chatState.activeAssistantEl.childNodes)
          .filter((n) => n.nodeType === Node.TEXT_NODE)
          .map((n) => n.textContent)
          .join('');
        if (content.trim()) chatState.history.push({ role: 'assistant', content });
      }
      cost.add(event.usage, event.cost_usd);
      finishRun();
      break;
    }
    case 'error': {
      appendErrorBubble(event.message || 'unknown error');
      finishRun();
      break;
    }
    case 'budget_progress': {
      if (typeof event.spent_usd === 'number') {
        // Replace cumulative usd if the daemon provides authoritative number.
        cost.usd = event.spent_usd;
        cost.render();
      }
      break;
    }
    default:
      // Unhandled event types (worker_*, plan_*, approval_required,
      // memory_recalled, ...) are surfaced only in the next slice.
      break;
  }
}

function finishRun() {
  chatState.activeRunId = null;
  chatState.activeAssistantEl = null;
  document.getElementById('btn-send').disabled = false;
  document.getElementById('btn-cancel').classList.add('hidden');
}

async function sendPrompt() {
  const ta = document.getElementById('chat-input');
  const prompt = ta.value.trim();
  if (!prompt || chatState.activeRunId) return;
  ta.value = '';
  document.getElementById('btn-send').disabled = true;
  document.getElementById('btn-cancel').classList.remove('hidden');

  appendUserMessage(prompt);
  chatState.history.push({ role: 'user', content: prompt });
  startAssistantBubble();
  chatState.toolCards.clear();

  try {
    const runId = await invoke('run_turn', {
      request: {
        prompt,
        history: chatState.history.slice(0, -1), // server takes the last as current; pass prior turns
      },
    });
    chatState.activeRunId = runId;
  } catch (err) {
    appendErrorBubble(`run_turn failed: ${err}`);
    finishRun();
  }
}

async function cancelRun() {
  if (!chatState.activeRunId) return;
  try { await invoke('run_cancel', { runId: chatState.activeRunId }); } catch (e) { console.warn(e); }
  appendErrorBubble('cancelled');
  finishRun();
}

// ---------------------------------------------------------------------------
// Boot
// ---------------------------------------------------------------------------

document.addEventListener('DOMContentLoaded', async () => {
  cost.render();

  // Subscribe to streaming events from the Rust runtime.
  await listen('hone:event', (e) => {
    const payload = e.payload || {};
    if (chatState.activeRunId && payload.run_id !== chatState.activeRunId) return;
    handleAgentEvent(payload.event || {});
  });

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

  // Chat
  document.getElementById('chat-form').addEventListener('submit', (e) => {
    e.preventDefault();
    sendPrompt();
  });
  document.getElementById('chat-input').addEventListener('keydown', (e) => {
    if ((e.metaKey || e.ctrlKey) && e.key === 'Enter') {
      e.preventDefault();
      sendPrompt();
    }
  });
  document.getElementById('btn-cancel').addEventListener('click', cancelRun);

  // Cmd+K palette
  const palette = document.getElementById('palette');
  const palInput = document.getElementById('palette-input');
  const palResults = document.getElementById('palette-results');
  const PALETTE_ITEMS = [
    { id: 'refresh', label: 'Refresh daemon status', run: refreshDaemon },
    { id: 'start', label: 'Start managed honed sidecar', run: async () => { await invoke('daemon_start'); refreshDaemon(); } },
    { id: 'stop',  label: 'Stop managed honed sidecar', run: async () => { await invoke('daemon_stop'); refreshDaemon(); } },
    { id: 'cancel', label: 'Cancel current run', run: cancelRun },
    { id: 'cost-reset', label: 'Reset session cost meter', run: () => cost.reset() },
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
  function openPalette() { palette.classList.remove('hidden'); palInput.value = ''; renderPalette(); palInput.focus(); }
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
