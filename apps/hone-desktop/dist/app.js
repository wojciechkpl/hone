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
    // Prefix the topbar label with the active profile name when known.
    let prefix = '';
    try {
      const list = await invoke('list_daemons');
      if (list && list.active) prefix = `${list.active} · `;
    } catch (_) {}

    if (status && status.healthy) {
      dot.className = 'dot dot--ok';
      label.textContent = `${prefix}${status.mode} @ ${status.url}`;
    } else {
      dot.className = 'dot dot--bad';
      label.textContent = prefix ? `${prefix}unreachable` : 'daemon not reachable';
    }
    if (info) info.textContent = JSON.stringify(status, null, 2);
  } catch (err) {
    dot.className = 'dot dot--bad';
    label.textContent = `error: ${err}`;
    if (info) info.textContent = String(err);
  }
}

// ---------------------------------------------------------------------------
// Multi-daemon switcher (topbar dropdown)
// ---------------------------------------------------------------------------

async function openDaemonMenu() {
  const menu = document.getElementById('daemon-menu');
  const list = document.getElementById('daemon-menu-list');
  list.innerHTML = '<li class="hint">loading…</li>';
  menu.classList.remove('hidden');
  document.getElementById('daemon-switcher').setAttribute('aria-expanded', 'true');

  try {
    const data = await invoke('list_daemons');
    list.innerHTML = '';
    const profiles = (data && Array.isArray(data.daemons)) ? data.daemons : [];
    const active = data && data.active;
    if (profiles.length === 0) {
      const li = document.createElement('li');
      li.className = 'hint';
      li.textContent = '(no profiles yet — add one below)';
      list.appendChild(li);
    }
    for (const p of profiles) {
      list.appendChild(renderDaemonProfileRow(p, active));
    }
  } catch (err) {
    list.innerHTML = '';
    const li = document.createElement('li');
    li.className = 'hint';
    li.textContent = `error: ${err}`;
    list.appendChild(li);
  }
}

function closeDaemonMenu() {
  document.getElementById('daemon-menu').classList.add('hidden');
  document.getElementById('daemon-switcher').setAttribute('aria-expanded', 'false');
}

function renderDaemonProfileRow(profile, activeName) {
  const li = document.createElement('li');
  if (profile.name === activeName) li.classList.add('active');

  const name = document.createElement('span');
  name.className = 'name';
  name.textContent = profile.name;
  li.appendChild(name);

  const meta = document.createElement('span');
  meta.className = 'meta';
  meta.textContent = describeProfileMode(profile);
  li.appendChild(meta);

  const actions = document.createElement('div');
  actions.className = 'actions';
  if (profile.name !== activeName) {
    const switchBtn = document.createElement('button');
    switchBtn.textContent = 'Switch';
    switchBtn.addEventListener('click', async () => {
      try {
        await invoke('switch_daemon', { name: profile.name });
        closeDaemonMenu();
        refreshDaemon();
      } catch (err) { appendErrorBubble(`switch failed: ${err}`); }
    });
    actions.appendChild(switchBtn);
  }
  const removeBtn = document.createElement('button');
  removeBtn.textContent = '✕';
  removeBtn.title = 'Remove profile';
  removeBtn.addEventListener('click', async () => {
    if (!confirm(`Remove profile '${profile.name}'?`)) return;
    try {
      await invoke('remove_daemon', { name: profile.name });
      openDaemonMenu();
      refreshDaemon();
    } catch (err) { appendErrorBubble(`remove failed: ${err}`); }
  });
  actions.appendChild(removeBtn);
  li.appendChild(actions);
  return li;
}

function describeProfileMode(profile) {
  // Wire shape (internally-tagged + flattened):
  //   sidecar  → { name, kind: "sidecar" }
  //   external → { name, kind: "external", url, token }
  if (!profile) return '';
  if (profile.kind === 'external' && profile.url) {
    return `external @ ${profile.url}`;
  }
  return 'sidecar (managed)';
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
  pendingApprovals: new Map(), // approvalId -> {runId, name, desc, kind}
  plan: null,             // { goal, subtask_count } | null
  workers: new Map(),     // task_id -> { agent, description, status, cost_usd }
  budget: null,           // { spent_usd, budget_usd, workers_completed, workers_total }
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
    case 'tool_call_paused': {
      // Step-mode pause — render in approval queue. The id is the tool
      // call's id; we use it to dedupe in case the agent re-emits.
      enqueueApproval({
        runId: chatState.activeRunId,
        approvalId: `tool:${event.id}`,
        name: event.name,
        desc: event.args ? JSON.stringify(event.args, null, 2) : '',
        kind: 'tool',
      });
      break;
    }
    case 'approval_required': {
      enqueueApproval({
        runId: chatState.activeRunId,
        approvalId: `req:${Date.now()}-${Math.random().toString(36).slice(2,8)}`,
        name: event.action || 'unknown',
        desc: event.description || '',
        kind: 'generic',
      });
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
      chatState.budget = {
        spent_usd: event.spent_usd,
        budget_usd: event.budget_usd,
        workers_completed: event.workers_completed,
        workers_total: event.workers_total,
      };
      if (typeof event.spent_usd === 'number') {
        cost.usd = event.spent_usd;
        cost.render();
      }
      renderTaskTree();
      break;
    }
    case 'plan_created': {
      chatState.plan = {
        goal: event.goal || '(no goal)',
        subtask_count: event.subtask_count || 0,
      };
      // Workers start in `queued` once the plan exists; they're filled in
      // as worker_started arrives. We don't yet know the per-task ids
      // until worker_started fires.
      chatState.workers.clear();
      renderTaskTree();
      break;
    }
    case 'worker_started': {
      chatState.workers.set(event.task_id, {
        agent: event.agent || 'unknown',
        description: event.description || '',
        status: 'running',
        cost_usd: 0,
      });
      renderTaskTree();
      break;
    }
    case 'worker_completed': {
      const cur = chatState.workers.get(event.task_id) || {
        agent: event.agent || 'unknown',
        description: '',
      };
      cur.status = (event.status === 'completed') ? 'completed' : 'failed';
      cur.cost_usd = event.cost_usd || 0;
      cur.agent = event.agent || cur.agent;
      chatState.workers.set(event.task_id, cur);
      renderTaskTree();
      break;
    }
    default:
      // Unhandled event types (memory_recalled, model_info, etc.) are
      // surfaced when there's a UI use for them.
      break;
  }
}

