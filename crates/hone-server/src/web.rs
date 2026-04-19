use axum::response::Html;

/// Embedded single-page web UI served at `GET /`.
///
/// Vanilla JS, no build step required.  Sends `POST /run` with the prompt
/// and full conversation history, reads the SSE stream, and renders messages
/// token-by-token.
pub async fn serve_ui() -> Html<&'static str> {
    Html(UI_HTML)
}

const UI_HTML: &str = r#"<!DOCTYPE html>
<html lang="en">
<head>
<meta charset="UTF-8">
<meta name="viewport" content="width=device-width, initial-scale=1.0">
<title>Hone</title>
<style>
*{box-sizing:border-box;margin:0;padding:0}
:root{
  --bg:#0d1117;--bg2:#161b22;--bg3:#21262d;
  --border:#30363d;--text:#e6edf3;--muted:#8b949e;
  --accent:#ff6b6b;--accent2:#ff8e53;
  --user-bg:#1d3557;--user-border:#2d5a8e;
  --code-bg:#0d1117;--error-bg:#2d0f0f;--error-border:#6b1a1a;
}
body{background:var(--bg);color:var(--text);font-family:'Segoe UI',system-ui,sans-serif;height:100dvh;display:flex;flex-direction:column;overflow:hidden}
header{background:var(--bg2);border-bottom:1px solid var(--border);padding:10px 20px;display:flex;align-items:center;gap:12px;flex-shrink:0}
.logo{font-size:15px;font-weight:700;background:linear-gradient(135deg,var(--accent),var(--accent2));-webkit-background-clip:text;-webkit-text-fill-color:transparent}
select{background:var(--bg3);color:var(--text);border:1px solid var(--border);border-radius:6px;padding:4px 8px;font-size:12px;cursor:pointer}
select:focus{outline:none;border-color:var(--accent)}
#wd-input{background:var(--bg3);color:var(--muted);border:1px solid var(--border);border-radius:6px;padding:4px 10px;font-size:12px;width:260px;font-family:monospace}
#wd-input:focus{outline:none;color:var(--text);border-color:var(--accent)}
#status{font-size:12px;color:var(--muted);margin-left:auto}
#messages{flex:1;overflow-y:auto;padding:20px;display:flex;flex-direction:column;gap:16px;scroll-behavior:smooth}
#messages:empty::after{content:"Ask Hone anything about your code.";color:var(--muted);font-size:14px;text-align:center;margin-top:80px;display:block}
.msg{display:flex;gap:10px;max-width:860px;width:100%;margin:0 auto}
.msg.user{flex-direction:row-reverse}
.avatar{width:28px;height:28px;border-radius:50%;display:flex;align-items:center;justify-content:center;font-size:11px;font-weight:700;flex-shrink:0;margin-top:2px}
.msg.user .avatar{background:var(--user-bg);color:var(--text);border:1px solid var(--user-border)}
.msg.assistant .avatar{background:linear-gradient(135deg,var(--accent),var(--accent2));color:#fff}
.bubble{background:var(--bg2);border:1px solid var(--border);border-radius:12px;padding:12px 16px;font-size:14px;line-height:1.65;min-width:40px;max-width:calc(100% - 40px);word-break:break-word;overflow-wrap:anywhere}
.msg.user .bubble{background:var(--user-bg);border-color:var(--user-border)}
.bubble.error{background:var(--error-bg);border-color:var(--error-border);color:#ff8080}
.text-seg{white-space:pre-wrap}
.tool-card{background:var(--bg3);border:1px solid var(--border);border-radius:8px;padding:8px 12px;margin:6px 0;font-size:12px;font-family:monospace}
.tool-name{color:var(--accent);font-weight:700;margin-bottom:4px}
.tool-result{color:var(--muted);white-space:pre-wrap;max-height:120px;overflow:hidden;text-overflow:ellipsis}
.tool-result.error{color:#ff8080}
code{background:var(--code-bg);border-radius:4px;padding:1px 5px;font-family:'JetBrains Mono','Fira Code',monospace;font-size:92%}
pre{background:var(--code-bg);border:1px solid var(--border);border-radius:8px;padding:12px;overflow-x:auto;margin:6px 0}
pre code{background:none;padding:0;font-size:13px}
strong{font-weight:600}
em{font-style:italic}
.cursor::after{content:'▊';animation:blink 1s step-end infinite;color:var(--accent);font-size:13px}
@keyframes blink{50%{opacity:0}}
#input-area{background:var(--bg2);border-top:1px solid var(--border);padding:14px 20px;flex-shrink:0}
.input-row{max-width:860px;margin:0 auto;display:flex;gap:8px;align-items:flex-end}
textarea{flex:1;background:var(--bg3);color:var(--text);border:1px solid var(--border);border-radius:10px;padding:10px 14px;font-size:14px;resize:none;min-height:44px;max-height:180px;line-height:1.5;outline:none;font-family:inherit}
textarea:focus{border-color:var(--accent)}
textarea::placeholder{color:var(--muted)}
#send-btn{background:linear-gradient(135deg,var(--accent),var(--accent2));color:#fff;border:none;border-radius:10px;padding:0 18px;font-size:14px;cursor:pointer;font-weight:600;height:44px;flex-shrink:0;transition:filter .15s}
#send-btn:hover:not(:disabled){filter:brightness(1.1)}
#send-btn:disabled{opacity:.45;cursor:not-allowed}
</style>
</head>
<body>
<header>
  <span class="logo">⬡ Hone</span>
  <select id="model">
    <option value="claude-sonnet-4-6">claude-sonnet-4-6</option>
    <option value="claude-opus-4-7">claude-opus-4-7</option>
    <option value="claude-haiku-4-5-20251001">claude-haiku-4-5</option>
  </select>
  <input id="wd-input" type="text" placeholder="Working dir (leave blank for server default)" title="Working directory passed to file tools">
  <span id="status"></span>
</header>
<div id="messages"></div>
<div id="input-area">
  <div class="input-row">
    <textarea id="prompt" placeholder="Ask Hone… (Enter to send, Shift+Enter for newline)" rows="1"></textarea>
    <button id="send-btn">Send</button>
  </div>
</div>
<script>
const msgsEl = document.getElementById('messages');
const promptEl = document.getElementById('prompt');
const sendBtn = document.getElementById('send-btn');
const modelEl = document.getElementById('model');
const wdEl = document.getElementById('wd-input');
const statusEl = document.getElementById('status');

// Client-side conversation history (user+assistant pairs).
let history = [];
let busy = false;

// ── Markdown rendering (minimal, no deps) ──────────────────────────────────

function esc(s) {
  return s.replace(/&/g,'&amp;').replace(/</g,'&lt;').replace(/>/g,'&gt;');
}

function renderMd(text) {
  const escaped = esc(text);
  return escaped
    // fenced code blocks
    .replace(/```(\w*)\n([\s\S]*?)```/g, (_, lang, code) =>
      `<pre><code class="${lang}">${code.trimEnd()}</code></pre>`)
    // inline code
    .replace(/`([^`\n]+)`/g, (_, c) => `<code>${c}</code>`)
    // bold
    .replace(/\*\*([^*\n]+)\*\*/g, '<strong>$1</strong>')
    // italic
    .replace(/\*([^*\n]+)\*/g, '<em>$1</em>')
    // keep newlines
    .replace(/\n/g, '<br>');
}

// ── DOM helpers ─────────────────────────────────────────────────────────────

function addUserMessage(text) {
  const div = document.createElement('div');
  div.className = 'msg user';
  div.innerHTML = `<div class="avatar">U</div><div class="bubble"><div class="text-seg">${esc(text)}</div></div>`;
  msgsEl.appendChild(div);
  scrollToBottom();
}

function createAssistantBubble() {
  const div = document.createElement('div');
  div.className = 'msg assistant';
  div.innerHTML = `<div class="avatar">H</div><div class="bubble"><div class="text-seg cursor"></div></div>`;
  msgsEl.appendChild(div);
  scrollToBottom();
  return div.querySelector('.bubble');
}

function scrollToBottom() {
  msgsEl.scrollTop = msgsEl.scrollHeight;
}

// ── SSE event handling ───────────────────────────────────────────────────────

let currentBubble = null;
let currentTextSeg = null;
let currentToolCards = {};
let streamedText = '';

function handleEvent(type, payload) {
  switch (type) {
    case 'token': {
      const t = payload.text || '';
      if (!t) break;
      streamedText += t;
      if (!currentTextSeg) {
        // start a new text segment (after a tool call block)
        currentTextSeg = document.createElement('div');
        currentTextSeg.className = 'text-seg cursor';
        currentBubble.appendChild(currentTextSeg);
      }
      currentTextSeg.textContent = streamedText.slice(
        streamedText.length - getTextSegOffset()
      );
      scrollToBottom();
      break;
    }
    case 'tool_call_start': {
      statusEl.textContent = `⚙ ${payload.name}…`;
      // Close current text segment before showing tool card.
      if (currentTextSeg) {
        currentTextSeg.classList.remove('cursor');
      }
      currentTextSeg = null;
      const card = document.createElement('div');
      card.className = 'tool-card';
      card.innerHTML = `<div class="tool-name">⚙ ${esc(payload.name)}</div>`;
      currentBubble.appendChild(card);
      currentToolCards[payload.id] = card;
      scrollToBottom();
      break;
    }
    case 'tool_call_end': {
      statusEl.textContent = 'Thinking…';
      const card = currentToolCards[payload.id];
      if (card) {
        const isErr = payload.is_error;
        const raw = typeof payload.result === 'string'
          ? payload.result
          : JSON.stringify(payload.result, null, 2);
        const preview = raw.length > 300 ? raw.slice(0, 300) + '\n…' : raw;
        const resultDiv = document.createElement('div');
        resultDiv.className = 'tool-result' + (isErr ? ' error' : '');
        resultDiv.textContent = (isErr ? '✗ ' : '✓ ') + preview;
        card.appendChild(resultDiv);
        scrollToBottom();
      }
      break;
    }
    case 'done': {
      if (currentTextSeg) {
        currentTextSeg.classList.remove('cursor');
        // Re-render the final text as markdown.
        currentTextSeg.innerHTML = renderMd(streamedText.slice(getTextSegOffset()));
      }
      const usage = payload.usage;
      statusEl.textContent = usage
        ? `${(usage.input + usage.output).toLocaleString()} tokens`
        : '';
      history.push({ role: 'assistant', content: streamedText });
      streamedText = '';
      currentBubble = null;
      currentTextSeg = null;
      currentToolCards = {};
      busy = false;
      sendBtn.disabled = false;
      promptEl.focus();
      break;
    }
    case 'error': {
      if (currentBubble) {
        currentBubble.classList.add('error');
        currentBubble.textContent = payload.message || 'Unknown error';
      }
      statusEl.textContent = 'Error';
      busy = false;
      sendBtn.disabled = false;
      break;
    }
  }
}

// Track how many chars of streamedText belong to the current text segment
// (chars before a tool call belong to a previous segment).
let _textSegStart = 0;
function getTextSegOffset() { return _textSegStart; }

// Reset when a new text segment starts.
function resetTextSeg() { _textSegStart = streamedText.length; }

// Patch the token handler to track segment start offset.
const _origHandleEvent = handleEvent;
function handleEvent2(type, payload) {
  if (type === 'tool_call_start') {
    resetTextSeg();
  }
  _origHandleEvent(type, payload);
}

// ── SSE stream parsing ───────────────────────────────────────────────────────

async function runStream(prompt) {
  const body = {
    prompt,
    model: modelEl.value,
    history: [...history],
    working_dir: wdEl.value.trim() || undefined,
  };

  const resp = await fetch('/run', {
    method: 'POST',
    headers: { 'Content-Type': 'application/json' },
    body: JSON.stringify(body),
  });

  if (!resp.ok) {
    throw new Error(`HTTP ${resp.status}: ${await resp.text()}`);
  }

  const reader = resp.body.getReader();
  const decoder = new TextDecoder();
  let buf = '';
  let evType = '', evData = '';

  while (true) {
    const { done, value } = await reader.read();
    if (done) break;
    buf += decoder.decode(value, { stream: true });
    const lines = buf.split('\n');
    buf = lines.pop() ?? '';

    for (const line of lines) {
      if (line.startsWith('event: ')) {
        evType = line.slice(7).trim();
      } else if (line.startsWith('data: ')) {
        evData = line.slice(6);
      } else if (line === '' && evType) {
        try {
          handleEvent2(evType, JSON.parse(evData));
        } catch { /* ignore parse errors */ }
        evType = ''; evData = '';
      }
    }
  }
}

// ── Send action ──────────────────────────────────────────────────────────────

async function send() {
  const prompt = promptEl.value.trim();
  if (!prompt || busy) return;

  busy = true;
  sendBtn.disabled = true;
  promptEl.value = '';
  promptEl.style.height = 'auto';
  statusEl.textContent = 'Thinking…';
  streamedText = '';
  _textSegStart = 0;

  history.push({ role: 'user', content: prompt });
  addUserMessage(prompt);
  currentBubble = createAssistantBubble();
  currentTextSeg = currentBubble.querySelector('.text-seg');
  currentToolCards = {};

  try {
    await runStream(prompt);
  } catch (err) {
    if (currentBubble) {
      currentBubble.classList.add('error');
      currentBubble.textContent = `Network error: ${err.message}`;
    }
    statusEl.textContent = 'Error';
    busy = false;
    sendBtn.disabled = false;
  }
}

// ── Input behaviour ──────────────────────────────────────────────────────────

promptEl.addEventListener('input', () => {
  promptEl.style.height = 'auto';
  promptEl.style.height = Math.min(promptEl.scrollHeight, 180) + 'px';
});

promptEl.addEventListener('keydown', e => {
  if (e.key === 'Enter' && !e.shiftKey) { e.preventDefault(); send(); }
});

sendBtn.addEventListener('click', send);
promptEl.focus();
</script>
</body>
</html>"#;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ui_html_is_non_empty() {
        assert!(!UI_HTML.is_empty());
        assert!(UI_HTML.contains("Hone"));
        assert!(UI_HTML.contains("/run"));
    }
}
