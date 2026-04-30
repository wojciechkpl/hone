# Debugging and Troubleshooting

When something doesn't work, this guide is the playbook. It's organised by symptom — find your symptom, read the diagnosis, apply the fix.

## First five minutes

Before deep-diving, run these:

```bash
hone --version          # confirm you're on the build you think
hone --doctor           # built-in environment check (PATH, API keys, sandbox, deps)
ls ~/.hone/logs/        # latest session logs
```

The `--doctor` command exits non-zero on any failure, with one suggestion per problem. If it's clean, the issue is more specific — read on.

## Provider issues

### "API key not found"

```bash
echo $ANTHROPIC_API_KEY     # is it set?
hone --doctor | grep -i key
```

Fix:

```bash
export ANTHROPIC_API_KEY="sk-ant-..."
# or persist:
echo 'export ANTHROPIC_API_KEY="sk-ant-..."' >> ~/.zshrc
```

Or in `~/.config/hone/config.toml`:

```toml
[providers.anthropic]
api_key = "sk-ant-..."
```

### "rate limit" / "429"

Hone retries with exponential backoff (250 ms → 500 ms → 1 s → 2 s, 4 attempts). If it still fails:

1. Check your account's rate limit at the provider's dashboard.
2. Configure a fallback chain in `config.toml`:
   ```toml
   [providers.fallback_chain]
   primary = "anthropic"
   fallback = ["deepseek", "ollama"]
   ```
3. Lower concurrency: `hone --max-parallel 1`.

### "model not found"

```bash
hone --list-models       # what hone thinks is available
```

Common causes:
- Typo in model ID — anthropic uses `claude-sonnet-4-5`, not `claude-4-sonnet`.
- Provider routes the request to a different endpoint. Check `[providers.<name>].base_url`.
- The model is deprecated upstream. Pin a newer one in `config.toml`.

### "supports_tools: false"

Visible in `/configure`. The provider can't round-trip tool calls. For ACP providers, ensure you're on `--provider claude_acp`, not the legacy `--provider claude`. For raw HTTP providers, check that the model supports tool/function calling — Ollama models below 7B params often don't.

### Hot-swap doesn't take effect

```
/configure provider claude_acp
> some prompt
```

If the prompt still uses the old provider, the swap was queued for the *next* turn but the current one was already in flight. Cancel with `Esc` and re-issue.

## Tool dispatch issues

### Tool never gets called

Run with debug logging:

```bash
RUST_LOG=hone_core::dispatcher=debug,hone_providers=debug hone
```

Look for `tools=` in the request log — if the tools array is empty, hone didn't announce any. Common causes:
- No `[[tools]]` block in `~/.hone/tools.toml`.
- Tools are registered but the active provider has `supports_tools: false`.
- The model's `system_prompt` says "you have no tools" — check the prompt template.

### Tool times out

Default timeout is 30 s per tool call. Override per-tool in `tools.toml`:

```toml
[[tools]]
name = "long_running_query"
timeout_secs = 120
```

Or globally:

```toml
[dispatcher]
default_tool_timeout_secs = 60
```

### Tool returns `isError: true`

Read the error payload — it's plain text. Three common causes:

1. **Path traversal blocked** — sandbox refused a path outside the workspace. Adjust `[sandbox.fs]` in `config.toml` or stop the agent from leaving the project.
2. **Network denied** — by default the sandbox blocks outbound traffic. Allowlist hosts in `[sandbox.network]`.
3. **Schema validation** — the model passed args that don't match the tool's schema. Inspect with `RUST_LOG=hone_core::tool_registry=debug`.

### "tool result was never sent back"

For ACP only. The dispatcher executed the tool but the channel-write failed. Check for a panic in the runtime log:

```bash
grep -i panic ~/.hone/logs/<session>.log
```

If the tool itself panicked, fix the tool. If the channel was closed (rare), restart hone — this is a known race surfaced when the agent process dies mid-call; hone now reaps and reports it as `ObservedToolEnd { error }`.

## TUI issues

### Terminal too small

Hone requires 80×24. Below that you see "Terminal too small". Resize the terminal or switch to one-shot mode: `hone -i "..."`.

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

Sessions live in `~/.hone/sessions/<id>/`. Verify:

```bash
ls -la ~/.hone/sessions/
```

If the directory is gone, you ran `rm -rf ~/.hone` somewhere. There's no remote backup unless you opted in via `[storage.remote]`.

### Daemon crash / `honed` won't start

```bash
honed --port 7777 --foreground   # don't fork; logs go to stderr
```

Read the stderr output. Common causes:
- Port already in use: `lsof -i :7777`.
- Database lock: stale `~/.hone/state.db-wal`. Delete WAL, retry.
- Auth required but no token: see `docs/tutorials/server-and-remote.md`.

## Sandbox issues

### "permission denied" inside an agent shell call

Hone's sandbox blocks file/network access by default. To allow:

```toml
[sandbox.fs]
read_paths = ["./", "/tmp"]
write_paths = ["./", "/tmp"]

[sandbox.network]
allowed_hosts = ["api.github.com", "registry.npmjs.org"]
```

Restart hone after editing.

### "seatbelt: cannot enable" on macOS

Recent macOS versions deprecated `sandbox-exec`. Hone falls back gracefully but warns. To suppress the warning, set:

```toml
[sandbox]
prefer_native = false
```

### "landlock: not supported" on Linux

Kernel < 5.13. Either upgrade or set:

```toml
[sandbox]
prefer_native = false
```

(falls back to `setrlimit` + path validation; weaker but functional).

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

```
~/.hone/logs/
├── <session-id>.log          # session-scoped: every event, every turn
├── audit.log                 # structured audit (RBAC, tool dispatch, errors)
└── metrics.json              # per-session counters
```

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

For programmatic access, the daemon exposes `/sessions/<id>/metrics` (JSON).

## Reporting bugs

Reproduce on a clean session:

```bash
HONE_TELEMETRY=0 hone --doctor 2>&1 | tee doctor.log
HONE_TELEMETRY=0 RUST_LOG=hone=debug hone -i "<minimal repro>" 2>&1 | tee repro.log
```

File an issue with `doctor.log` and `repro.log` attached. Redact API keys (hone tries to mask them but verify before posting).

Issues: https://github.com/wojciechkpl/hone/issues

## Related

- [`docs/tutorials/getting-started.md`](getting-started.md) — happy path setup.
- [`docs/tutorials/security.md`](security.md) — sandbox configuration in depth.
- [`docs/tutorials/server-and-remote.md`](server-and-remote.md) — daemon-specific issues.
- [`docs/tutorials/tdd-with-hone.md`](tdd-with-hone.md) — when tests fail and you want to debug them with the agent.