function finishRun() {
  chatState.activeRunId = null;
  chatState.activeAssistantEl = null;
  document.getElementById('btn-send').disabled = false;
  document.getElementById('btn-cancel').classList.add('hidden');
  // A finished run invalidates any leftover approvals — the daemon won't
  // accept steps for it anymore.
  chatState.pendingApprovals.clear();
  renderApprovalQueue();
}

// ---------------------------------------------------------------------------
// Approval queue
// ---------------------------------------------------------------------------

function enqueueApproval(item) {
  if (!item.runId) return;
  chatState.pendingApprovals.set(item.approvalId, item);
  renderApprovalQueue();
}

function renderApprovalQueue() {
  const root = document.getElementById('approval-queue');
  const empty = document.getElementById('approval-empty');
  const badge = document.getElementById('approval-count');
  const items = Array.from(chatState.pendingApprovals.values());

  badge.textContent = String(items.length);
  badge.classList.toggle('hidden', items.length === 0);
  empty.style.display = items.length === 0 ? 'block' : 'none';

  // Wipe non-empty rendering area
  Array.from(root.querySelectorAll('.approval-card')).forEach((el) => el.remove());

  for (const item of items) {
    const card = document.createElement('div');
    card.className = 'approval-card';
    const name = document.createElement('span');
    name.className = 'name';
    name.textContent = item.name;
    const desc = document.createElement('div');
    desc.className = 'desc';
    desc.textContent = item.desc || '(no detail provided)';
    const actions = document.createElement('div');
    actions.className = 'actions';

    const buttons = [
      { label: 'Approve', action: 'Proceed' },
      { label: 'Approve all', action: { ApproveAlways: item.name } },
      { label: 'Skip', action: 'Skip' },
      { label: 'Deny', action: { Deny: 'user denied' } },
    ];
    for (const b of buttons) {
      const btn = document.createElement('button');
      btn.textContent = b.label;
      btn.addEventListener('click', () => respondToApproval(item, b.action));
      actions.appendChild(btn);
    }

    card.append(name, desc, actions);
    root.appendChild(card);
  }
}

async function respondToApproval(item, action) {
  try {
    await invoke('run_step', { runId: item.runId, action });
    chatState.pendingApprovals.delete(item.approvalId);
    renderApprovalQueue();
  } catch (err) {
    appendErrorBubble(`step failed: ${err}`);
  }
}

// ---------------------------------------------------------------------------
// Discover (agents + recipes)
// ---------------------------------------------------------------------------

async function refreshAgents() {
  const list = document.getElementById('agents-list');
  list.innerHTML = '<li class="hint">loading…</li>';
  try {
    const items = await invoke('list_agents');
    renderDiscoverList(list, items, renderAgentEntry);
    document.getElementById('agents-count').textContent = `· ${items.length}`;
    updateDiscoverBadge();
  } catch (err) {
    list.innerHTML = '';
    const li = document.createElement('li');
    li.className = 'hint';
    li.textContent = `error: ${err}`;
    list.appendChild(li);
  }
}

async function refreshRecipes() {
  const list = document.getElementById('recipes-list');
  list.innerHTML = '<li class="hint">loading…</li>';
  try {
    const items = await invoke('list_recipes');
    renderDiscoverList(list, items, renderRecipeEntry);
    document.getElementById('recipes-count').textContent = `· ${items.length}`;
    updateDiscoverBadge();
  } catch (err) {
    list.innerHTML = '';
    const li = document.createElement('li');
    li.className = 'hint';
    li.textContent = `error: ${err}`;
    list.appendChild(li);
  }
}

function renderDiscoverList(list, items, renderItem) {
  list.innerHTML = '';
  if (!Array.isArray(items) || items.length === 0) {
    const li = document.createElement('li');
    li.className = 'hint';
    li.textContent = 'none found';
    list.appendChild(li);
    return;
  }
  for (const item of items) {
    list.appendChild(renderItem(item));
  }
}

function renderAgentEntry(a) {
  const li = document.createElement('li');
  li.className = 'entry expandable';
  const head = document.createElement('div');
  head.className = 'entry-head';
  const name = document.createElement('span');
  name.className = 'name';
  name.textContent = a.name || '(unnamed)';
  const tag = document.createElement('span');
  tag.className = `src-tag ${a.source || 'builtin'}`;
  tag.textContent = a.source || 'builtin';
  head.append(name, tag);
  li.appendChild(head);
  if (a.description) {
    const desc = document.createElement('span');
    desc.className = 'desc';
    desc.textContent = a.description;
    li.appendChild(desc);
  }
  const meta = document.createElement('span');
  meta.className = 'meta';
  const toolCount = Array.isArray(a.tools) ? a.tools.length : 0;
  meta.textContent = `model: ${a.model || '(default)'} · ${toolCount} tool${toolCount === 1 ? '' : 's'}${a.has_sandbox_profile ? ' · sandbox' : ''}`;
  li.appendChild(meta);
  const detail = document.createElement('div');
  detail.className = 'entry-detail hidden';
  li.appendChild(detail);
  head.addEventListener('click', () => toggleAgentDetail(li, detail, a.name, a.source));
  return li;
}

