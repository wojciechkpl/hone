# Hone cheatsheet

<p align="center">
  <img alt="flags" src="https://img.shields.io/badge/CLI-flags%20%2B%20subcommands-e8445a">
  <img alt="keys" src="https://img.shields.io/badge/TUI-keys%20%2B%20slash%20commands-6e40c9">
  <img alt="env" src="https://img.shields.io/badge/env-the%20ones%20you%20need-1f6feb">
</p>

Everything on this page is checked against the code on `main`. The complete reference is
[Configuration](../configuration.md).

## Start

```bash
hone                                  # TUI in the current directory
hone "what does this repo do?"        # one-shot
hone --json "list TODOs in src/"      # JSON for scripts and CI
hone -m ollama:qwen2.5-coder:14b      # pick a model
hone --orchestrate "add OAuth login"  # multi-agent
hone --ensemble 3 "fix flaky test"    # 3 attempts, best one wins
hone --recipe bug-fix                 # built-in workflow
hone spec "add rate limiting"         # requirements + design + tasks
```

## Providers

| Use | Set |
|---|---|
| Anthropic | `ANTHROPIC_API_KEY=sk-ant-...` |
| OpenAI | `OPENAI_API_KEY=sk-...` |
| OpenAI-compatible | `OPENAI_API_KEY` + `HONE_OPENAI_BASE_URL=https://api.deepseek.com` + `HONE_OPENAI_MODEL=deepseek-chat` |
| Ollama | `ollama serve`, `ollama pull llama3.2` |
| Claude / Gemini CLI | `claude` or `gemini` on `PATH` |
| Wizard | `hone configure` |

Detection order: Anthropic → OpenAI → `claude` → `gemini` → Ollama.
`--model` prefixes: `anthropic:` `openai:` `ollama:` `claude-cli:` `gemini-cli:`.

## CLI flags

