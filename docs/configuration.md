# Configuration reference

<p align="center">
  <img alt="config files" src="https://img.shields.io/badge/config-TOML%20%2B%20Markdown%20%2B%20JSON-6e40c9">
  <img alt="env vars" src="https://img.shields.io/badge/env%20vars-40%2B-1f6feb">
  <img alt="permissions" src="https://img.shields.io/badge/permissions-allow%20%7C%20ask%20%7C%20deny-e8445a">
</p>

Hone has no single settings file. Each file below is read by one part of Hone, and most
behaviour is controlled by environment variables and CLI flags. This page lists every file
and variable that the code reads today, and says plainly where a setting is parsed but not
applied yet.

## At a glance

```mermaid
%%{init: {"themeVariables": {"fontSize": "13px"}, "flowchart": {"nodeSpacing": 25, "rankSpacing": 30, "padding": 8}, "sequence": {"actorMargin": 30, "messageMargin": 25, "boxMargin": 6}, "gantt": {"barHeight": 16, "barGap": 3}}}%%
flowchart LR
  subgraph inputs["What you write"]
    ENV["Environment variables"]
    FLAGS["CLI flags"]
    CFG["config.toml"]
    PERM["permissions.toml"]
    MCPJ["mcp.json"]
    MD["HONE.md"]
    AG["agents/*.md<br/>commands/*.md"]
    TH["theme.toml"]
  end
  subgraph hone["What reads it"]
    DET["Provider detection"]
    STRICT["Strict Config schema<br/>(validated, not applied yet)"]
    GATE["Permission gate"]
    MGR["MCP manager"]
    SYS["System prompt"]
    REG["Agent registry,<br/>TUI slash commands"]
    TUI["TUI theme"]
  end
  ENV --> DET
  FLAGS --> DET
  CFG -->|"[provider] table"| DET
  CFG --> STRICT
  PERM --> GATE
  MCPJ --> MGR
  MD --> SYS
  AG --> REG
  TH --> TUI
```

## Files