async function toggleAgentDetail(li, detail, name, source) {
  if (!detail.classList.contains('hidden')) {
    detail.classList.add('hidden');
    return;
  }
  detail.innerHTML = '<p class="hint">loading…</p>';
  detail.classList.remove('hidden');
  try {
    const full = await invoke('get_agent', { name });
    detail.innerHTML = '';
    if (Array.isArray(full.tools) && full.tools.length > 0) {
      const tools = document.createElement('div');
      tools.className = 'detail-row';
      tools.innerHTML = `<strong>tools:</strong> ${full.tools.map(t => `<code>${t}</code>`).join(' ')}`;
      detail.appendChild(tools);
    }
    const promptHead = document.createElement('strong');
    promptHead.textContent = 'system prompt:';
    detail.appendChild(promptHead);
    const pre = document.createElement('pre');
    pre.className = 'system-prompt';
    pre.textContent = full.system_prompt || '(empty)';
    detail.appendChild(pre);
    if (source !== 'project') {
      const actions = document.createElement('div');
      actions.className = 'actions';
      const fork = document.createElement('button');
      fork.textContent = 'Fork to .hone/agents/';
      fork.addEventListener('click', () => forkAgent(full));
      actions.appendChild(fork);
      detail.appendChild(actions);
    } else {
      const note = document.createElement('p');
      note.className = 'hint';
      note.textContent = 'This agent already lives in your project. Edit the .hone/agents/<name>.md file directly.';
      detail.appendChild(note);
    }
  } catch (err) {
    detail.innerHTML = `<p class="hint">error: ${err}</p>`;
  }
}

async function forkAgent(full) {
  // For v1 the project root is the user's chosen working dir; we use the
  // app's cwd derived from where honed was launched. The Tauri side
  // refuses path-traversal, so prompting the user for an explicit dir is
  // a follow-up.
  const projectRoot = prompt('Fork into project root:', '');
  if (!projectRoot) return;
  try {
    const path = await invoke('agent_fork', { projectRoot, agent: full });
    appendErrorBubble(`Forked to ${path}`);
    refreshAgents();
  } catch (err) {
    appendErrorBubble(`fork failed: ${err}`);
  }
}

function renderRecipeEntry(r) {
  const li = document.createElement('li');
  li.className = 'entry expandable';
  const head = document.createElement('div');
  head.className = 'entry-head';
  const name = document.createElement('span');
  name.className = 'name';
  name.textContent = r.name || '(unnamed)';
  const tag = document.createElement('span');
  tag.className = `src-tag ${r.source || 'builtin'}`;
  tag.textContent = r.source || 'builtin';
  head.append(name, tag);
  li.appendChild(head);
  if (r.description) {
    const desc = document.createElement('span');
    desc.className = 'desc';
    desc.textContent = r.description;
    li.appendChild(desc);
  }
  const meta = document.createElement('span');
  meta.className = 'meta';
  const stepCount = r.step_count || 0;
  meta.textContent = `${stepCount} step${stepCount === 1 ? '' : 's'}${r.has_test_cmd ? ' · test_cmd' : ''}`;
  li.appendChild(meta);
  const detail = document.createElement('div');
  detail.className = 'entry-detail hidden';
  li.appendChild(detail);
  head.addEventListener('click', () => toggleRecipeDetail(li, detail, r.name));
  return li;
}

async function toggleRecipeDetail(li, detail, name) {
  if (!detail.classList.contains('hidden')) {
    detail.classList.add('hidden');
    return;
  }
  detail.innerHTML = '<p class="hint">loading…</p>';
  detail.classList.remove('hidden');
  try {
    const full = await invoke('get_recipe', { name });
    detail.innerHTML = '';
    if (Array.isArray(full.steps) && full.steps.length > 0) {
      const ol = document.createElement('ol');
      ol.className = 'step-list';
      for (const step of full.steps) {
        const li2 = document.createElement('li');
        const sname = document.createElement('strong');
        sname.textContent = step.name;
        li2.appendChild(sname);
        const sprompt = document.createElement('pre');
        sprompt.className = 'step-prompt';
        sprompt.textContent = step.prompt;
        li2.appendChild(sprompt);
        ol.appendChild(li2);
      }
      detail.appendChild(ol);
    }
    if (full.test_cmd) {
      const tc = document.createElement('div');
      tc.className = 'detail-row';
      tc.innerHTML = `<strong>test_cmd:</strong> <code>${full.test_cmd}</code>`;
      detail.appendChild(tc);
    }
    const actions = document.createElement('div');
    actions.className = 'actions';
    const useBtn = document.createElement('button');
    useBtn.textContent = 'Use first step as prompt';
    useBtn.addEventListener('click', () => {
      const first = full.steps && full.steps[0];
      if (first) {
        document.getElementById('chat-input').value = first.prompt;
        document.getElementById('chat-input').focus();
      }
    });
    actions.appendChild(useBtn);
    detail.appendChild(actions);
  } catch (err) {
    detail.innerHTML = `<p class="hint">error: ${err}</p>`;
  }
}