| Flag | Does |
|---|---|
| `-m, --model <ID>` | Model, with a provider prefix |
| `--json` | Structured output (one-shot) |
| `--orchestrate` · `--distributed <URL>` | Multi-agent run · through a remote `honed` coordinator |
| `--ensemble <2-10>` | Rank N candidates |
| `--recipe <NAME>` · `--list-recipes` · `--var K=V` | Recipes |
| `--plan <FILE>` | Run a plan saved from plan mode |
| `--image <FILE>` | Attach an image (repeatable) |
| `--approve <RULE_ID>` | Pre-approve a permission rule's `ask` (repeatable) |
| `--session <NAME>` | Save a one-shot run under this name |
| `--ephemeral` | Write no memory, index or session data |
| `--encrypted-memory` | Encrypt memory (needs `HONE_MEMORY_PASSPHRASE`) |
| `--audit` | Log every tool call to `.hone/audit.jsonl` |
| `--no-inject` · `--skeleton-strategy <S>` | Context injection off · `adaptive`, `call-graph`, `hybrid`, `dependency-graph`, `flat` |
| `--no-clarify` | No clarifying questions on ambiguous one-shot prompts |
| `--step` · `--vim` | TUI: ask before every tool (today: orchestrated turns only, [known bug](../permissions-and-sandbox.md#known-issue-tui)) · vim keys |

## Subcommands

| Command | Does |
|---|---|
| `hone configure` | Provider wizard |
| `hone session list` · `resume <n>` · `rename <a> <b>` · `delete <n>` | Saved sessions |
| `hone memory ls [--kind K] [--limit N]` · `show <id>` · `forget --tag T \| --kind K \| --before RFC3339` | Memory |
| `hone trace list` · `show <id>` · `stats` | Traces from the audit log |
| `hone reports list` · `show <id>` · `diff <a> <b>` | Session reports |
| `hone eval tasks.toml [-o DIR]` | Eval run with `report.json` and `report.md` |
| `hone spec "<goal>" [-c DIR]` | Spec documents in `.hone/specs/` |

## TUI keys

| Key | Action |
|---|---|
| <kbd>Enter</kbd> | Send |
| <kbd>Shift</kbd>+<kbd>Enter</kbd> / <kbd>Alt</kbd>+<kbd>Enter</kbd> | New line |
| <kbd>↑</kbd> / <kbd>↓</kbd> | Prompt history |
| <kbd>Ctrl</kbd>+<kbd>F</kbd> | History search |
| <kbd>Ctrl</kbd>+<kbd>K</kbd> | Command palette |
| <kbd>Ctrl</kbd>+<kbd>/</kbd> | Search the conversation (many terminals don't send this key; Hone doesn't enable enhanced keyboard reporting) |
| <kbd>Tab</kbd> | Cycle focus between panes |
| <kbd>j</kbd> / <kbd>k</kbd> (conversation focused) | Move between messages |
| <kbd>Enter</kbd> / <kbd>Space</kbd> on a tool card | Expand or collapse |
| <kbd>PgUp</kbd> / <kbd>PgDn</kbd> | Scroll |
| <kbd>Shift</kbd>+<kbd>Tab</kbd> | Cycle mode: Build → Plan (read-only tools) → AutoApprove |
| <kbd>Ctrl</kbd>+<kbd>A</kbd> | Session anatomy (turn stats) |
| <kbd>Ctrl</kbd>+<kbd>M</kbd> | Prompt mirror (what the model will see). Most terminals send Ctrl+M as Enter, so this often doesn't work |
| <kbd>Ctrl</kbd>+<kbd>O</kbd> | Session switcher: pick one and the TUI exits, printing `hone session resume <name>` |
| <kbd>Ctrl</kbd>+<kbd>R</kbd> | Agent Rail |
| <kbd>F1</kbd> | Help overlay |
| <kbd>Esc</kbd> | Cancel the running turn · clear the input when idle |
| <kbd>Ctrl</kbd>+<kbd>C</kbd> | Quit |

**Approval menu** (shown when a tool call pauses; today only on orchestrated turns, see the
[known bug](../permissions-and-sandbox.md#known-issue-tui)):

| <kbd>y</kbd> / <kbd>Enter</kbd> / <kbd>Space</kbd> | <kbd>a</kbd> | <kbd>s</kbd> / <kbd>Esc</kbd> | <kbd>d</kbd> / <kbd>n</kbd> |
|---|---|---|---|
| run it once | approve for the session | skip this call | deny |

## In the input box

| Input | Does |
|---|---|
| `/help` | Help text |
| `/model <provider:model>` | Change the model from the next turn (same provider) |
| `/cost` | Session cost so far |
| `/budget <usd>` | Cap the session cost |
| `/permit <tool>` | Don't ask again for this tool on the next run |
| `/plan <task>` | Preview the orchestrator's plan, without running workers |
| `/review` | Diff of everything changed this session |
| `/clarify <prompt>` | Ask clarifying questions first |
| `/tdd [cmd]` · `/tdd off` | Watch tests (auto-detected or given command) · stop |
| `/configure` · `/configure model <id>` · `/configure theme <name>` · `/configure vim on\|off` · `/configure budget <usd>\|off` · `/configure provider <name>` (needs a restart) | Settings inside the TUI |
| `/theme crimson\|neon\|chalk\|minimal` · `/theme-preview` | Theme |
| `/split` · `/panel` | Split layout · bottom tool panel |
| `/set vim` · `/set novim` | Vim keys |
| `/quit` · `/q` | Quit |
| `/<name>` | Your command from `.hone/commands/<name>.md` |
| `@<agent> <message>` | Send this message to a persona, e.g. `@security-auditor review auth.rs` |
| `!<command>` | Run a shell command yourself; `!cd dir` changes directory; `!vim file` suspends the TUI |

## Personas and recipes

| Personas (`@name`) | |
|---|---|
| `api-designer` · `code-reviewer` · `data-engineer` · `debugger` | `documentation-agent` · `ml-engineer` · `performance-optimizer` · `python-expert` |
| `researcher` · `rust-expert` · `security-auditor` · `tdd-workflow` | add yours in `~/.config/hone/agents/` or `.hone/agents/` |

| Recipe | Purpose |
|---|---|
| `tdd-feature` | Feature with test-driven development |
| `bug-fix` | Bug fix with a regression test |
| `code-review` | Review for correctness, security and performance |
| `refactor` | Refactor with tests at each step |
| `security-audit` | OWASP Top 10 audit |
| `performance` | Profile, analyse, optimise |
| `migration` | Move between frameworks, libraries or API versions |
| `documentation` | Generate or update docs |
| `onboarding` | Map an unfamiliar codebase |

## Guardrails

```toml
# ~/.config/hone/permissions.toml   (or <workdir>/.hone/permissions.toml, tighten-only)
[[rules]]
id = "no-push"
command = "git push*"
decision = "deny"          # allow | ask | deny

[[rules]]
path = "~/.ssh/**"
decision = "deny"

[[rules]]
host = "*.internal.example"
decision = "ask"
```

| Need | Use |
|---|---|
| Run an `ask` rule headless | `hone --approve <id> "..."` |
| Same for `hone-mcp-server` | `HONE_APPROVE=id1,id2` |
| Use Claude/Gemini CLI under a policy | `HONE_ALLOW_UNGATED_PROVIDER=1` |
| Sandbox shell commands on Linux | `HONE_SANDBOX_ENFORCE=1` |
| Let the sandbox read another toolchain | `HONE_SANDBOX_EXTRA_READ=/opt/tool:/srv/sdk` |

## Files

| Path | What |
|---|---|
| `~/.config/hone/config.toml` | Wizard output (`[provider]`) |
| `~/.config/hone/permissions.toml` · `.hone/permissions.toml` | Permission rules |
| `~/.config/hone/mcp.json` · `.hone/mcp.json` | MCP servers |
| `HONE.md` (user, project root, cwd) | Conventions in the system prompt |
| `~/.config/hone/agents/` · `.hone/agents/` | Personas |
| `~/.config/hone/commands/` · `.hone/commands/` | Slash commands |
| `~/.config/hone/theme.toml` | Colour overrides |
| `~/.local/share/hone/` | `memory.db`, `sessions.db`, `index.db`, `tool-cache.db` |
| `.hone/` in the project | `audit.jsonl`, `reports/`, `plan.md`, `plans/`, `specs/` |

## Daemon

```bash
HONE_BIND=127.0.0.1 honed                         # loopback, no token
HONE_AUTH_TOKEN=secret honed 3117                 # any interface, token required
curl http://127.0.0.1:3117/health
open http://127.0.0.1:3117/ui                     # after `make webui` and a rebuild
HONE_AUTH_TOKEN=secret HONE_DISTRIBUTED_DB=queue.db honed               # coordinator
HONE_AUTH_TOKEN=secret HONE_WORKER_COORDINATOR=http://host:3117 honed   # worker
HONE_AUTH_TOKEN=secret hone --orchestrate --distributed http://host:3117 "…"
```

`honed` exits with code 2 if it would bind a non-loopback address without a token.

## Quick fixes

| Symptom | Fix |
|---|---|
| "No AI provider detected" | Set an API key, or `ollama pull llama3.2` |
| Local model overflows its context | `HONE_CONTEXT_WINDOW=32768` |
| `requires approval (rule …)` | `--approve <id>` |
| `command blocked (requires approval)` | Split chained commands (pipes into `head`, `grep`, `wc`, ... are fine); avoid `curl`, `rm -rf` |
| `command blocked: privilege escalation …` | `sudo`, `su`, `doas` and `pkexec` are never allowed |
| `honed: refusing to start` | Set `HONE_AUTH_TOKEN` or `HONE_BIND=127.0.0.1` |
| Memory errors | `--ephemeral`, or move `~/.local/share/hone/memory.db` aside |
| No colour / odd glyphs | `NO_COLOR=1`, `HONE_ASCII_ONLY=1` |
| Agent Rail doesn't show | It needs a terminal at least 100 columns wide |
| Terminal beeps after long turns | Hone rings the bell when a turn takes 10 s or more; there is no setting to turn it off |

More: [Getting started → Troubleshooting](../getting-started.md#9-troubleshooting) ·
[FAQ](faq.md) · [Core concepts](concepts.md)
