# Security: Sandboxing and Policies

## Feature Status

| Feature | Status | Notes |
|---|---|---|
| Kernel sandbox (Seatbelt / Landlock) | Available | Seatbelt on macOS; Landlock on Linux 5.13+ is opt-in (`HONE_SANDBOX_ENFORCE=1`) |
| Environment variable filtering | **Not available** for shell commands | Commands the `shell` tool starts inherit Hone's environment, including API keys. MCP servers get only `PATH`, `HOME` and their configured `env`. See [Environment Variable Filtering](#environment-variable-filtering) |
| Audit logging | Opt-in | `--audit` or `HONE_AUDIT_LOG=1` writes `.hone/audit.jsonl` |
| Memory encryption (AES-256-GCM) | Opt-in | `--encrypted-memory` with `HONE_MEMORY_PASSPHRASE` set; built in, no Cargo feature |
| SQLite at-rest encryption (sqlcipher) | Planned | Not yet implemented |

> [!NOTE]
> Security settings live in `permissions.toml` and environment variables, not in `config.toml`. The reference for both is [Permissions and sandbox](../permissions-and-sandbox.md).

---

Hone puts three checks between the model and your machine: permission rules, shell command checks, and an OS sandbox for shell commands. It can also keep an audit log of tool calls. This guide covers the sandbox, the threat model, and how to configure each part.

## Linux Landlock Sandbox