function updateDiscoverBadge() {
  const ids = ['agents-count', 'recipes-count', 'mcp-installed-count', 'mcp-marketplace-count'];
  let total = 0;
  for (const id of ids) {
    total += parseInt(document.getElementById(id).textContent.replace(/\D/g, ''), 10) || 0;
  }
  const badge = document.getElementById('discover-count');
  badge.textContent = String(total);
  badge.classList.toggle('hidden', total === 0);
}

async function refreshMcpInstalled() {
  const list = document.getElementById('mcp-installed-list');
  list.innerHTML = '<li class="hint">loading…</li>';
  try {
    const items = await invoke('list_mcp_installed');
    renderDiscoverList(list, items, renderMcpInstalledEntry);
    document.getElementById('mcp-installed-count').textContent = `· ${items.length}`;
    updateDiscoverBadge();
  } catch (err) {
    list.innerHTML = '';
    const li = document.createElement('li');
    li.className = 'hint';
    li.textContent = `error: ${err}`;
    list.appendChild(li);
  }
}

async function refreshMcpMarketplace() {
  const list = document.getElementById('mcp-marketplace-list');
  list.innerHTML = '<li class="hint">loading…</li>';
  try {
    const items = await invoke('list_mcp_marketplace');
    renderDiscoverList(list, items, renderMcpMarketplaceEntry);
    document.getElementById('mcp-marketplace-count').textContent = `· ${items.length}`;
    updateDiscoverBadge();
  } catch (err) {
    list.innerHTML = '';
    const li = document.createElement('li');
    li.className = 'hint';
    li.textContent = `error: ${err}`;
    list.appendChild(li);
  }
}

function renderMcpInstalledEntry(s) {
  const li = document.createElement('li');
  li.className = 'entry';
  const head = document.createElement('div');
  head.className = 'entry-head';
  const name = document.createElement('span');
  name.className = 'name';
  name.textContent = s.name || '(unnamed)';
  const tag = document.createElement('span');
  tag.className = `src-tag ${s.source || 'global'}`;
  tag.textContent = s.source || 'global';
  head.append(name, tag);
  li.appendChild(head);
  const meta = document.createElement('span');
  meta.className = 'meta';
  meta.textContent = `${s.command || ''} ${(s.args || []).join(' ')}${s.disabled ? ' · disabled' : ''}`;
  li.appendChild(meta);
  return li;
}

function renderMcpMarketplaceEntry(e) {
  const li = document.createElement('li');
  li.className = 'entry expandable';
  const head = document.createElement('div');
  head.className = 'entry-head';
  const name = document.createElement('span');
  name.className = 'name';
  name.textContent = e.name || e.id || '(unnamed)';
  if (e.official) {
    const tag = document.createElement('span');
    tag.className = 'src-tag project'; // green = official
    tag.textContent = 'official';
    head.append(name, tag);
  } else {
    const tag = document.createElement('span');
    tag.className = 'src-tag';
    tag.textContent = 'community';
    head.append(name, tag);
  }
  li.appendChild(head);
  if (e.description) {
    const desc = document.createElement('span');
    desc.className = 'desc';
    desc.textContent = e.description;
    li.appendChild(desc);
  }
  const detail = document.createElement('div');
  detail.className = 'entry-detail hidden';
  li.appendChild(detail);
  head.addEventListener('click', () => toggleMcpMarketplaceDetail(detail, e));
  return li;
}

function toggleMcpMarketplaceDetail(detail, entry) {
  if (!detail.classList.contains('hidden')) {
    detail.classList.add('hidden');
    return;
  }
  detail.innerHTML = '';
  detail.classList.remove('hidden');
  const cmd = document.createElement('div');
  cmd.className = 'detail-row';
  cmd.innerHTML = `<strong>command:</strong> <code>${entry.command} ${(entry.args || []).join(' ')}</code>`;
  detail.appendChild(cmd);
  if (entry.install_hint) {
    const hint = document.createElement('p');
    hint.className = 'hint';
    hint.textContent = entry.install_hint;
    detail.appendChild(hint);
  }
  if (entry.source_url) {
    const link = document.createElement('p');
    link.className = 'hint';
    const a = document.createElement('a');
    a.href = entry.source_url;
    a.textContent = entry.source_url;
    a.target = '_blank';
    a.rel = 'noopener';
    link.appendChild(a);
    detail.appendChild(link);
  }
  const actions = document.createElement('div');
  actions.className = 'actions';
  const installBtn = document.createElement('button');
  installBtn.textContent = `Install '${entry.id}' to .hone/mcp.json`;
  installBtn.addEventListener('click', () => installMcpEntry(entry));
  actions.appendChild(installBtn);
  detail.appendChild(actions);
}

// ---------------------------------------------------------------------------
// Sandbox policy viewer
// ---------------------------------------------------------------------------

async function refreshSandbox() {
  const activeRoot = document.getElementById('sandbox-active');
  const overridesRoot = document.getElementById('sandbox-overrides');
  activeRoot.innerHTML = '<p class="hint">loading…</p>';
  overridesRoot.innerHTML = '';
  try {
    const data = await invoke('list_sandbox_policies');
    renderSandboxActive(activeRoot, data.active || {});
    renderSandboxOverrides(overridesRoot, data.agent_overrides || []);
    const badge = document.getElementById('sandbox-count');
    badge.textContent = String((data.agent_overrides || []).length);
    badge.classList.toggle('hidden', (data.agent_overrides || []).length === 0);
  } catch (err) {
    activeRoot.innerHTML = `<p class="hint">error: ${err}</p>`;
  }
}