| File | Read by | Format | Applied? |
|---|---|---|---|
| `~/.config/hone/config.toml`, `<cwd>/.hone/config.toml` | `hone` | TOML | The `[provider]` table: **yes**. The strict schema: validated only ([details](#configtoml)) |
| `~/.config/hone/permissions.toml`, `<cwd>/.hone/permissions.toml` | `hone`, `honed`, `hone-mcp-server` | TOML | **yes** ([Permissions](#permissions)) |
| `~/.config/hone/mcp.json`, `<cwd>/.hone/mcp.json` | `hone` | JSON (Claude Code compatible) | **yes** ([MCP servers](#mcp-servers)) |
| `~/.config/hone/HONE.md`, `<project root>/HONE.md`, `<cwd>/HONE.md` | `hone` | Markdown | **yes** ([HONE.md](#honemd)) |
| `~/.config/hone/agents/*.md`, `<project root>/.hone/agents/*.md` | `hone`, `honed` | Markdown + YAML front matter | **yes** ([Agents and commands](#custom-agents-and-slash-commands)) |
| `~/.config/hone/commands/*.md`, `<project root>/.hone/commands/*.md` | `hone` TUI | Markdown | **yes** |
| `~/.config/hone/theme.toml` | `hone` TUI | flat `key = "#rrggbb"` lines | **yes** ([Theme](#theme)) |

`<project root>` is the nearest ancestor of the working directory that contains `.git`,
`Cargo.toml`, `package.json`, `pyproject.toml`, `go.mod` or `pom.xml`. `<cwd>` is the
working directory itself: those files are **not** looked up in parent directories.

---

## Choosing a provider and model

Provider settings come from environment variables first, then from the `[provider]` table
that `hone configure` writes. With no `--model` flag, Hone takes the first provider that
is available, in this order:

```mermaid
%%{init: {"themeVariables": {"fontSize": "13px"}, "flowchart": {"nodeSpacing": 25, "rankSpacing": 30, "padding": 8}, "sequence": {"actorMargin": 30, "messageMargin": 25, "boxMargin": 6}, "gantt": {"barHeight": 16, "barGap": 3}}}%%
flowchart TD
  A{"ANTHROPIC_API_KEY<br/>(or wizard key)"} -->|set| A1["anthropic:claude-sonnet-4-5"]
  A -->|unset| B{"OPENAI_API_KEY<br/>(or wizard key)"}
  B -->|set| B1["openai:gpt-4o<br/>HONE_OPENAI_BASE_URL / HONE_OPENAI_MODEL override"]
  B -->|unset| C{"claude on PATH?"}
  C -->|yes| C1["claude-cli:claude-sonnet-4-5<br/>(no Hone tools)"]
  C -->|no| D{"gemini on PATH?"}
  D -->|yes| D1["gemini-cli:gemini-2.5-pro<br/>(no Hone tools)"]
  D -->|no| E{"Ollama answers on<br/>localhost:11434 and has llama3.2?"}
  E -->|yes| E1["ollama:llama3.2"]
  E -->|no| X["Exit 1 with setup hints"]
```

`--model <provider>:<name>` picks the model explicitly. The accepted prefixes are
`anthropic:`, `openai:`, `ollama:`, `claude-cli:` and `gemini-cli:`; anything else exits
with an error. `anthropic:` and `openai:` still need their API key; `ollama:` always
connects to `http://localhost:11434`.

| Provider | How to enable | Hone's tools | Notes |
|---|---|---|---|
| Anthropic | `ANTHROPIC_API_KEY=sk-ant-...` | yes | Default model `claude-sonnet-4-5` |
| OpenAI | `OPENAI_API_KEY=...` | yes | Default model `gpt-4o` |
| OpenAI-compatible | `OPENAI_API_KEY` + `HONE_OPENAI_BASE_URL` (+ `HONE_OPENAI_MODEL`) | yes | DeepSeek `https://api.deepseek.com`, OpenRouter `https://openrouter.ai/api/v1`, vLLM `http://localhost:8000/v1`, ... |
| Ollama | `ollama serve` + `ollama pull llama3.2` | yes | Fixed URL `http://localhost:11434` |
| Claude CLI | `claude` on `PATH` | no: runs its own tools | Permission mode `acceptEdits` unless `HONE_CLAUDE_PERMISSION_MODE` is set |
| Gemini CLI | `gemini` on `PATH` | no: runs its own tools | |

> [!NOTE]
> The Claude CLI and Gemini CLI adapters run their own tools, which Hone's permission
> rules can't see. With a non-empty permission policy, Hone refuses them unless you set
> `HONE_ALLOW_UNGATED_PROVIDER=1`. See [Permissions and sandbox](permissions-and-sandbox.md#providers-that-run-their-own-tools).

### What `hone configure` writes

The wizard asks for a provider (Anthropic, OpenAI, DeepSeek, OpenRouter, Ollama or
Claude CLI), an API key when one is needed, and a model, then writes
`~/.config/hone/config.toml`:

```toml
[provider]
name = "deepseek"
api_key = "sk-..."
model = "deepseek-chat"

base_url = "https://api.deepseek.com"
```

At startup Hone uses these values only where the matching environment variable is unset:
`anthropic` fills `ANTHROPIC_API_KEY`; `openai`, `deepseek` and `openrouter` fill
`OPENAI_API_KEY`, `HONE_OPENAI_BASE_URL` and `HONE_OPENAI_MODEL`. Nothing else is read:
choosing Ollama or Claude CLI, or picking an Anthropic model, currently has no effect. The key is stored in
plain text, and the wizard does not mask it while you type.

---

## config.toml

`~/.config/hone/config.toml` and `<cwd>/.hone/config.toml` are also loaded through a
**strict** schema (`hone_config::Config`, `deny_unknown_fields`). The project file wins
over the user file, key by key.

> [!WARNING]
> **The strict schema is validated but not applied yet.** `hone` loads it and logs
> "config loaded successfully", but no code reads `default_model`, `[sandbox]`,
> `[[routing]]`, `[budget]` or `[server]` today. Use `--model`, the environment variables
> and the TUI's `/budget` command instead. A file that fails validation is skipped with a
> warning (visible with `RUST_LOG=warn`); it never stops Hone from starting.

> [!CAUTION]
> The wizard's `[provider]` table is not part of the strict schema, so a wizard-written file
> always logs `unknown field 'provider'` at warn level. That warning is harmless: the
> `[provider]` table is still read by the provider reader above. Tables such as `[agent]`,
> `[memory]` and `[pricing]` were never part of either reader and are rejected.

### Schema

| Key | Type | Default | Meaning |
|---|---|---|---|
| `default_model` | string `provider:model` | none | Model to use when no routing rule matches |
| `[providers.<name>]` | table | empty | `api_key_env` (name of the env var holding the key), `base_url`, `model`. No other keys. |
| `[sandbox]` | table | `mode = "Manual"` | `mode` = `"Manual"`, `"Supervised"` or `"Autonomous"` (capitalised); `allowed_commands`, `allowed_network_domains`, `protected_paths` (lists) |
| `[[routing]]` | array of tables | empty | `task` (for example `"file_read"`) and `model`; first match wins |
| `[budget]` | table | none | `daily_limit_usd`, `session_limit_usd` (floats) |
| `[server]` | table | `127.0.0.1`, `3117` | `host`, `port`, `auth_token` |

<details>
<summary><b>A complete file that passes validation</b></summary>

```toml
default_model = "anthropic:claude-sonnet-4-5"

[providers.openai]
api_key_env = "OPENAI_API_KEY"
base_url = "https://api.deepseek.com"
model = "deepseek-chat"

[sandbox]
mode = "Supervised"
allowed_commands = ["cargo", "git"]
allowed_network_domains = ["crates.io"]
protected_paths = ["~/.ssh"]

[[routing]]
task = "file_read"
model = "anthropic:claude-haiku-4-5"

[budget]
daily_limit_usd = 10.0
session_limit_usd = 2.0

[server]
host = "127.0.0.1"
port = 3117
```

Again: these values are validated, not applied. `honed` takes its host, port and token
from `HONE_BIND`, its `PORT` argument and `HONE_AUTH_TOKEN`, not from `[server]`.

</details>

---

## Permissions

Permission rules decide which tool calls run, ask first, or are refused. With no
permission files, Hone behaves as before. The full guide, with diagrams and recipes, is
[Permissions and sandbox](permissions-and-sandbox.md).

**Files**

- User: `~/.config/hone/permissions.toml`
- Project: `<working dir>/.hone/permissions.toml`. Only the working directory itself is
  checked: running from a subdirectory does not pick up a parent's `.hone/`.

**Rules.** Each rule has one matcher (`command`, `path` or `host`), a `decision`
(`allow`, `ask` or `deny`), an optional `id`, and for path rules an `op` filter
(`read`, `write` or `any`). Rules without an `id` get an automatic one such as
`user:2` or `project:1`. The full format is in
[`docs/plans/policy-v2-spec.md`](plans/policy-v2-spec.md).

```toml
version = 2

[[rules]]
id = "no-push"
command = "git push*"
decision = "deny"

[[rules]]
id = "ask-secrets"
path = "secrets/**"
op = "read"
decision = "ask"
```

**Precedence.** Built-in rules come first (they ship, but are opt-in; see
[Built-in rules](#built-in-rules)), then the user file, then the project file. User rules
replace the built-in verdict. Project rules can only tighten: a project file cannot allow
what the user file asks about or denies. A broken permission file stops the run (exit
code 2 for the CLI, HTTP 400 for honed).

```mermaid
%%{init: {"themeVariables": {"fontSize": "13px"}, "flowchart": {"nodeSpacing": 25, "rankSpacing": 30, "padding": 8}, "sequence": {"actorMargin": 30, "messageMargin": 25, "boxMargin": 6}, "gantt": {"barHeight": 16, "barGap": 3}}}%%
flowchart LR
  S["Subject"] --> U{"First matching<br/>user rule?"}
  U -->|yes| BASE["base = user verdict"]
  U -->|no| BI{"Built-in rule?<br/>(only with HONE_POLICY_BUILTINS=1)"}
  BI -->|no| ALLOW["base = allow"]
  BI -->|yes| BASE2["base = built-in verdict"]
  BASE --> P{"Project rule<br/>stricter than base?"}
  ALLOW --> P
  BASE2 --> P
  P -->|yes| PV["verdict = project rule"]
  P -->|no| BV["verdict = base"]
```

**Asks.**

- Interactive runs (the TUI) pause and prompt. Approving "always" on a rule with a
  wildcard (any `*`, `?`, `[` or `{`, e.g. `*.example`, `{workdir}/**`, `git push*`)
  approves only that host, path or command, so approving one doesn't approve the others.
  Approving "always" on a host rule without a wildcard, or on a single-pattern path rule
  without a wildcard and with an explicit `op = "read"` or `"write"`, remembers the whole
  rule for the session; every other path rule is remembered per path and op. Approving "always" on a **command** rule, even one without a wildcard,
  covers one command line: approving `git push origin main` does not cover
  `git push --force origin main`. (A command rule without a wildcard matches the command
  with any arguments, which is why it is remembered per command line, not per rule.)
- Headless runs (one-shot, `--json`, honed without a step channel) cannot prompt, so an
  `ask` is refused with `requires approval (rule <id>)`. A `deny` is always refused.

**`--approve <RULE_ID>`** pre-approves the `ask` decisions of a rule for one run. It is
repeatable and takes rule ids from your permission files (explicit or automatic), the
built-in ids when `HONE_POLICY_BUILTINS=1`, or `builtin:unparsed-command` and
`builtin:invalid-host`. `--approve` covers every command line of the rule, unlike
"always". With built-ins on it is valid with no permission files, e.g.
`--approve builtin:git-push`. An unknown id, or `--approve`
with no permission files, exits with code 2. It never turns a `deny` into an allow.

```bash
hone --approve ask-secrets "summarise the notes under secrets/"
```

**honed.** `POST /run` loads the policy for the run's working directory (the user file
comes from the daemon's `$HOME`) and accepts `"approve": ["rule-id", ...]` in the
request body. A broken policy file, an unknown id, or `approve` with no policy gives
400. A non-empty `approve` also needs an authenticated honed (an auth token set);
without one the request gets 400 `approve requires an authenticated server`.
Orchestration runs apply the policy but do not take `approve`. The distributed worker
loads the policy once at startup, from its own working directory, and exits with code 2
if a permission file is broken.

**`hone-mcp-server`** applies the policy to every `tools/call`: a `deny` or an `ask`
returns an error result and the tool is not called. There is no prompt, so use
`HONE_APPROVE=id1,id2` to pre-approve rules. An unknown id, or a broken policy file,
makes the server exit with code 2.

**Providers that run their own tools** (the `claude` CLI adapter, ACP agents and the
Gemini CLI adapter) execute tools themselves, so the policy cannot check them. Under a
non-empty policy Hone refuses to use such a provider (exit code 2 / HTTP 400) unless
`HONE_ALLOW_UNGATED_PROVIDER=1` is set, in which case it prints a warning and continues.
With no policy or an empty one there is no refusal.

### Built-in rules

Set `HONE_POLICY_BUILTINS=1` (exactly `1`) to load a built-in rule set as the first
layer. It is **opt-in** until the benchmark gate passes, the same way as
`HONE_SANDBOX_ENFORCE`. Without it, and without permission files, behaviour doesn't
change. Every entry point reads the variable once at startup: the CLI, honed
(including orchestration and the distributed worker) and `hone-mcp-server`.

With built-ins on the policy is never empty, so providers that run their own tools are
refused unless `HONE_ALLOW_UNGATED_PROVIDER=1`. Rules are checked in this order, and
within the layer the first match wins:

| Id | Covers | Decision |
|----|--------|----------|
| `builtin:env-template` | `.env.example`, `.env.sample`, `.env.template` under the working directory | allow |
| `builtin:env-file` | `.env`, `.env.*`, `*.env` and `.envrc` (any directory) | ask |
| `builtin:ssh` | the `.ssh` directory in your home | ask |
| `builtin:cloud-credentials` | `~/.aws`, `~/.config/gcloud`, `~/.azure`, `~/.kube`, `~/.docker/config.json`, `~/.vault-token`, the Terraform credentials file | ask |
| `builtin:git-credentials` | `~/.git-credentials`, `~/.config/git/credentials`, `~/.netrc`, `~/.config/gh/hosts.yml` | ask |
| `builtin:package-credentials` | `.npmrc` and `.pypirc` (any directory), `~/.cargo/credentials*`, `~/.gem/credentials` | ask |
| `builtin:gnupg` | `~/.gnupg/**` | ask |
| `builtin:shell-history` | `~/.bash_history`, `~/.zsh_history`, the fish, python, psql and mysql history files | ask |
| `builtin:hone-config` | writes to `~/.config/hone/**` and to any `.hone` directory (permissions, hooks, MCP servers, skills, agents) | ask |
| `builtin:shell-startup` | writes to `~/.bashrc`, `~/.zshrc`, `~/.profile` and the other shell startup and logout files, `~/.config/fish/**` | ask |
| `builtin:git-hooks` | writes to any `.git` directory or anything under it (hooks, config, `commondir`, modules, nested repos) | ask |
| `builtin:git-config` | writes to `~/.gitconfig` and `~/.config/git/**` | ask |
| `builtin:autostart` | writes to `~/.config/systemd`, `~/.config/autostart`, `~/Library/LaunchAgents` | ask |
| `builtin:user-bin` | writes to `~/.local/bin`, `~/bin` and `~/.cargo/bin` | ask |
| `builtin:toolchain-config` | writes to `~/.cargo/config.toml` and `~/.cargo/config` | ask |
| `builtin:workdir-write` | writes under the working directory | allow |
| `builtin:temp-write` | writes to `/tmp`, `/var/tmp` (also under `/private`), the per-user `T/` directory under `/var/folders` and `/private/var/folders` (both spellings of `$TMPDIR`), `/dev/null` and a few other devices | allow |
| `builtin:outside-workdir` | any other write | ask |
| `builtin:git-push` | `git push` with any arguments | ask |
| `builtin:chmod-777` | `chmod` with `777` in the arguments | ask |
| `builtin:git-config-get`, `builtin:git-config-list`, `builtin:git-config-list-long` | `git config --get…`, `-l…`, `--list…` | allow |
| `builtin:git-config-cmd`, `builtin:git-inline-config` | any other `git config`, and `git -c …` | ask |
| `builtin:git-remote-set-url`, `builtin:git-remote-add` | changing a remote | ask |
| `builtin:sudo`, `builtin:sudoedit`, `builtin:su`, `builtin:doas`, `builtin:pkexec`, `builtin:run0` | privilege escalation | deny |
| `builtin:crontab-list` | `crontab -l` | allow |
| `builtin:crontab`, `builtin:at`, `builtin:systemd-run`, `builtin:launchctl` | scheduling and service managers | ask |
| `builtin:new-host` | any network host | ask |

Reads are allowed unless a rule above says otherwise. Path rules for `~` don't match
when no home directory is known. Paths are matched case-insensitively on macOS.

**Loosening a built-in.** The user file replaces a built-in verdict for the subjects its
rules match, so you can allow one host without touching the rest:

```toml
version = 2

[[rules]]
id = "pypi"
host = "pypi.org"
decision = "allow"
```

The project file can only tighten, so a project `host = "*"` allow does not lift
`builtin:new-host`. User and project rule ids can't contain `:`, so they can't collide
with `builtin:` ids.

**Pipe-to-shell.** With built-ins on, a command that runs code read from a stream is
denied with `builtin:pipe-to-shell`. That covers:

- a later pipeline stage that reads its program from standard input: a shell
  (`curl … | sh`, `wget -qO- … | bash -s`), an interpreter (`cat x | python3`,
  `curl … | python3 -`, also `perl`, `ruby`, `node`, `php`, `lua`, `pwsh`, `tclsh`,
  `osascript`, `deno run -`, `bun run -`, `Rscript -`, `awk -f /dev/stdin`), a script
  argument of `-`, `/dev/stdin`, `/dev/fd/N` or `/proc/self/fd/N`, and `source`/`.` with
  those paths;
- a process substitution used as the script, in any position: `bash <(curl …)`,
  `source <(curl …)`, `python3 <(curl …)`;
- a heredoc or here-string fed to `source`/`.` or to a shell reading stdin. A literal body
  (a quoted delimiter such as `<<'EOF'`, or a here-string without expansions) is the
  script: it is parsed like a `-c` string and checked against the rules, so
  `bash <<'EOF'` with `sudo id` inside is denied by `builtin:sudo`. An unquoted heredoc whose
  body has `$(…)` or backticks, or a here-string with an expansion, cannot be known before
  it runs and is denied by `builtin:pipe-to-shell`. A stdin path as the script
  (`bash /dev/stdin`) with no heredoc, here-string or pipe asks through
  `builtin:unparsed-command`.

Wrappers are looked through (`env`, `sudo`, `nice`, `busybox`, `ionice`, `strace`, `exec`,
`command`, `unbuffer`, `flock FILE`, `chroot DIR`, and `script`/`sg -c CMD`), more shells
are covered (`ash`, `mksh`, `ksh93`, `yash`, `tcsh`, `csh`, `elvish`, `nu`, `xonsh`),
versioned names such as `python3.12`, `php8.2` and `node22` are matched, and options that take a value consume it
(`bash --rcfile F`, `python3 -X dev`, `perl -I lib`). `xargs` children are not flagged:
xargs gives its child no pipe on stdin. No approval lifts the deny, and
`--approve builtin:pipe-to-shell` is rejected. `curl … | jq .`, `… | python3 -m json.tool`,
`git ls-files | xargs python3` and `bash script.sh` are not affected. `sh -c "$(curl …)"`
asks (through `builtin:unparsed-command`) rather than denies, as do `parallel` and `expect`.

**Tool-level asks.** With built-ins on, `computer_control` (`builtin:computer-control`) and
`screen_capture` (`builtin:screen-capture`, since a screenshot can carry anything on
screen to the provider) ask before every call. Approving "always" remembers the id for
the session, and `--approve builtin:computer-control` is valid only with built-ins on.

**Redaction.** With built-ins on, tool output is scanned before it enters the model
context or a `ToolCallEnd` event, and secret-shaped values become `[REDACTED:<kind>]`:
key blocks (including PGP), AWS, GitHub, GitLab, Slack (and webhook URLs), Stripe,
Anthropic, OpenAI, Google, npm, Hugging Face and SendGrid keys, JWTs, `Authorization:`
credentials, Docker `auth` values, kubeconfig `*-key-data`, netrc passwords, the password
in `scheme://user:password@host` (the user may be empty), and the value of a
`KEY=value` / `"key": "value"` assignment whose key names a secret (`token`,
`password`, `api_key`, ...). An assignment value is redacted only if it looks like a
literal secret: at least 8 characters, not a plain number, not an identifier or field
name without a digit (`api_key`, `args.password`), and containing a digit or at least 20
characters. So `Client(api_key=api_key)` and `password = args.password` are left alone,
while `API_KEY=abc123xyz789` is redacted. When something was redacted, the tool result ends with
`[hone: redacted N secret-shaped value(s) from this output]`. `hone-mcp-server` redacts
the text of `tools/call` results the same way. The egress audit log's result previews,
`PostToolUse` hook payloads and context-injection text are redacted too. Each result is
scanned once, in time linear in its size. Tool arguments, model messages and user
messages are not redacted. **Known limit:** redaction is pattern-based, so an encoded
or unusually shaped secret still reaches the model, and a key such as
`TOKEN_LIMIT=unlimited` is redacted by mistake.

**`grep` and `glob`.** When a loaded policy has path rules, the native `grep` skips
files the policy denies or wants approved, and ends its output with
`(N file(s) skipped by permission policy)`. `glob` drops denied paths but keeps paths
that only ask, since a name isn't the content and reading the file still goes through
the gate. The tool-result cache never serves or stores a `grep` or `glob` result when the
filter is active.

**Shell commands and paths.** Literal and glob paths in shell commands (`cat ~/.netrc`,
`cat ~/.ss*/id_*`, redirections, `cp`, `tee`) are gated like the file tools. Each path is
checked both as written and as resolved through symlinks, and the stricter verdict wins
(on a tie the resolved path's rule id is reported), so a symlinked shell startup file
that points into the working directory still asks (globs and `dd if=`/`of=` included).
If the working directory itself is a symlink, `{workdir}` in a rule matches both the
resolved and the logical spelling. A path built from a variable or a substitution cannot be
resolved, so with built-ins on it asks through `builtin:unparsed-command`, but only where
a path is read or written: a redirection target or source, a path argument of a
file-consuming command (`cat`, `head`, `tail`, `cp`, `mv`, `rm`, `ln`, `tee`, `dd`,
`touch`, `chmod`, `tar -f`, `sed -i`, `source`, …) and the script argument of a shell or
interpreter (`f=…; cat "$f"`, `cat "$(printf …)"`, `echo x > "$OUT"`). Option values
(`-m "$MSG"`), `sed`/`grep`/`awk`/`jq` scripts, non-file commands (`kill`, `sleep`,
`git checkout`, `echo`) and `$(cat <<'EOF' … EOF)` do not ask. `$HOME`, `${HOME}` and `~`
are resolved, not unknown.

A `for` variable over literal words or a literal glob is substituted: each value (or glob
match), up to 32 combinations per word, becomes a path subject with the command's real
read or write op, exactly as if written out (`for f in ~/.bashrc; do cp evil "$f"; done`
asks `builtin:shell-startup`). The binding holds only inside the loop body and is dropped
when the variable is reassigned (`f=`, `f+=`, `read`, `printf -v`, `mapfile`, `declare`,
`export f=…`, `getopts`, `((f=…))`, `unset`, `f[0]=…`), when an inner loop rebinds the same
name, after any `eval`, `source` or `.`, after `declare -n`/`local -n`/`typeset -n`, and inside
function bodies, and on any line that defines a function (in any form); past 32 combinations, or for a non-literal
list, the variable counts as unknown. After `cd`/`pushd` to a target that isn't literal
(an expansion, a substitution, `-`, `$OLDPWD`), later relative path arguments and
redirects on the same line count as unknown too, and no later `cd` makes it known again
(it may not run); a `cd` inside `( … )` or `$( … )` never changes the outer line's directory,
while `eval` and `source` strings do; absolute paths and non-file commands
(`cd "$DIR" && cargo test`) are unaffected.

An unquoted heredoc fed to a shell or `source` is denied by `builtin:pipe-to-shell` when
its body has any parameter expansion (`$X`, `${…}`) as well as `$(…)` or backticks: the
outer shell splices the value into the script before the inner shell parses it. A
quoted heredoc (`<<'EOF'`) is parsed and gated.

**Known gaps.**

- **Shell startup-file variables** can still run code from a stream with no Deny:
  `… | BASH_ENV=/dev/stdin bash -c true`, `ENV=…`, `bash --rcfile /dev/stdin -i`, and
  `PS4='$(…)' bash -x`. This is the first item planned for P2.6.
- **Loop variables are not tracked** through aliases (`expand_aliases`), or through
  functions or aliases defined in a sourced file or in `BASH_ENV`.
- **Directory changes are not followed** through `env -C DIR cmd`, a sourced script, or
  `trap '…'` strings. `trap '…'` strings can also reassign variables.
- **False positive:** an array assignment like `a[0]=/usr/bin/sudo` can be read as a
  command named by the value's basename. Not seen by the path checks (the sandbox is the boundary): aliases, hard
links, `python -c "open(…)"`, reads by data flow (`xargs cat`, `find -exec cat`) and by
copying a parent directory (`cp -a ~ /tmp/h`, `rsync -a ~/`). Not yet handled: the logical
working directory from `$PWD` in the CLI, `hone-mcp-server` and the worker; git's global
options (`git -C dir push`, `git -C . config …`); command runners (`find -exec`, `tmux new`,
`screen -dm`, `ssh host cmd`, `watch`, `make -f /dev/stdin`); `export NAME` without `=`,
function bodies and `expand_aliases` in the approval context; and a few redaction misses
(short passphrases and URL passwords, `JWT_SECRET=word`) and false positives
(`Basic base64(user:pass)` placeholders, `public-key-data:`).

**What an approval covers.** Answering a prompt "once" approves only the rule that was
asked, for that call. If the same call also hits another ask rule, you are prompted again
for that one (for example `git push … && echo x >> ~/.bashrc` asks for `builtin:git-push`,
then `builtin:shell-startup`). Headless runs fail on the first ask. "Always" on a command
rule is remembered for the exact command line **and** the same leading assignments,
wrapper words and working directory: after approving `git push origin main`,
`GIT_SSH_COMMAND=… git push origin main`, `env X=1 git push origin main` and
`cd /tmp/o && git push origin main` all ask again, as do variables exported earlier on the
line (`export X=…`, `declare -x`, `set -a`) and assignments made inside a subshell. A nested
command string (`sh -c`, `bash -c`, `eval`), a heredoc script and a line that runs `source`
or `.` never match a remembered approval, and "always" there is not remembered. Only a literal host rule, or a literal
single-pattern path rule with an explicit `read` or `write` op, is remembered by its id;
every other rule is remembered per subject, and path keys include the op, so approving a
read never approves a write.

**Switch and messages.** A `HONE_POLICY_BUILTINS` value other than exactly `1` (for
example `true`, `yes`, `on`) leaves built-ins off and prints one warning line (honed logs
it at warn level). When the policy is non-empty only because of built-ins, the refusal for
a provider that runs its own tools tells you to unset `HONE_POLICY_BUILTINS`, use another
provider, or set `HONE_ALLOW_UNGATED_PROVIDER=1`; with that override set, tool output from
that provider is not redacted, and the warning says so. `--approve` (and honed `approve`,
and `HONE_APPROVE`) rejects an id whose rule always denies (the privilege rules,
`builtin:pipe-to-shell` and any user or project `deny` rule) with
`--approve: rule '<id>' always denies; --approve only lifts ask rules` (exit code 2; 400
for honed).

**Order of checks.** The permission gate runs before `PreToolUse` hooks, so a denied or
unapproved call never reaches a hook. If a hook rewrites the input, the gate runs again
on the rewritten input.

**Limits.** Command rules match the command text after parsing, which is a speed bump
rather than a security boundary: shell aliases, interpreters (`python -c ...`), paths
inside tool options and similar indirections are not seen. The sandbox is the boundary;
see [Sandbox](#sandbox).

<a id="sandbox-configuration"></a>

## Sandbox

The OS sandbox wraps child processes started by the `shell` tool. It is configured only
through environment variables; the `[sandbox]` table in `config.toml` is not applied.

| Platform | Default | Variable |
|---|---|---|
| macOS (Seatbelt) | on | `HONE_SANDBOX_EXTRA_READ` adds read-only roots |
| Linux (Landlock) | off: opt-in while it is validated on a Landlock kernel | `HONE_SANDBOX_ENFORCE=1` turns it on; `HONE_SANDBOX_EXTRA_READ` adds read-only roots |
| Other | no sandbox | — |

Details, including what is readable and writable, are in
[Permissions and sandbox](permissions-and-sandbox.md#3-os-sandbox).

---

## MCP servers

MCP servers are configured in JSON, in the same shape as Claude Code's `.claude/mcp.json`,
so you can copy or symlink an existing file.

```json
{
  "mcpServers": {
    "github": {
      "command": "npx",
      "args": ["-y", "@modelcontextprotocol/server-github"],
      "env": { "GITHUB_PERSONAL_ACCESS_TOKEN": "${GITHUB_TOKEN}" },
      "timeout_ms": 10000
    },
    "filesystem": {
      "command": "npx",
      "args": ["-y", "@modelcontextprotocol/server-filesystem", "/workspace"],
      "disabled": true
    }
  }
}
```

| Field | Default | Meaning |
|---|---|---|
| `command` | required | Executable to spawn (stdio transport) |
| `args` | `[]` | Arguments |
| `env` | `{}` | Extra environment; values may reference `${VAR}` |
| `disabled` | `false` | Skip this server at startup |
| `timeout_ms` | `10000` | Per-request timeout |

The project file (`<cwd>/.hone/mcp.json`) wins over `~/.config/hone/mcp.json` when server
names collide. A missing or malformed file, or a server that fails to start, is logged and
skipped. Native tools win over MCP tools with the same name. Permission rules don't see
MCP tool calls.

## HONE.md

`HONE.md` holds conventions for the agent. Hone merges up to three files, least specific
first: `~/.config/hone/HONE.md`, `<project root>/HONE.md`, `<cwd>/HONE.md`. Each `##`
heading is a section; sections with the same heading are concatenated. The result is
added to the system prompt.

```markdown
## Style
- Prefer small, pure functions. No `unwrap()` outside tests.

## Commands
- Tests: `cargo test --workspace`
```

## Custom agents and slash commands

| What | Where | Precedence |
|---|---|---|
| Agent personas | `~/.config/hone/agents/*.md`, `<project root>/.hone/agents/*.md` | project > user > built-in, by name |
| Slash commands | `~/.config/hone/commands/*.md`, `<project root>/.hone/commands/*.md` | project > user, by file name |

An agent file is Markdown with YAML front matter. The body becomes the system prompt.

```markdown
---
name: release-notes
description: Writes release notes from merged PRs and the changelog
tools: [read_file, grep, glob, shell, write_file]
model: sonnet
---

You write concise release notes. Group changes by user impact...
```

In the TUI, start a message with `@release-notes` to use it. `tools:` must use real tool
names (`read_file`, `write_file`, `edit_file`, `multi_edit`, `glob`, `grep`, `shell`,
`git_status`, `git_diff`, `git_commit`, `web_fetch`, ...); unknown names are dropped with
only a log warning. An empty or missing `tools:` list keeps every tool.

> [!WARNING]
> The built-in personas list short names such as `read`, `edit` and `write`, which match
> no tool. When `@persona` (or a distributed worker) applies their list, they keep only
> `shell`, `grep` and `glob` and lose the file tools. This is a known bug.

## Theme

The TUI picks a base theme from `HONE_THEME` (`crimson` by default, or `neon`, `chalk`,
`minimal`), then applies overrides from `~/.config/hone/theme.toml`. Each line is
`key = "#rrggbb"`; unknown keys and invalid colours are ignored.

```toml
accent = "#FF1744"
surface = "#0d1117"
text_primary = "#E6E1E8"
role_error = "#E74C3C"
```

<details>
<summary><b>All theme keys</b></summary>

| Key | Aliases |
|---|---|
| `accent_hot` | `accent` |
| `accent_mid`, `accent_deep` | |
| `text_primary`, `text_secondary`, `text_muted` | |
| `role_user`, `role_assistant`, `role_tool_result`, `role_error` | |
| `role_tool` | `role_tool_call` |
| `surface_base` | `surface` |
| `surface_raised`, `surface_overlay`, `surface_focus`, `surface_border` | |
| `budget_low`, `budget_mid`, `budget_high` | |

`NO_COLOR` or `HONE_NO_COLOR` switches to the minimal ASCII theme. `HONE_REDUCED_MOTION=1`
stops the spinner animation; `HONE_ASCII_ONLY=1` uses ASCII box drawing.

</details>

---

## Where Hone keeps data

| Path | Contents | Turn off |
|---|---|---|
| `~/.local/share/hone/memory.db` | Long-term memory (SQLite) | `--ephemeral`, `HONE_DISABLE_PERSISTENT_STORES=1` |
| `~/.local/share/hone/index.db` | Symbol index of `.rs` and `.py` files in the working dir | same |
| `~/.local/share/hone/sessions.db` | Conversations from one-shot runs (saved under a generated name unless you pass `--session`); `honed` sessions (`HONE_SESSION_DB` overrides) | `--ephemeral`; `HONE_EPHEMERAL` for `honed` |
| `~/.local/share/hone/tool-cache.db` | Cache of read-only tool results | `HONE_TOOL_CACHE=0` |
| `<cwd>/.hone/audit.jsonl` | Egress audit log of every tool call | off unless `--audit` or `HONE_AUDIT_LOG=1` |
| `<cwd>/.hone/reports/` | Session reports (`hone reports`) | |
| `<cwd>/.hone/plan.md` | Orchestrator plan and worker status | |
| `<cwd>/.hone/plans/` | Saved plans from plan mode (`hone --plan FILE`) | |
| `<project root>/.hone/specs/` | Output of `hone spec` | |

## Environment variables

<details open>
<summary><b>Providers and models</b></summary>

| Variable | Purpose |
|---|---|
| `ANTHROPIC_API_KEY` | Anthropic API key; also drives `honed`'s `POST /run` |
| `OPENAI_API_KEY` | OpenAI key, or the token for an OpenAI-compatible endpoint |
| `HONE_OPENAI_BASE_URL` | OpenAI-compatible endpoint URL (DeepSeek, Qwen, Kimi, OpenRouter, vLLM, ...) |
| `HONE_OPENAI_MODEL` | Model name for that endpoint (default `gpt-4o`) |
| `HONE_CLAUDE_PERMISSION_MODE` | Permission mode passed to the `claude` CLI (default `acceptEdits`) |
| `HONE_CACHE` | Any value wraps the provider in an in-process LRU response cache (100 entries) |
| `HONE_CONTEXT_WINDOW` | Override the model's context window in tokens (4096–10000000), e.g. `32768` for a model served with a 32k window. Applies to every agent and model in the process. Hone compacts conversation history at its per-model budget ratio of this value (Qwen: 45%); the system prompt and tool schemas come on top, so windows below about 16k may still overflow. Unset: the window is inferred from the model name. An invalid value is ignored and logged at warn level. |

</details>

<details>
<summary><b>Context, memory and storage</b></summary>

| Variable | Purpose |
|---|---|
| `HONE_NO_INJECTION` | `1` disables proactive context injection (same as `--no-inject`) |
| `HONE_SKELETON_STRATEGY` | `adaptive` (default), `call-graph`, `hybrid`, `dependency-graph` or `flat` |
| `HONE_DISABLE_PERSISTENT_STORES` | `1` skips the memory and symbol-index stores. (`--ephemeral` does that and also skips saving the session.) |
| `HONE_MEMORY_PASSPHRASE` | Passphrase for `--encrypted-memory` (required with that flag) |
| `HONE_TOOL_CACHE` | `0` disables the tool-result cache |
| `HONE_AUDIT_LOG` | `1` writes `.hone/audit.jsonl` (same as `--audit`) |
| `HONE_WEBSEARCH_PROVIDER`, `HONE_WEBSEARCH_KEY` | `tavily`, `brave` or `exa` plus a key. Live search is not implemented yet: the `web_search` tool returns an error either way. |

</details>

<details>
<summary><b>Permissions and sandbox</b></summary>

| Variable | Purpose |
|---|---|
| `HONE_ALLOW_UNGATED_PROVIDER` | `1` lets a provider that runs its own tools be used under a non-empty permission policy (see [Permissions](#permissions)) |
| `HONE_POLICY_BUILTINS` | `1` loads the built-in permission rules and turns on pipe-to-shell denial, tool-level asks and secret redaction (opt-in; see [Built-in rules](#built-in-rules)) |
| `HONE_APPROVE` | Comma-separated permission rule ids pre-approved for `hone-mcp-server` |
| `HONE_SANDBOX_ENFORCE` | `1` turns on Landlock enforcement for shell commands on Linux (opt-in while it is validated; Seatbelt on macOS is always on) |
| `HONE_SANDBOX_EXTRA_READ` | Extra read-only roots for the shell sandbox, colon-separated |

</details>

<details>
<summary><b>Terminal UI and logging</b></summary>

| Variable | Purpose |
|---|---|
| `HONE_THEME` | `crimson` (default), `neon`, `chalk`, `minimal` |
| `NO_COLOR`, `HONE_NO_COLOR` | Disable colour (minimal ASCII theme) |
| `HONE_REDUCED_MOTION` | Disable spinner and Agent Rail animation |
| `HONE_ASCII_ONLY` | Force ASCII box drawing and file-op badges |
| `RUST_LOG` | Log filter, e.g. `warn`, `hone_core=debug` |

</details>

<a id="server-configuration"></a>

<details>
<summary><b><code>honed</code> daemon and distributed mode</b></summary>

| Variable | Purpose |
|---|---|
| `HONE_AUTH_TOKEN` | Bearer token. Required when binding a non-loopback address, whatever the role: `honed` exits 2 without it. Also needed by `hone --distributed` and by workers |
| `HONE_BIND` | Bind host, default `0.0.0.0`. Use `127.0.0.1` for loopback-only |
| `HONE_ALLOW_UNAUTH` | Exactly `1` allows a non-loopback bind without a token (not recommended) |
| `HONE_SESSION_DB` | SQLite path for sessions (default `~/.local/share/hone/sessions.db`) |
| `HONE_EPHEMERAL` | Any value keeps sessions in memory only |
| `HONE_MEMORY_DB` | Enables `/memory` endpoints backed by this SQLite file |
| `HONE_PROVENANCE_DIR` | Records each `/run` as a hash chain here and enables `/provenance` endpoints |
| `HONE_EVAL_HISTORY_DB` | Enables `/evals` endpoints |
| `HONE_DISTRIBUTED_DB` | Run as coordinator, with the task queue in this SQLite file |
| `HONE_DISTRIBUTED_REAP_SECS`, `HONE_DISTRIBUTED_MAX_ATTEMPTS` | Lease reaper interval (default 15) and max claim attempts (default 3) |
| `HONE_WORKER_COORDINATOR` | Run as a worker that polls this coordinator URL |
| `HONE_WORKER_ID`, `HONE_WORKER_CAPABILITIES` | Worker id (default: hostname) and comma-separated tags |
| `HONE_WORKER_LSP_LANGS`, `HONE_WORKER_MCP_CONFIG`, `HONE_WORKER_MEMORY_DB` | Optional LSP languages, MCP config and memory file for the worker |

</details>

## CLI flags

```text
hone [OPTIONS] [PROMPT] [COMMAND]
```

| Flag | Purpose |
|---|---|
| `PROMPT` | Run one prompt and exit (one-shot) |
| `-m, --model <ID>` | `anthropic:…`, `openai:…`, `ollama:…`, `claude-cli:…`, `gemini-cli:…` |
| `--json` | One-shot with structured JSON output (response, tool calls, usage, cost, exit status) |
| `--orchestrate` | Run the prompt through the multi-agent orchestrator |
| `--distributed <URL>` | With `--orchestrate`, dispatch subtasks through a remote `honed` coordinator (needs `HONE_AUTH_TOKEN`) |
| `--ensemble <N>` | Run N candidates (2–10) at different temperatures and rank them |
| `--recipe <NAME>`, `--list-recipes`, `--var KEY=VALUE` | Run a built-in recipe, list recipes, set recipe variables |
| `--plan <FILE>` | Execute a plan saved by plan mode |
| `--image <FILE>` | Attach png, jpg, gif or webp images to the prompt (repeatable) |
| `--session <NAME>` | Name under which a one-shot run is saved. The TUI doesn't save sessions yet; it writes a session report on exit. |
| `--ephemeral` | No memory, index or session persistence |
| `--encrypted-memory` | Encrypt memory content with AES-256-GCM (needs `HONE_MEMORY_PASSPHRASE`) |
| `--approve <RULE_ID>` | Pre-approve a permission rule's `ask` (repeatable) |
| `--audit` | Write the egress audit log to `.hone/audit.jsonl` |
| `--no-inject`, `--skeleton-strategy <S>` | Turn off context injection, or choose the skeleton strategy |
| `--no-clarify` | Don't print clarifying questions for ambiguous one-shot prompts |
| `--step` | TUI: ask before every tool call, including read-only ones. Not covered: the read-before-edit guard's own `read_file` and the automatic test run after an edit |
| `--vim` | TUI: vim-style keybindings |
| `--agent <NAME>` | Parsed but not applied yet; use `@agent-name` in the TUI |

| Subcommand | Purpose |
|---|---|
| `hone configure` | Provider wizard (writes `~/.config/hone/config.toml`) |
| `hone session list \| resume <name> \| rename <old> <new> \| delete <name>` | Named sessions |
| `hone memory ls \| show <id> \| forget --tag/--kind/--before` | Inspect and prune memory |
| `hone trace list \| show <id> \| stats` | Read the egress audit log as traces |
| `hone reports list \| show <id> \| diff <a> <b>` | Session reports |
| `hone eval <tasks.toml>` | Run an eval task file and write `report.json` / `report.md` |
| `hone spec "<goal>"` | Write requirements, design and tasks to `.hone/specs/<slug>/` |

There are no flags for temperature, iteration limit, sandbox mode or context window. Set a
budget in the TUI with `/budget <usd>`; set the context window with `HONE_CONTEXT_WINDOW`.

## Troubleshooting

<details>
<summary><b>"No AI provider detected"</b></summary>

Hone checked Anthropic, OpenAI, `claude`, `gemini` and Ollama in that order and found
none. Set a key or start Ollama:

```bash
export ANTHROPIC_API_KEY=sk-ant-...
# or
ollama pull llama3.2      # with `ollama serve` running
```

</details>

<details>
<summary><b>My <code>default_model</code> (or <code>[sandbox]</code>, <code>[budget]</code>) setting has no effect</b></summary>

The strict `config.toml` schema is validated but not applied yet. Use `--model`, the
provider environment variables, `/budget` in the TUI, and the sandbox variables above.

</details>

<details>
<summary><b><code>Failed to load config: unknown field ...</code> in the logs</b></summary>

The strict schema rejects keys it doesn't know. A wizard-written file always produces
`unknown field 'provider'`; that one is harmless. For other keys, compare the file with
the [schema](#schema).

</details>

<details>
<summary><b><code>hone: permission policy error: ...</code> and exit code 2</b></summary>

A `permissions.toml` failed to parse or contains an invalid rule (bad id, a host pattern
with a port, `**` inside a segment, ...). Fix the file named in the message; Hone won't run
without the rules you wrote.

</details>

<details>
<summary><b><code>requires approval (rule ...)</code> in a one-shot or <code>--json</code> run</b></summary>

Headless runs can't prompt. Re-run with `--approve <rule id>`, or change the rule.

</details>

<details>
<summary><b><code>command blocked (requires approval): ...</code></b></summary>

The shell tool's built-in checks refused a destructive, network or chained command.
There is no prompt at this layer; split the command or use a dedicated tool. See
[shell command checks](permissions-and-sandbox.md#2-shell-command-checks).

</details>

## See also

- [Getting started](getting-started.md)
- [Permissions and sandbox](permissions-and-sandbox.md)
- [Architecture](architecture.md)
- [Provider tutorial](tutorials/providers.md) (older; may be out of date, this page wins)