On Linux kernel 5.13+, Hone can enforce filesystem access for shell commands via the **Landlock LSM** (Linux Security Module). It is opt-in: Landlock applies only when `HONE_SANDBOX_ENFORCE` is exactly `1`. Otherwise Hone logs one warning (at warn level) and runs shell commands unsandboxed on Linux. In `hone` the sandbox covers shell commands only, and the only setting beyond that switch is `HONE_SANDBOX_EXTRA_READ` (see [Permissions and sandbox](../permissions-and-sandbox.md#3-os-sandbox)). The `hone-sandbox` library API looks like this:

```rust
use hone_sandbox::{SandboxPolicyBuilder, SandboxMode};
use std::path::PathBuf;

let policy = SandboxPolicyBuilder::new(SandboxMode::Manual)
    .allow_read(PathBuf::from("/tmp"))
    .allow_write(PathBuf::from("/tmp/output"))
    .build()?;
```

Only shell commands are sandboxed: the shell tool installs the sandbox on the child process it starts. File tools such as `read_file` and `write_file` run inside the Hone process, so the OS sandbox never applies to them; Hone confines them to the working directory itself, and permission rules narrow them further. See [Permissions and sandbox](../permissions-and-sandbox.md#3-os-sandbox).

**Requirements:**
- Linux kernel 5.13+ (Landlock upstreamed in this release); check with `uname -r`
- Landlock enabled in the running kernel: `cat /sys/kernel/security/lsm` must list `landlock`

With `HONE_SANDBOX_ENFORCE=1`, if Landlock is unavailable, the sandbox setup in the child process returns `SandboxError::Unavailable`, the command never runs, and the shell call fails instead of running unsandboxed (fail-closed). Without the variable there is no fail-closed behaviour: commands run unsandboxed.

To require Landlock and fail if unavailable:

```bash
HONE_SANDBOX_ENFORCE=1 hone
```

---

## Sandbox Architecture Overview

| Platform | Sandbox | Default |
|---|---|---|
| macOS | Seatbelt (via `sandbox-exec`) | On |
| Linux | Landlock | Opt-in: `HONE_SANDBOX_ENFORCE=1` |
| Other | None | — |

The sandbox covers shell commands only. `SandboxBuilder::detect()` picks Seatbelt, Landlock or no sandbox by platform; there is no seccomp, container or `setrlimit` tier. See [Permissions and sandbox](../permissions-and-sandbox.md#3-os-sandbox).

## SandboxPolicy

`SandboxPolicy` is the `hone-sandbox` library type that describes what a sandboxed child process may do:

```rust
pub struct SandboxPolicy {
    pub mode: SandboxMode,
    pub allow_read: Vec<PathBuf>,      // paths the child may read
    pub allow_write: Vec<PathBuf>,     // paths the child may write (implies read)
    pub allow_exec: Vec<PathBuf>,      // executables the child may exec
    pub allow_network: bool,           // outbound network (Seatbelt only)
    pub allowed_domains: Vec<String>,  // not enforced by Seatbelt or Landlock
    pub allow_env: Vec<String>,        // not enforced by Seatbelt or Landlock
}
```

You can't set it from `hone`. The shell tool builds one for every command: the working directory, `/tmp` and `/dev/null` are writable; system directories, `~/.cache`, `~/.cargo`, `~/.rustup`, your git config and any `HONE_SANDBOX_EXTRA_READ` roots are readable; network is denied (Seatbelt only: Landlock doesn't restrict it). The full table is in [Permissions and sandbox](../permissions-and-sandbox.md#3-os-sandbox).

## Configuration

Hone's security settings are in these places:

| What | Where |
|---|---|
| Which files, commands and `web_fetch` hosts the agent may use | Permission rules in `~/.config/hone/permissions.toml` and `<workdir>/.hone/permissions.toml` |
| Built-in rules and secret redaction | `HONE_POLICY_BUILTINS=1` |
| The OS sandbox for shell commands | `HONE_SANDBOX_ENFORCE=1` (Linux) and `HONE_SANDBOX_EXTRA_READ` |
| Audit log | `--audit` or `HONE_AUDIT_LOG=1` |

There are no per-tool sandbox policies. The `[sandbox]` table in `config.toml` is not applied (Hone warns at startup), and sub-tables such as `[sandbox.tools.shell]` make the file fail to load.

A starting `permissions.toml`:

```toml
# ~/.config/hone/permissions.toml
version = 2

[[rules]]
id = "no-ssh-keys"
path = "~/.ssh/**"
decision = "deny"

[[rules]]
id = "no-dotenv"
path = "**/.env"
decision = "deny"

[[rules]]
id = "ask-push"
command = "git push*"
decision = "ask"
```

A call that matches no rule is allowed. Within a file, the first matching rule wins. In `path` rules, `~/` expands to your home directory, `{workdir}` to the working directory, and a relative pattern is joined onto the working directory. A pattern that starts with `**` is not joined, so `**/.env` matches a `.env` file anywhere. The full format is in [Permissions and sandbox](../permissions-and-sandbox.md#rule-format).

## Network Access

Hone doesn't deny network access for all tools, and there is no per-tool network setting. What applies depends on how the call reaches the network:

| Route | What limits it |
|---|---|
| Shell commands | The shell command checks refuse `curl`, `wget`, `ssh`, `scp`, `rsync`, `nc`, `ftp` and `git push/pull/fetch/clone/remote` on every platform. On macOS, Seatbelt also denies network to every shell command. Landlock doesn't restrict network |
| `web_fetch` | `host` permission rules |
| MCP servers, `web_search` | Nothing in Hone: MCP servers run outside the sandbox, and permission rules don't see MCP tools or `web_search` |
| Requests to the model provider | Not restricted: they go to the provider you chose |

To limit which hosts `web_fetch` may reach:

```toml
# ~/.config/hone/permissions.toml
[[rules]]
id = "docs-ok"
host = "*.rust-lang.org"
decision = "allow"

[[rules]]
id = "metadata-never"
host = "169.254.169.254"
decision = "deny"

[[rules]]
id = "other-hosts"
host = "*"
decision = "ask"
```

Host rules match names, not resolved addresses, so they don't stop DNS tricks such as a name that resolves to a metadata IP.

## Process Resource Limits

Hone sets no CPU, memory, disk or open-file limits on commands. The only limit is time: each `shell` call has a `timeout_ms` (30 s by default; the model can ask for up to 5 minutes), and when it runs out Hone kills the command's whole process group. With the default timeout, the call then fails with `tool 'shell' timed out after 30000ms`. MCP servers have a per-request `timeout_ms` in `mcp.json` (see [MCP servers](../configuration.md#mcp-servers)).

## Environment Variable Filtering

> [!WARNING]
> Hone doesn't filter the environment of shell commands, and there is no `allow_env` setting. Commands the `shell` tool starts inherit Hone's environment, including provider API keys, so the model can read a key with a command such as `echo $ANTHROPIC_API_KEY`. Other processes Hone starts, such as LSP servers, hooks and the TUI's test command, inherit it too.
>
> MCP servers are the exception: they start with a cleared environment and get only `PATH`, `HOME` and the `env` set for them in `mcp.json`.

To limit what the model can read:

- Export only the keys Hone needs, and don't start Hone from a shell that holds other credentials.
- `HONE_POLICY_BUILTINS=1` loads the built-in permission rules (including pipe-to-shell denial and tool-level asks) and turns on redaction: secret-shaped values in tool output (Anthropic and OpenAI keys, among other formats) are replaced with `[REDACTED:<kind>]` before the model sees them. The matching is pattern-based, so an encoded or unusually shaped secret still gets through. See [Permissions and sandbox](../permissions-and-sandbox.md).

## File System Restrictions

File tools (`read_file`, `write_file`, `edit_file`, `multi_edit`, `glob`, `grep`) are confined to the working directory: each one resolves its path, symlinks included, and refuses anything that lands outside the working directory, with or without permission rules. This check is in Hone's code; the OS sandbox doesn't apply to file tools. In the TUI, write tools ask before each call unless you switch to AutoApprove.

`path` rules in `permissions.toml` narrow access inside the working directory. They also apply to the paths in shell commands (`cat`, redirections, `cp`, `tee`), which can reach outside it.

### Read Rules

A `path` rule with `op = "read"` covers reads only:

```toml
# ~/.config/hone/permissions.toml
[[rules]]
id = "no-secrets"
path = "secrets/**"
op = "read"
decision = "deny"
```

When the agent calls `read_file(path="secrets/api.txt")`, the call is refused with `denied by policy` and the rule id `no-secrets`. A path outside the working directory, such as `/etc/shadow`, is refused even without a rule.

### Write Rules

`op = "write"` covers writes only, so the agent can still read the files:

```toml
# <repo>/.hone/permissions.toml  (can only tighten your user rules)
[[rules]]
id = "frozen-migrations"
path = "migrations/**"
op = "write"
decision = "deny"
```

### Symlink Handling

A file tool resolves symlinks before its working-directory check, so if `notes/key` is a symlink to `~/.ssh/id_ed25519`, `read_file(path="notes/key")` is refused even without a rule.

Permission rules check a path both as written and with symlinks resolved, and the stricter verdict wins. If `docs/key.txt` is a symlink to `secrets/key.txt`, the `no-secrets` rule above still denies `read_file(path="docs/key.txt")`. Shell commands get the same check for the paths Hone can see in them.

## Command Restrictions

Hone has no executable allowlist. Two things limit shell commands:

- The shell command checks, always on: privilege escalation (`sudo`, `su`, `doas`, `pkexec`) is blocked, and destructive, network and chained commands are refused. See [Shell command checks](../permissions-and-sandbox.md#2-shell-command-checks).
- `command` rules in `permissions.toml`. A pattern without `*` or `?` is a prefix at a word boundary (`cargo publish` matches `cargo publish --dry-run`); a pattern with them is a glob over the whole command.

```toml
# ~/.config/hone/permissions.toml
[[rules]]
id = "no-publish"
command = "cargo publish"
decision = "deny"

[[rules]]
id = "ask-npm"
command = "npm"
decision = "ask"
```

Both match command text. Shell aliases, interpreters (`python -c …`) and paths hidden inside tool options are not visible to them, so treat them as a speed bump. The OS sandbox is the boundary.

## Server Mode (`honed`)

Hone has no roles and no per-user permissions or budgets. `honed` checks one bearer token, `HONE_AUTH_TOKEN`, and every client that sends it gets the same access. The token is required when `honed` binds a non-loopback address, unless you set `HONE_ALLOW_UNAUTH=1`.

> [!WARNING]
> With `HONE_ALLOW_UNAUTH=1` and no token, anyone who can reach the port can run the agent on your machine. Bind to loopback (`HONE_BIND=127.0.0.1`) or set a token instead. Permission rules apply to every run: `honed` loads the user file from its own `$HOME` and the project file from the run's working directory. See the [Server Guide](server-and-remote.md).

## Audit Logging

Hone writes an audit log only when you ask for it:

```bash
hone --audit "fix the failing test"

# or, for every run
export HONE_AUDIT_LOG=1
```

Entries are appended to `.hone/audit.jsonl` in the working directory, one JSON object per line: one per tool call, including calls a permission rule refused, plus one `context` line per model call. There is no setting for the log's path or level.

A call refused by a rule (wrapped here; in the file it is one line):

```json
{
  "timestamp": "2026-11-12T09:30:00Z",
  "tool": "read_file",
  "arguments": {"path": "/home/alice/.ssh/id_ed25519"},
  "result_preview": "policy: deny (rule no-ssh-keys)",
  "blocked": true,
  "block_reason": "policy: deny (rule no-ssh-keys)",
  "injection_risk": null,
  "duration_ms": 0,
  "trace_id": "019a77c4-2f1e-7d0a-9b1c-5e3f2a8d4c61",
  "span_id": "019a77c4-3a02-7e4b-8c2d-6f4a3b9e5d72",
  "parent_span_id": "019a77c4-2f1f-7a3c-a4e5-1b2c3d4e5f60",
  "span_kind": "tool_call"
}
```

Read the log with `hone trace` (from the same directory) or `jq`:

```bash
hone trace list          # recent traces, one summary line each
hone trace show 019a77c4 # one trace as a tree; an id prefix is enough
hone trace stats

# Calls a rule refused
jq 'select(.blocked)' .hone/audit.jsonl

# Shell commands the agent ran
jq -r 'select(.tool == "shell") | .arguments.command' .hone/audit.jsonl
```

## Memory Encryption (Encrypted Memory Feature)

> **Status: Available** — Application-layer AES-256-GCM encryption of memory content is implemented in `crates/hone-memory/src/encrypted.rs`. This encrypts the `content` field of each memory record using a key derived from a user passphrase via Argon2id. Full SQLite file encryption via sqlcipher is **not yet implemented** (Planned).

Enable optional memory encryption with the `--encrypted-memory` flag. It is built in: there is no Cargo feature and no `config.toml` setting.

The encryption key is derived from a user passphrase via Argon2id:

```bash
export HONE_MEMORY_PASSPHRASE="your-strong-passphrase"
hone --encrypted-memory
# The passphrase is read from the environment on every run; nothing is stored in a keychain.
```

Memory content (conversation history) is encrypted at rest. The SQLite file structure itself is not encrypted — for full disk encryption, use OS-level tools or ephemeral mode.

## Ephemeral Mode

Run a session with all data immediately discarded after completion:

```bash
hone --ephemeral
```

In ephemeral mode:
- Session data is never persisted to disk
- No memories are recorded
- Cost tracking is not saved
- Useful for sensitive queries

---

## Memory Persistence and Encryption

By default, session memories are stored in SQLite at `~/.local/share/hone/memory.db`:

```bash
ls -la ~/.local/share/hone/
# memory.db            - session memories (content encrypted with --encrypted-memory)
# sessions.db          - conversations from one-shot and --plan runs (not with --ephemeral)
```

The TUI doesn't save conversations to `sessions.db`. `HONE_SESSION_DB` changes the sessions path only for `honed`; see [Where Hone keeps data](../configuration.md#where-hone-keeps-data).

With `--encrypted-memory`, the content field of each memory record is encrypted with AES-256-GCM before being written to disk (see [Memory Encryption](#memory-encryption-encrypted-memory-feature) above).

Full SQLite file encryption via sqlcipher is planned for a future release. Until then, for sensitive workloads where even encrypted content fields are insufficient, use `--ephemeral` to disable persistence entirely:

```bash
hone --ephemeral "Sensitive query"
```

Or clear memories manually:

```bash
rm ~/.local/share/hone/memory.db
```

## Threat Model Summary

| Threat | What Hone does | What it doesn't cover |
|---|---|---|
| Prompt injection that runs commands or edits files | Shell command checks; permission rules; the OS sandbox limits shell writes to the working dir and `/tmp`; file tools refuse paths outside the working dir | Landlock is off on Linux unless `HONE_SANDBOX_ENFORCE=1`; inside the working dir, file tools can change anything no rule stops |
| Supply-chain attack on an MCP server | MCP servers start with a cleared environment: only `PATH`, `HOME` and their configured `env` | Hone doesn't verify, pin or hash servers; they run outside the sandbox, and permission rules don't see MCP tool calls |
| Model exfiltrating data | Seatbelt denies network to shell commands on macOS; command checks refuse `curl`, `ssh` and similar; `host` rules for `web_fetch` | Landlock doesn't restrict network; MCP tools and `web_search` aren't covered; host rules don't stop DNS tricks |
| Runaway command | The shell tool's `timeout_ms` (30 s by default) kills the process group | No CPU, memory or disk limits |
| Secret leakage via environment variables | MCP servers get only `PATH`, `HOME` and their configured `env`; `HONE_POLICY_BUILTINS=1` redacts secret-shaped values in tool output | Shell commands inherit Hone's full environment; redaction is pattern-based |
| Symlink escape | File tools resolve symlinks and refuse paths outside the working dir; path rules check the path as written and with symlinks resolved, and the stricter verdict wins | Paths Hone can't see in a shell command, such as those inside a script |
| Privilege escalation | `sudo`, `su`, `doas` and `pkexec` are blocked | The checks match command text, so a script that escalates isn't seen |

## Best Practices

1. **Start restrictive:** Turn on the built-in rules (credentials, persistence, `git push`, privilege escalation, pipe-to-shell, new hosts) and secret redaction, then add your own `deny` rules. [Configuration](#configuration) has a starting file.

```bash
export HONE_POLICY_BUILTINS=1
```

2. **End host rules with a catch-all:** a final `host = "*"` rule with `decision = "ask"` makes every host you didn't list ask first (see [Network Access](#network-access)). Path and command rules have no default deny: a call that matches no rule is allowed.

3. **Enable audit logging:** Record what tools actually do.

```bash
export HONE_AUDIT_LOG=1   # same as --audit
```

4. **Review MCP servers before you add them:** Hone doesn't hash or pin MCP servers, they run outside the sandbox, and permission rules don't see their tool calls. Pin the package version in `args`, and set `"disabled": true` on servers you don't use (see [MCP servers](../configuration.md#mcp-servers)).

5. **Rotate secrets regularly:** If an API key is exposed, rotate it immediately.

6. **Use ephemeral mode for sensitive prompts:**

```bash
hone --ephemeral "Analyze this sensitive document"
```

## Troubleshooting

### Sandbox Not Available

On Linux with `HONE_SANDBOX_ENFORCE=1`, a kernel without Landlock makes shell commands fail instead of running unsandboxed. Check the kernel version (5.13+ is needed) and that Landlock is enabled:

```bash
uname -r
cat /sys/kernel/security/lsm   # must list landlock
```

Upgrade the kernel, or unset `HONE_SANDBOX_ENFORCE` to run shell commands unsandboxed (not recommended). There is no `config.toml` setting for the sandbox, and on macOS Seatbelt is always on.

### File Access Denied

A call refused by a permission rule names the rule. Edit or remove that rule in `permissions.toml`. An `ask` rule in a headless run reports `(requires approval: rule <id>)` in the run's output (the audit log records it as `policy: requires approval (rule <id>)`). Approve it for one run with `--approve <id>`:

```bash
hone --approve ask-npm "run the npm tests"
```

A shell command that fails with a permission error from the OS hit the sandbox. Shell commands can write only to the working directory, `/tmp` and `/dev/null`, and that can't be widened. To let them read a directory outside the defaults, add it to `HONE_SANDBOX_EXTRA_READ` (colon-separated):

```bash
HONE_SANDBOX_EXTRA_READ="$HOME/.nvm:$HOME/.pyenv" hone
```

### Shell Command Timed Out

```
tool 'shell' timed out after 30000ms
```

Hone has no memory or CPU limits to raise. A `shell` call stops after its `timeout_ms`, 30 s by default; the model can pass a larger `timeout_ms` for a slow command, up to 5 minutes. For longer jobs, run them yourself outside Hone.

## Next Steps

- **Getting started:** See [Getting Started](getting-started.md) to run your first session.
- **Permissions:** Rules, command checks and the sandbox in [Permissions and sandbox](../permissions-and-sandbox.md).
- **Tools:** Built-in tools and MCP servers in [Tools Guide](tools-and-mcp.md).
- **Server:** Secure the daemon in [Server Guide](server-and-remote.md).