function renderSandboxActive(root, active) {
  root.innerHTML = '';
  const head = document.createElement('div');
  const label = document.createElement('strong');
  label.textContent = 'Active server policy';
  head.appendChild(label);
  const pill = document.createElement('span');
  pill.className = `mode-pill ${active.mode || 'manual'}`;
  pill.textContent = active.mode || '?';
  head.appendChild(pill);
  root.appendChild(head);
  if (active.description) {
    const desc = document.createElement('p');
    desc.className = 'desc';
    desc.textContent = active.description;
    root.appendChild(desc);
  }
  const dl = document.createElement('dl');
  dl.className = 'rules';
  const rows = [
    ['network', active.allow_network ? 'on' : 'off'],
    ['allow_read', listOrEmpty(active.allow_read)],
    ['allow_write', listOrEmpty(active.allow_write)],
    ['allow_exec', listOrEmpty(active.allow_exec)],
    ['allowed_domains', listOrEmpty(active.allowed_domains)],
    ['allow_env', listOrEmpty(active.allow_env)],
  ];
  for (const [k, v] of rows) {
    const dt = document.createElement('dt');
    dt.textContent = k;
    const dd = document.createElement('dd');
    dd.textContent = v;
    dl.append(dt, dd);
  }
  root.appendChild(dl);
}

function listOrEmpty(arr) {
  if (!Array.isArray(arr) || arr.length === 0) return '(none)';
  return arr.join(', ');
}

function renderSandboxOverrides(root, overrides) {
  root.innerHTML = '';
  if (overrides.length === 0) {
    const hint = document.createElement('p');
    hint.className = 'hint';
    hint.textContent = 'No agent overrides — every agent inherits the active server policy.';
    root.appendChild(hint);
    return;
  }
  const h = document.createElement('h4');
  h.textContent = `Per-agent overrides (${overrides.length})`;
  root.appendChild(h);
  for (const o of overrides) {
    const row = document.createElement('div');
    row.className = 'override-row';
    const name = document.createElement('span');
    name.className = 'name';
    name.textContent = o.agent;
    const meta = document.createElement('span');
    const p = o.policy || {};
    const counts = [
      `mode=${p.mode || '?'}`,
      `r=${(p.allow_read || []).length}`,
      `w=${(p.allow_write || []).length}`,
      `net=${p.allow_network ? 'on' : 'off'}`,
    ].join(' ');
    meta.textContent = counts;
    row.append(name, meta);
    root.appendChild(row);
  }
}

// ---------------------------------------------------------------------------
// Eval-run dashboard
// ---------------------------------------------------------------------------

async function refreshEvals() {
  const list = document.getElementById('evals-list');
  list.innerHTML = '<li class="hint">loading…</li>';
  const tag = document.getElementById('evals-tag').value.trim();
  try {
    const items = await invoke('list_evals', { limit: 50, tag: tag || null });
    list.innerHTML = '';
    if (!Array.isArray(items) || items.length === 0) {
      const li = document.createElement('li');
      li.className = 'hint';
      li.textContent = 'no eval runs found';
      list.appendChild(li);
    } else {
      for (const summary of items) {
        list.appendChild(renderEvalRow(summary));
      }
    }
    const badge = document.getElementById('evals-count');
    badge.textContent = String(Array.isArray(items) ? items.length : 0);
    badge.classList.toggle('hidden', !Array.isArray(items) || items.length === 0);
  } catch (err) {
    list.innerHTML = '';
    const li = document.createElement('li');
    li.className = 'hint';
    li.textContent = String(err).includes('not configured')
      ? 'set HONE_EVAL_HISTORY_DB when launching honed'
      : `error: ${err}`;
    list.appendChild(li);
  }
}

function renderEvalRow(summary) {
  const li = document.createElement('li');
  li.className = 'entry expandable eval-row';
  const head = document.createElement('div');
  head.className = 'entry-head';
  const name = document.createElement('span');
  name.className = 'name';
  name.textContent = summary.model || '(no model)';
  head.appendChild(name);
  if (summary.tag) {
    const tagPill = document.createElement('span');
    tagPill.className = 'src-tag';
    tagPill.textContent = summary.tag;
    head.appendChild(tagPill);
  }
  const passed = summary.passed || 0;
  const total = summary.total || 0;
  const pct = total > 0 ? passed / total : 0;
  const passText = document.createElement('span');
  passText.className = 'pass-text';
  passText.classList.add(pct >= 0.9 ? 'green' : (pct >= 0.5 ? 'amber' : 'red'));
  passText.textContent = `${passed}/${total}`;
  head.appendChild(passText);
  const bar = document.createElement('span');
  bar.className = 'pass-bar';
  const fill = document.createElement('span');
  fill.style.width = `${(pct * 100).toFixed(0)}%`;
  bar.appendChild(fill);
  head.appendChild(bar);
  li.appendChild(head);

  const meta = document.createElement('span');
  meta.className = 'meta';
  const ts = summary.timestamp ? new Date(summary.timestamp).toLocaleString() : '?';
  const sha = summary.commit_sha ? ` · ${summary.commit_sha.slice(0, 7)}` : '';
  meta.textContent = `${ts}${sha}`;
  li.appendChild(meta);

  const detail = document.createElement('div');
  detail.className = 'entry-detail hidden';
  li.appendChild(detail);
  head.addEventListener('click', () => toggleEvalDetail(detail, summary.id));
  return li;
}

