# Debugging and Troubleshooting

When something doesn't work, this guide is the playbook. It's organised by symptom — find your symptom, read the diagnosis, apply the fix.

## First five minutes

Before deep-diving, run these:

```bash
hone --version          # confirm you're on the build you think
hone strategies --check # parse check of config.toml: exits 2 on a TOML error, an unknown key or a bad [context]/[memory] value
```

`--check` doesn't resolve the config (a `default_model` with an unknown prefix still passes) and doesn't read `permissions.toml`. Hone has no all-in-one environment check. If the config parses, the issue is more specific — read on. Hone writes no log files; its logs go to stderr (see [Logging and observability](#logging-and-observability)).

## Provider issues

### "API key not found"

```bash
echo $ANTHROPIC_API_KEY     # is it set?
```

Fix:

```bash
export ANTHROPIC_API_KEY="sk-ant-..."
# or persist:
echo 'export ANTHROPIC_API_KEY="sk-ant-..."' >> ~/.zshrc
```

Or run `hone configure`, which saves the key in `~/.config/hone/config.toml` (in plain text). If your key is in a variable with another name, point Hone at it in that file:

```toml
[providers.anthropic]
api_key_env = "MY_ANTHROPIC_KEY"
```

When the variable `api_key_env` names isn't set, Hone doesn't fall back to `ANTHROPIC_API_KEY`.

### "rate limit" / "429"

Hone retries with exponential backoff (250 ms → 500 ms → 1 s → 2 s, 4 attempts). If it still fails:

1. Check your account's rate limit at the provider's dashboard.
2. List fallback models at the top level of your user file, `~/.config/hone/config.toml` (a project file's list is ignored). When the turn's model fails before answering, a rate limit included, Hone tries the next one. Runs with `--model` don't fall back:
   ```toml
   fallback_models = ["openai:gpt-4o", "ollama:qwen2.5-coder"]
   ```
3. Make fewer calls at once. There is no concurrency flag: `--orchestrate` runs up to 4 workers at a time and `--ensemble N` runs N candidates, so avoid them while you're rate-limited.

### "model not found"

Hone has no command that lists models. Check the ID against the provider's own list (`ollama list` for local models).

Common causes:
- Typo in model ID — anthropic uses `claude-sonnet-4-5`, not `claude-4-sonnet`.
- Provider routes the request to a different endpoint. Check `[providers.<name>].base_url`.
- The model is deprecated upstream. Pin a newer one in `config.toml`.

### "supports_tools: false"

The provider can't round-trip tool calls. `/configure` doesn't show this; with `--orchestrate` or `--ensemble`, Hone warns that the provider does not support tools. ACP providers can't be selected from `hone` (see [ACP](acp.md)), and the Claude and Gemini CLI pass-through models (`claude-cli:`, `gemini-cli:`) never use Hone's tools. For raw HTTP providers, check that the model supports tool/function calling — Ollama models below 7B params often don't.

### Hot-swap doesn't take effect

`/configure provider` only notes the name; changing provider needs a restart. To switch model mid-session use `/model <provider:id>` (an `anthropic:`, `openai:` or `ollama:` model with a key). Typed during a turn, it applies once that turn ends.

## Tool dispatch issues

### Tool never gets called

Run with debug logging:

```bash
RUST_LOG=hone_core::dispatcher=debug,hone_providers=debug hone
```

Look for `tools=` in the request log — if the tools array is empty, hone didn't announce any. Common causes:
- The tool comes from an MCP server that didn't start, or is `disabled`, in `mcp.json`. Hone logs that and skips the server; see [MCP servers](../configuration.md#mcp-servers).
- Tools are registered but the active provider has `supports_tools: false`.
- The model's `system_prompt` says "you have no tools" — check the prompt template.

### Tool times out

There is no global tool timeout setting. The `shell` tool takes a `timeout_ms` argument on each call (default 30 000 ms, at most 300 000 ms), so ask the agent to pass a longer one for a slow command.

For an MCP server, set `timeout_ms` for that server in `mcp.json` (default 10 000 ms, `0` means no timeout):

```json
{
  "mcpServers": {
    "db": {
      "command": "/usr/local/bin/mcp-postgres",
      "timeout_ms": 120000
    }
  }
}
```

### Tool returns `isError: true`

Read the error payload — it's plain text. Three common causes:

1. **Path outside the working directory** — the file tools refuse any path that resolves outside the directory Hone runs in, and no setting changes that. Start Hone from a directory that contains the files. A permission rule can also refuse a path; the message names the rule.
2. **Network refused** — the `shell` tool refuses network commands (`curl`, `wget`, `git fetch`, ...), and on macOS the sandbox gives shell commands no network. Neither can be changed. Use `web_fetch` for HTTP; `host` rules in `permissions.toml` apply to it (see [Permissions](../configuration.md#permissions)).
3. **Schema validation** — the model passed args that don't match the tool's schema. Inspect with `RUST_LOG=hone_core::tool_registry=debug`.

### "tool result was never sent back"

For ACP only. The dispatcher executed the tool but the channel-write failed. Check the stderr of the program that runs the `AcpProvider` for a panic (Hone writes no log files):

```bash
RUST_LOG=debug ./your-app 2> run.log
grep -i panic run.log
```

If the tool itself panicked, fix the tool. If the channel was closed (rare), restart hone — this is a known race surfaced when the agent process dies mid-call; hone now reaps and reports it as `ObservedToolEnd { error }`.

## TUI issues

### Terminal too small

Hone requires 80×24. Below that you see "Terminal too small". Resize the terminal or switch to one-shot mode: `hone "..."`.

### Garbled output / box-drawing characters wrong

```bash
echo $LANG                      # should end in .UTF-8
locale charmap                  # should print UTF-8
```

If not:

```bash
export LANG=en_US.UTF-8
```

Or set `HONE_ASCII_ONLY=1` to switch to plain-ASCII borders.

### Mouse scroll inverted

Older builds had reversed scroll direction. Update to a build after April 2026 (commit `a6a4be4`).

### Theme change has no visible effect

Update — earlier builds had `Theme::neon()` differing from `crimson` in only 2 colors. Modern builds (after April 2026) make `neon` visibly distinct. If you're customising `~/.config/hone/theme.toml`, those overrides survive `/theme` switches.

### Command palette descriptions invisible

Same vintage bug — descriptions used `text_muted` (very low contrast). Fixed in builds after April 2026.

## Session and state issues

### "out of memory" or sluggish input

The conversation history is too long. Clear it:

```
/clear
```

Or compact via the existing summariser:

```
/compact
```

`/compact` keeps the last N turns verbatim and replaces older turns with an LLM-generated summary, freeing context.

### Sessions disappear

One-shot runs are saved in `~/.local/share/hone/sessions.db`; `honed` uses the same file unless `HONE_SESSION_DB` is set. The TUI doesn't save sessions yet, and `--ephemeral` runs save nothing. List what is saved:

```bash
hone session list
```

If `sessions.db` is gone, it was deleted. Hone has no remote backup.

### Daemon crash / `honed` won't start

```bash
HONE_BIND=127.0.0.1 RUST_LOG=info honed 7777   # runs in the foreground (it never forks); RUST_LOG=info shows its log
```

Read the stderr output. Common causes:
- Port already in use: `lsof -i :7777`.
- Auth required but no token: see `docs/tutorials/server-and-remote.md`.

## Sandbox issues

### "permission denied" inside an agent shell call

The OS sandbox covers commands the `shell` tool starts, not the file tools. On macOS it is always on: a command can write only to the working directory and `/tmp`, can read system directories, toolchain caches and the working directory, and has no network. On Linux it is off unless `HONE_SANDBOX_ENFORCE=1` is set, and then restricts files only. To let commands read another directory, add it as a read-only root before starting Hone:

```bash
export HONE_SANDBOX_EXTRA_READ="$HOME/.nvm:$HOME/datasets"   # colon-separated
hone
```

There is no setting for extra writable paths or for network access, and the `[sandbox]` table in `config.toml` is not applied. File, command and host rules for the tools go in `permissions.toml`. See [OS sandbox](../permissions-and-sandbox.md#3-os-sandbox) and [Permissions](../configuration.md#permissions).

### Shell commands fail under the sandbox on macOS

Every `shell` command runs under `sandbox-exec`. There is no setting to turn that off or to choose another backend, and if `sandbox-exec` can't run the command, the command fails; Hone doesn't run it unsandboxed.

### Landlock on Linux

Landlock is off unless `HONE_SANDBOX_ENFORCE=1` is set; without it shell commands run unsandboxed, and Hone logs that once at warn level. With it, Landlock needs kernel 5.13 or newer: on an older kernel the command fails instead of running unsandboxed. There is no fallback backend.

## Recipes & TDD issues

### Recipe `test_cmd` fails but recipe completes

The recipe's `test_cmd` is enforced after `green` and after `verify`. If you see a green check on a failing suite, your `test_cmd` is wrong. Verify by running it directly:

```bash
cargo test --workspace        # whatever your test_cmd is
echo $?                       # 0 means pass
```

If exit is 0 but tests "fail", your test runner doesn't propagate failure — common with bash-piped output. Use `set -o pipefail` or run tests directly.

### TDD watcher never triggers

```bash
RUST_LOG=hone_tui::tdd_mode=debug hone
```

If you see "watching N paths" with N=0, the watcher couldn't enumerate. On NFS/SSHFS, file events are unreliable; restart hone after each save.

## Logging and observability

### Where things are written

Hone writes no log files. Logs go to stderr, filtered by `RUST_LOG`. Redirect them to keep them; the TUI draws on stdout, so this works there too:

```bash
RUST_LOG=debug hone 2> hone.log
```

MCP servers' own stderr is discarded. The files Hone does write (full list in [Where Hone keeps data](../configuration.md#where-hone-keeps-data)):

| Path | Contents |
|---|---|
| `~/.local/share/hone/sessions.db` | Saved sessions (one-shot runs, `honed`) |
| `~/.local/share/hone/memory.db` | Long-term memory |
| `~/.local/share/hone/tool-cache.db` | Cache of read-only tool results |
| `<cwd>/.hone/audit.jsonl` | Every tool call, plus one `context` line per model call. Only with `--audit` or `HONE_AUDIT_LOG=1`; read it with `hone trace list` |
| `<cwd>/.hone/reports/` | Session reports (`hone reports`) |

### Bumping log levels at runtime

```bash
RUST_LOG=hone=debug hone
RUST_LOG=hone_providers::acp=trace,hone_core::dispatcher=debug hone
```

`trace` is noisy — keep it scoped to one module.

### Per-turn metrics

Inside a session: `Ctrl+A` opens the session anatomy overlay. Each turn shows:
- Input tokens, output tokens, cached tokens.
- Wall-clock latency (TTFT and full-completion).
- Tool calls dispatched and their latencies.
- Cost in USD.

For programmatic access, a one-shot run with `--json` reports usage and cost.

## Reporting bugs

Reproduce on a clean session:

```bash
hone --version 2>&1 | tee version.log
RUST_LOG=hone=debug hone "<minimal repro>" 2>&1 | tee repro.log
```

File an issue with `version.log` and `repro.log` attached. Redact API keys (hone tries to mask them but verify before posting).

Issues: https://github.com/wojciechkpl/hone/issues

## Related

- [`docs/tutorials/getting-started.md`](getting-started.md) — happy path setup.
- [`docs/tutorials/security.md`](security.md) — sandbox configuration in depth.
- [`docs/tutorials/server-and-remote.md`](server-and-remote.md) — daemon-specific issues.
- [`docs/tutorials/tdd-with-hone.md`](tdd-with-hone.md) — when tests fail and you want to debug them with the agent.