async function toggleEvalDetail(detail, id) {
  if (!detail.classList.contains('hidden')) {
    detail.classList.add('hidden');
    return;
  }
  detail.innerHTML = '<p class="hint">loading…</p>';
  detail.classList.remove('hidden');
  try {
    const record = await invoke('get_eval', { id });
    detail.innerHTML = '';
    const head = document.createElement('div');
    head.className = 'detail-row';
    head.innerHTML = `<strong>config:</strong> <code>${(record.config_hash || '').slice(0, 16)}</code>`;
    detail.appendChild(head);

    const outcomes = Array.isArray(record.outcomes) ? record.outcomes : [];
    if (outcomes.length === 0) {
      const p = document.createElement('p');
      p.className = 'hint';
      p.textContent = '(no per-task outcomes)';
      detail.appendChild(p);
      return;
    }
    const ul = document.createElement('ul');
    ul.className = 'eval-outcomes';
    let totalCost = 0;
    for (const o of outcomes) {
      totalCost += (o.cost_usd || 0);
      const li2 = document.createElement('li');
      li2.dataset.status = String(o.status || '').toLowerCase();
      const dot = document.createElement('span');
      dot.className = 'status-dot';
      const idSpan = document.createElement('span');
      idSpan.textContent = o.id || '';
      const meta = document.createElement('span');
      meta.className = 'meta';
      meta.textContent = `${o.duration_ms || 0}ms · $${(o.cost_usd || 0).toFixed(4)} · ${(o.tokens_input || 0) + (o.tokens_output || 0)} tok`;
      li2.append(dot, idSpan, meta);
      ul.appendChild(li2);
    }
    detail.appendChild(ul);
    const footer = document.createElement('p');
    footer.className = 'hint';
    footer.textContent = `total: $${totalCost.toFixed(4)} across ${outcomes.length} task${outcomes.length === 1 ? '' : 's'}`;
    detail.appendChild(footer);
  } catch (err) {
    detail.innerHTML = `<p class="hint">error: ${err}</p>`;
  }
}

// ---------------------------------------------------------------------------
// Provenance hash-chain explorer
// ---------------------------------------------------------------------------

async function refreshProvenance() {
  const list = document.getElementById('prov-list');
  list.innerHTML = '<li class="hint">loading…</li>';
  try {
    const items = await invoke('list_provenance');
    list.innerHTML = '';
    if (!Array.isArray(items) || items.length === 0) {
      const li = document.createElement('li');
      li.className = 'hint';
      li.textContent = 'no provenance logs found';
      list.appendChild(li);
    } else {
      for (const summary of items) {
        list.appendChild(renderProvEntry(summary));
      }
    }
    const badge = document.getElementById('prov-count');
    badge.textContent = String(Array.isArray(items) ? items.length : 0);
    badge.classList.toggle('hidden', !Array.isArray(items) || items.length === 0);
  } catch (err) {
    list.innerHTML = '';
    const li = document.createElement('li');
    li.className = 'hint';
    li.textContent = String(err).includes('not configured')
      ? 'set HONE_PROVENANCE_DIR when launching honed'
      : `error: ${err}`;
    list.appendChild(li);
  }
}

function renderProvEntry(summary) {
  const li = document.createElement('li');
  li.className = 'entry expandable';
  const head = document.createElement('div');
  head.className = 'entry-head';
  const name = document.createElement('span');
  name.className = 'name';
  name.textContent = summary.id || '(unnamed)';
  head.appendChild(name);
  const meta = document.createElement('span');
  meta.className = 'src-tag';
  meta.textContent = `${summary.size_bytes || 0} B`;
  head.appendChild(meta);
  li.appendChild(head);

  const detail = document.createElement('div');
  detail.className = 'entry-detail hidden';
  li.appendChild(detail);

  head.addEventListener('click', () => toggleProvDetail(detail, summary.id));
  return li;
}

async function toggleProvDetail(detail, id) {
  if (!detail.classList.contains('hidden')) {
    detail.classList.add('hidden');
    return;
  }
  detail.innerHTML = '<p class="hint">verifying…</p>';
  detail.classList.remove('hidden');
  try {
    const [verify, entries] = await Promise.all([
      invoke('verify_provenance', { id }),
      invoke('get_provenance_entries', { id }),
    ]);
    detail.innerHTML = '';
    const line = document.createElement('div');
    line.className = `prov-verify-line ${verify.valid ? 'valid' : 'invalid'}`;
    if (verify.valid) {
      line.textContent = `✓ chain valid · ${verify.entry_count} entries`;
    } else {
      line.textContent = `✗ chain invalid${verify.broken_at != null ? ` at index ${verify.broken_at}` : ''}: ${verify.reason || ''}`;
    }
    const merkle = document.createElement('span');
    merkle.className = 'merkle';
    merkle.textContent = `merkle: ${verify.merkle_root || '(none)'}`;
    line.appendChild(merkle);
    detail.appendChild(line);

    if (Array.isArray(entries) && entries.length > 0) {
      const ul = document.createElement('ul');
      ul.className = 'prov-entries';
      for (const e of entries.slice(0, 200)) {
        const li2 = document.createElement('li');
        const idx = document.createElement('span');
        idx.className = 'idx';
        idx.textContent = `#${e.index ?? '?'}`;
        const kind = document.createElement('span');
        kind.className = 'ev-kind';
        kind.textContent = (e.event && e.event.kind) || '?';
        li2.append(idx, kind);
        const summary = describeProvEvent(e.event);
        if (summary) {
          li2.append(' ', document.createTextNode(summary));
        }
        ul.appendChild(li2);
      }
      detail.appendChild(ul);
      if (entries.length > 200) {
        const note = document.createElement('p');
        note.className = 'hint';
        note.textContent = `(truncated; showing first 200 of ${entries.length})`;
        detail.appendChild(note);
      }
    }
  } catch (err) {
    detail.innerHTML = `<p class="hint">error: ${err}</p>`;
  }
}

function describeProvEvent(ev) {
  if (!ev || typeof ev !== 'object') return '';
  switch (ev.kind) {
    case 'user_prompt':
      return (ev.text || '').slice(0, 80);
    case 'model_completion':
      return `${ev.model || '?'}: ${(ev.output || '').slice(0, 60)}`;
    case 'tool_call':
      return ev.name || '';
    case 'tool_result':
      return `${ev.name || '?'} ${ev.is_error ? '(err)' : ''}`;
    case 'session_end':
      return ev.reason || '';
    default:
      return '';
  }
}

// ---------------------------------------------------------------------------
// Memory viewer
// ---------------------------------------------------------------------------

async function refreshMemory() {
  const statsRoot = document.getElementById('memory-stats');
  const list = document.getElementById('memory-list');
  const q = document.getElementById('memory-q').value.trim();
  statsRoot.innerHTML = '<span>loading…</span>';
  list.innerHTML = '';

  try {
    const [stats, items] = await Promise.all([
      invoke('memory_stats').catch((e) => ({ error: String(e) })),
      invoke('list_memory', { limit: 200, q: q || null }).catch((e) => ({ error: String(e) })),
    ]);
    if (stats && stats.error) {
      statsRoot.textContent = stats.error.includes('not configured')
        ? 'memory store not configured (set HONE_MEMORY_DB)'
        : `error: ${stats.error}`;
      const badge = document.getElementById('memory-count');
      badge.classList.add('hidden');
      return;
    }
    renderMemoryStats(statsRoot, stats);
    if (Array.isArray(items)) {
      renderMemoryList(list, items);
      const badge = document.getElementById('memory-count');
      badge.textContent = String(items.length);
      badge.classList.toggle('hidden', items.length === 0);
    } else if (items && items.error) {
      list.innerHTML = '';
      const li = document.createElement('li');
      li.className = 'mem-row';
      li.textContent = items.error;
      list.appendChild(li);
    }
  } catch (err) {
    statsRoot.textContent = `error: ${err}`;
  }
}

function renderMemoryStats(root, stats) {
  root.innerHTML = '';
  const total = document.createElement('div');
  total.textContent = `total: ${stats.total || 0}`;
  root.appendChild(total);
  if (stats.by_kind && Object.keys(stats.by_kind).length > 0) {
    const breakdown = Object.entries(stats.by_kind)
      .map(([k, v]) => `${k}=${v}`)
      .join(' · ');
    const div = document.createElement('div');
    div.textContent = breakdown;
    root.appendChild(div);
  }
  if (Array.isArray(stats.top_tags) && stats.top_tags.length > 0) {
    const tagsDiv = document.createElement('div');
    tagsDiv.className = 'top-tags';
    for (const [name, count] of stats.top_tags.slice(0, 12)) {
      const chip = document.createElement('span');
      chip.className = 'tag-chip';
      chip.textContent = `${name} (${count})`;
      tagsDiv.appendChild(chip);
    }
    root.appendChild(tagsDiv);
  }
}

function renderMemoryList(list, items) {
  list.innerHTML = '';
  if (items.length === 0) {
    const li = document.createElement('li');
    li.className = 'mem-row';
    li.textContent = '(no memories)';
    list.appendChild(li);
    return;
  }
  for (const m of items) {
    const li = document.createElement('li');
    li.className = 'mem-row';

    const head = document.createElement('div');
    head.className = 'head';
    const idSpan = document.createElement('span');
    idSpan.className = 'kind-pill';
    idSpan.classList.add(m.kind || 'unknown');
    idSpan.textContent = m.kind || '?';
    const idText = document.createElement('span');
    idText.className = 'meta';
    idText.textContent = (m.id || '').slice(0, 12);
    head.append(idSpan, idText);
    li.appendChild(head);

    const content = document.createElement('div');
    content.className = 'content';
    content.textContent = m.content || '(empty)';
    li.appendChild(content);

    const meta = document.createElement('span');
    meta.className = 'meta';
    const tags = (m.tags || []).slice(0, 5).join(', ');
    const w = (typeof m.weight === 'number') ? m.weight.toFixed(2) : '?';
    meta.textContent = `weight ${w}${tags ? ` · ${tags}` : ''}${m.has_embedding ? ' · embedded' : ''}`;
    li.appendChild(meta);

    list.appendChild(li);
  }
}

async function installMcpEntry(entry) {
  const projectRoot = prompt('Install into project root:', '');
  if (!projectRoot) return;
  try {
    const path = await invoke('mcp_install', { projectRoot, entry });
    appendErrorBubble(`Installed '${entry.id}' to ${path}`);
    refreshMcpInstalled();
  } catch (err) {
    appendErrorBubble(`mcp_install failed: ${err}`);
  }
}

// ---------------------------------------------------------------------------
// Task-tree (orchestrator viz)
// ---------------------------------------------------------------------------

function renderTaskTree() {
  const root = document.getElementById('task-tree');
  const empty = document.getElementById('task-empty');
  const badge = document.getElementById('task-count');

  // Wipe everything except the empty hint.
  Array.from(root.querySelectorAll('.plan, .task-node, .budget')).forEach((el) => el.remove());

  const tasks = Array.from(chatState.workers.entries());
  badge.textContent = String(tasks.length);
  badge.classList.toggle('hidden', tasks.length === 0 && !chatState.plan);
  empty.style.display = (tasks.length === 0 && !chatState.plan) ? 'block' : 'none';

  if (chatState.plan) {
    const plan = document.createElement('div');
    plan.className = 'plan';
    const goal = document.createElement('span');
    goal.innerHTML = '<strong>Goal:</strong> ';
    goal.append(document.createTextNode(chatState.plan.goal));
    plan.appendChild(goal);
    plan.appendChild(document.createElement('br'));
    plan.append(`${chatState.plan.subtask_count} subtask${chatState.plan.subtask_count === 1 ? '' : 's'}`);
    root.appendChild(plan);
  }

  for (const [taskId, w] of tasks) {
    const node = document.createElement('div');
    node.className = 'task-node';
    node.dataset.status = w.status;
    const icon = document.createElement('span');
    icon.className = 'task-icon';
    const body = document.createElement('div');
    const title = document.createElement('div');
    const agentSpan = document.createElement('span');
    agentSpan.className = 'task-agent';
    agentSpan.textContent = w.agent;
    title.append(agentSpan, ` · ${taskId}`);
    body.appendChild(title);
    if (w.description) {
      const desc = document.createElement('div');
      desc.textContent = w.description;
      body.appendChild(desc);
    }
    if (w.status !== 'queued') {
      const meta = document.createElement('span');
      meta.className = 'task-meta';
      meta.textContent = `${w.status}${w.cost_usd ? ` · $${w.cost_usd.toFixed(4)}` : ''}`;
      body.appendChild(meta);
    }
    node.append(icon, body);
    root.appendChild(node);
  }

  if (chatState.budget) {
    const b = document.createElement('div');
    b.className = 'budget';
    const cap = chatState.budget.budget_usd != null
      ? ` / $${chatState.budget.budget_usd.toFixed(2)}`
      : '';
    b.textContent = `Spent $${(chatState.budget.spent_usd || 0).toFixed(4)}${cap} · ${chatState.budget.workers_completed}/${chatState.budget.workers_total} workers`;
    root.appendChild(b);
  }
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
  // Reset orchestrator state for the new turn.
  chatState.plan = null;
  chatState.workers.clear();
  chatState.budget = null;
  renderTaskTree();

  const orchestrate = document.getElementById('orchestrate-toggle').checked;
  const stepMode = document.getElementById('step-mode-toggle').checked;

  if (orchestrate) {
    // Orchestrate path: the daemon mints its own correlation key. We
    // assign a Tauri-side run_id once spawn returns it.
    const body = { prompt, history: chatState.history.slice(0, -1) };
    try {
      const runId = await invoke('run_orchestrate', { request: body });
      chatState.activeRunId = runId;
    } catch (err) {
      appendErrorBubble(`run_orchestrate failed: ${err}`);
      finishRun();
    }
    return;
  }

  // Single-agent /run path. We generate request_id client-side so the
  // GUI and daemon share the same step-mode correlation key.
  const requestId = (typeof crypto !== 'undefined' && crypto.randomUUID)
    ? crypto.randomUUID()
    : `r-${Date.now()}-${Math.random().toString(36).slice(2,10)}`;
  chatState.activeRunId = requestId;

  const requestBody = {
    prompt,
    history: chatState.history.slice(0, -1),
    request_id: requestId,
  };
  if (stepMode) {
    requestBody.approval_policy = { AskRisky: { always_allowed: [] } };
  }

  try {
    await invoke('run_turn', { request: requestBody });
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

  // Discover pane refresh handlers
  document.getElementById('btn-refresh-agents').addEventListener('click', refreshAgents);
  document.getElementById('btn-refresh-recipes').addEventListener('click', refreshRecipes);
  document.getElementById('btn-refresh-mcp-installed').addEventListener('click', refreshMcpInstalled);
  document.getElementById('btn-refresh-mcp-marketplace').addEventListener('click', refreshMcpMarketplace);
  document.getElementById('btn-refresh-sandbox').addEventListener('click', refreshSandbox);
  document.getElementById('btn-refresh-memory').addEventListener('click', refreshMemory);
  document.getElementById('btn-refresh-prov').addEventListener('click', refreshProvenance);
  document.getElementById('btn-refresh-evals').addEventListener('click', refreshEvals);

  // Multi-daemon switcher
  const switcher = document.getElementById('daemon-switcher');
  switcher.addEventListener('click', (e) => {
    e.stopPropagation();
    const menu = document.getElementById('daemon-menu');
    if (menu.classList.contains('hidden')) openDaemonMenu(); else closeDaemonMenu();
  });
  document.getElementById('daemon-menu').addEventListener('click', (e) => e.stopPropagation());
  document.addEventListener('click', (e) => {
    const menu = document.getElementById('daemon-menu');
    if (!menu.classList.contains('hidden') && !menu.contains(e.target) && e.target !== switcher) {
      closeDaemonMenu();
    }
  });
  document.getElementById('daemon-add-form').addEventListener('submit', async (e) => {
    e.preventDefault();
    const name = document.getElementById('daemon-add-name').value.trim();
    const url = document.getElementById('daemon-add-url').value.trim();
    const token = document.getElementById('daemon-add-token').value.trim() || null;
    if (!name || !url) return;
    try {
      await invoke('add_daemon', { name, url, token });
      document.getElementById('daemon-add-name').value = '';
      document.getElementById('daemon-add-url').value = '';
      document.getElementById('daemon-add-token').value = '';
      openDaemonMenu();
    } catch (err) {
      appendErrorBubble(`add_daemon failed: ${err}`);
    }
  });
  document.getElementById('evals-tag').addEventListener('keydown', (e) => {
    if (e.key === 'Enter') { e.preventDefault(); refreshEvals(); }
  });
  document.getElementById('memory-q').addEventListener('keydown', (e) => {
    if (e.key === 'Enter') { e.preventDefault(); refreshMemory(); }
  });

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
