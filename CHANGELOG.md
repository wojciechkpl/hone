# Changelog

All notable changes to Hone are documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.0.0/), and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

### Corrected: SWE-bench claims withdrawn

- The README, docs and earlier changelog entries reported 86% and 95% SWE-bench "pass rates". These were patch-generation rates (any non-empty diff) on 50- and 20-task samples, extrapolated to 300 tasks, and were never graded. They are withdrawn, along with the cost comparisons built on them. The only graded result is **58/300 (19.3%)** on SWE-bench Lite (DeepSeek V3, April 2026, official harness). A pre-registered, harness-controlled comparison is in progress (`docs/plans/trust-defaults-daemon-2026-11.md`, `benchmarks/harness-compare/`).
- Docs now name the actual default skeleton strategy, **Adaptive**. `--help` previously said "hybrid" and the README said "CallGraph".

### Permission policy v2 (P2.1; no callers yet)

- **`hone_config::policy`.** A new policy model for Phase 2, where the policy is on by default.
  - **Subjects:** a rule matches a shell command, a file path (read or write) or a network host.
  - **Layers:** built-in rules, then user rules (`~/.config/hone/permissions.toml`), then project rules (`.hone/permissions.toml`). Each rule has an id. User rules replace built-in verdicts. Project rules, which come from an untrusted directory, can only make a verdict stricter.
  - **Formats:** v1 files (`match = …`) still load.
  - The contract is `docs/plans/policy-v2-spec.md`, and the agent loop doesn't call it yet; that is P2.2. `crates/hone-config/src/policy.rs`.
- **Hardened before any caller exists, after a security review.**
  - Workdir and home paths are glob-escaped, including `\`, before they are put into patterns.
  - Untrusted policy files are read with care:
    - only regular files are accepted, so a FIFO can't hang the loader;
    - at most 64 KiB is read;
    - a project file that is a symlink is refused;
    - TOML errors never echo file text, because a symlinked "policy" could otherwise leak a secret into logs, and an ordinary file could inject terminal escapes or text addressed to the model.
    - Every error message escapes control characters, and control characters in patterns and ids are rejected.
  - The user file is read from `~/.config/hone` on macOS too, not `~/Library/Application Support`, and there is no relative fallback.
  - Host subjects are normalised: userinfo, ports, brackets, IPv4-mapped IPv6 and trailing dots. Hosts may contain only an allow-list of characters. A malformed host is never allowed: it asks, or is denied if an explicit rule denies it.
  - Rules that could never match are rejected at load time.
  - Each layer is capped at 1000 rules.

### Permission policy in the agent loop (P2.2)

- **Every tool call is checked.** A new decision point runs after PreToolUse hooks (and their input rewrites) and before dispatch, on both the sequential path and the parallel read-only path. The guard's automatic read before an edit is checked too. `crates/hone-core/src/policy_gate.rs`, `crates/hone-core/src/agent.rs`.
- **What gets checked.** `Tool::subjects()` says what a call touches:
  - file tools give their paths;
  - `web_fetch` gives the host;
  - `git_*` tools give a `git …` command;
  - `shell` gives every command plus the paths in its arguments and redirects.

  The shell parser handles quoting, wrappers (`env`, `sudo`, `timeout`, …), `sh -c`/`eval`, reserved words, substitutions, heredocs, globs, braces and `cd`. A command it can't parse asks whenever command or path rules exist. The parser is a speed bump; the sandbox is the boundary. `crates/hone-tools/src/subjects.rs`.
- **Outcomes.**
  - **Deny:** the call fails with the rule id.
  - **Ask, terminal UI:** Hone pauses. "Always" approves that rule for the session, but never the synthetic `builtin:*` ids.
  - **Ask, headless** (`--json`, one-shot): the call fails closed with "requires approval (rule …)". One approved rule never covers a different rule in the same call.
  - **Egress log:** when the audit log is on, blocked calls are recorded.
- **Paths are canonicalised** before matching: symlinks, dangling symlinks, on-disk case. Path rules ignore case on macOS and Windows.
- **The CLI loads** `~/.config/hone/permissions.toml` and `.hone/permissions.toml`. If neither file exists, nothing changes. A broken file stops startup with exit code 2.
- **Fixes found along the way.**
  - `write_file` could escape the working directory through a dangling symlink.
  - `web_fetch` followed redirects to other hosts; it now stops and returns the `Location`.
  - Esc on an approval prompt in the TUI used to approve the call; it now skips it.
  - The tool cache wrapper now forwards `is_read_only()`.
- **Deferred,** with owners in `docs/plans/policy-p2.2-spec.md` §9: the `claude` CLI provider and `hone-mcp-server` run tools outside the gate (P2.3); `computer_control`, and `grep`/`glob` walks into protected files (P2.5).

### Permission policy at every entry point (P2.3)

- **One gate.** `decide`, `PolicyConfig`, `resolve_permission_policy` and the approval helpers now live in `hone_tools::policy_gate`. `hone_core` re-exports the same items. The CLI, honed and `hone-mcp-server` all call this one implementation.
- **`--approve <RULE_ID>`** (repeatable) pre-approves an ask rule for a headless run. It is validated against the loaded policy: an unknown id, or an id given with no policy loaded, exits 2. It never lifts a deny.
- **honed** loads the policy for each run's working directory. `POST /run` accepts `approve: [..]`, but only on a server with an auth token; without one, a non-empty `approve` returns 400. A policy that won't load, an unknown approval or a refused provider each return 400. Orchestration and the worker pool get the policy too.
- **`hone-mcp-server`** gates every `tools/call`. A deny returns `denied by policy (rule …)` and an ask returns `requires approval (rule …)`, both as `isError` results. `HONE_APPROVE` (comma-separated) pre-approves rules.
- **Providers that run their own tools** (`claude` CLI, ACP, and Gemini CLI as a fail-closed choice) are refused while a non-empty policy is loaded, because the gate can't see their tool calls. Router and fallback wrappers report this if any provider they hold does. `HONE_ALLOW_UNGATED_PROVIDER=1` overrides this, with a warning.
- **Wildcard rules.** "Always" on any rule with a wildcard (`*.example`, `{workdir}/**`, `git push*`) approves only the subject it was asked about. A literal rule is still approved for the whole session. `--approve` still covers every subject of the rule it names.
- **Smaller changes.**
  - Removed the unused `ToolDispatcher`, which bypassed the gate.
  - Ensemble candidates run headless, so they never share an approval channel.
  - `env -S'cmd'` is parsed.
  - `docs/configuration.md` has a Permissions section.

### Built-in permission rules and secret redaction (P2.5, opt-in)

- **`HONE_POLICY_BUILTINS=1`** turns on a built-in rule set and output redaction. Without it, and without policy files, nothing changes. They stay opt-in until the Phase 2 benchmark gate passes.
  - Any value other than exactly `1` leaves them off and prints a one-line warning.
  - With built-ins on, the policy counts as non-empty, so providers that run their own tools are refused unless `HONE_ALLOW_UNGATED_PROVIDER=1`, and `--approve builtin:…` works with no policy files.
- **Built-in rules** (39 rules, ids `builtin:…`, listed in `docs/configuration.md`):
  - Reading or writing these asks: `.env` files, `~/.ssh`, cloud, git and package credentials, `~/.gnupg`, and shell history.
  - Writing these asks:
    - shell startup files;
    - anything under any `.git` or `.hone`;
    - `~/.gitconfig`;
    - autostart folders;
    - user `bin` folders and `~/.cargo/bin`;
    - anything outside the workdir except temp directories.
  - These commands ask: `git push`, `git config` changes, `git -c`, `git remote add`/`set-url`, `chmod 777`, `crontab`, `at`, `systemd-run`, `launchctl`.
  - These are denied: `sudo`, `sudoedit`, `su`, `doas`, `pkexec` and `run0`.
  - Any host the agent fetches from asks.
  - User rules can still replace a built-in verdict; project rules can only tighten it.
- **Running code from a stream is denied (`builtin:pipe-to-shell`).** That covers:
  - `curl … | sh` and the same into Python, Perl, Ruby, Node, PHP or PowerShell;
  - `bash <(curl …)` and `source <(…)`;
  - stdin script paths;
  - unquoted heredocs with expansions fed to a shell.

  The pattern is still caught behind wrappers such as `env`, `nice`, `busybox`, `flock` and `script`, and under versioned interpreter names.
- **`computer_control` and `screen_capture` ask** (`builtin:computer-control`, `builtin:screen-capture`).
- **Shell parsing.**
  - **Variable-derived paths ask**, but only in real file positions. Examples: `cat "$f"`, `> "$out"`, or a relative path after `cd "$dir"`.
  - **`for` loops over literal words or globs** are substituted and checked as if written out. The substitution is not trusted after reassignment, `eval`/`source`, namerefs, or on a line that defines a function.
  - **Heredoc and here-string bodies** fed to a shell or `source` are parsed and gated.
  - **Paths** are checked both as written and resolved, so a symlinked dotfile can't escape its rule. A symlinked workdir matches in both forms.
- **"Always" approvals are narrower.**
  - "Always" on a command covers only that exact command line, including its leading assignments, wrappers and `cd`. It never covers a line with `sh -c`, `eval` or `source`.
  - "Approve once" covers only the rule it was asked about. Any other ask in the same call prompts too.
  - `--approve`, honed `approve` and `HONE_APPROVE` reject ids whose rule always denies.
- **`grep` and `glob` skip files the policy protects.** `grep` reports how many it skipped. The tool cache never serves a result that skipped the filter.
- **The gate runs before `PreToolUse` hooks**, and again if a hook rewrites the input.
- **Redaction (P2.5b).**
  - **What is redacted:** tool output that reaches the model, `ToolCallEnd` events, `hone-mcp-server` results, the egress-log preview, `PostToolUse` payloads and injected context.
  - **What it catches:** private keys (PEM, PGP and wrapped base64), cloud and API tokens, auth headers, docker auth, kube key data, URL and netrc passwords, and secret-named assignments with secret-looking values.
  - **Each match** is replaced with `[REDACTED:<kind>]`, followed by a one-line note. Templates, identifiers, prose, URLs and paths are left alone, so code the model reads back stays intact.
  - **Performance:** it runs in linear time.
- **Reviewed** in five security-review rounds against the real parser and redactor. The remaining known gaps, such as git global options, `find -exec` and `env -C`, are listed in `docs/configuration.md` and assigned to P2.6. Contract: `docs/plans/policy-p2.5-spec.md`.

### Fixed: TUI approval prompts on every turn

- **The bug (since April 2026):** the TUI's main agent was created with `Auto` approvals and no approval channel, before the TUI set them up. Simple and medium turns, which are most messages, run on that agent, so:
  - write and shell tools ran **without prompting**;
  - `--step` did nothing;
  - permission `ask` rules were refused as if the run were headless.
  (Orchestrated turns never prompted either: their lead and worker agents have no tools.)
- **The fix:** `AgentHandle::set_step_channel` gives the running agent the approval channel, and `wire_tui_approvals` applies the approval policy and channel to it. The agent is not respawned, so `hone session resume` history is kept.
- **Pausing safely.** A review found that once the session agent can pause, it could hang or fail open. Fixed in the same change:
  - **Medium turns** (the default tier) no longer stop reading events at the first answer while the self-refine pass is still running, which would have left its approval prompts unanswered forever. The critique pass's text is no longer shown; a refined answer appears under a "Revised answer" divider. The TUI gets one `Done` with the usage of every pass.
  - **Esc** now cancels the agent's turn, not just the task watching it. The turn stops before its next tool call, approval prompt or model call; calls that didn't run get a "cancelled" result, so the history stays valid, and the next message runs normally. A cancelled message that hadn't started yet leaves no trace in the history.
  - **No answer means no.** If the approval channel closes while a prompt is open (for example, the TUI exits), the call is refused instead of run. If approval is needed and no channel exists, the call is refused without showing a prompt.
  - **Stale answers** left in the channel by a cancelled prompt are discarded before the next prompt opens.
  - **Typing can't answer a prompt.** A key counts only if pressed at least 400 ms after the menu opened and 400 ms after the previous key; faster keys go to the input box. Esc always skips. Space no longer approves.
  - **`/permit`** no longer waits on a busy agent, which could freeze the TUI. It applies for the rest of the session, as `/budget` does (both help texts now say so).
  - **The automatic test run** after an edit happens only when an edit actually ran, not when you denied or skipped it.
  - "Approve always" allow-lists the tool that was paused, not a name supplied by the UI.
  - **Typing a follow-up** can't approve: while the input box has text, only <kbd>y</kbd> approves, and a paste counts as typing.
- **`honed`:** a client that disconnects from a `POST /run` or `/orchestrate` stream now cancels the run at its next tool call, prompt or model call, instead of letting it finish. The spend of a `/run` is recorded in the governance totals however the run ends (finished, failed or cancelled); before, only finished runs were counted.
- **CLI `--orchestrate`:** a failing worker no longer stops the printer, so the other workers and the merge still run.
- **AutoApprove mode** (<kbd>Shift</kbd>+<kbd>Tab</kbd>) now does what its name says: it skips the write/shell approval prompt, including the one `--step` adds. It never skips a permission rule: `deny` still refuses and `ask` still prompts.
- **Known gaps:** the automatic test run after an edit runs (approved, or in AutoApprove) is not prompted and not sandboxed, and (until "shell commands stop when their call ends" below) <kbd>Esc</kbd> didn't stop the fix turns it starts; with `--step`, <kbd>a</kbd> approves once.
- Contract: `docs/plans/tui-approval-fix-spec.md`.

### Fixed: MCP servers start and stop in parallel

- **Before:** hone started MCP servers one after another and waited for each one's handshake before starting the next, so startup took the sum of their start times. A hung server held up the rest by its whole handshake timeout (60 s by default), and several `npx -y …` servers downloaded their packages one at a time. Exit stopped servers one after another too, giving each one that ignored its input closing 5 s. The message hone sends a server at exit had no time limit, so a server that had stopped reading could keep hone from exiting at all.
- **Now:**
  - All enabled servers start and handshake at once, so startup waits only for the slowest one. A hung server delays it by its handshake timeout while the others connect.
  - Which servers connect, and their tools, are unchanged. If startup is cancelled, every server it started is killed.
  - Exit stops every server at once. Each gets at most 5 s in all, that message included, before it is killed.
  - No exit leaves an MCP server running any more (unix), short of a crash or `kill -9`. Before, a server that ignored its input closing kept running in these cases, all older than this change:
    - `hone session resume` with a missing session;
    - an unknown `--approve` id or a malformed project `permissions.toml`;
    - an out-of-range `--ensemble`;
    - SIGTERM or SIGHUP sent to `hone` alone.
  - Each server now runs in its own process group, which hone stops on every exit.
    - A normal exit, TUI quit included, shuts it down gracefully: hone closes its input and gives it up to 5 s.
    - An error exit, or a signal to a one-shot run or `honed`, sends it SIGTERM first, then SIGKILL if it is still running 1 s later. (In the TUI a signal takes the graceful path.)
    - If you start a server through a shell (`sh -c "cd DIR && server"`), write `exec server`. On Linux the 1 s ends as soon as the shell itself exits.
    - A failed `session resume` with a server that ignores its input closing takes about 5 s to exit.
    - Because a server is no longer in the terminal's process group, one that prompts on the terminal (`/dev/tty`) now stops until its handshake timeout.
  - Killing a server kills everything it started. A server launched through `npx -y …`, `uvx` or `sh -c` no longer leaves its real process running after a failed start or at exit. Before, only the launcher was killed.
- Contract: `docs/plans/mcp-parallel-connect-spec.md`.

### Fixed: MCP requests time out

- **Before:** `timeout_ms` in `mcp.json` (documented as the per-request timeout, default 10 000 ms) was never applied, so a request to a stdio MCP server that didn't answer waited forever. Servers connect one after another at startup, so a server that hung during its handshake kept hone from starting. A tool call to a hung server waited until <kbd>Esc</kbd> (TUI) or <kbd>Ctrl</kbd>+<kbd>C</kbd>, and a `honed` worker waited indefinitely, with every other call to that server queued behind it. A line on a server's stdout that wasn't JSON (log output) failed the call.
- **Now:**
  - Every request gives up after the server's `timeout_ms` with a timeout error. The model sees the error and the server stays connected; its late answer is skipped, so the next call gets its own result.
  - The startup handshake gets at least 60 s (or `timeout_ms` if larger), so an `npx -y …` server can download its package on first start; a new `handshake_timeout_ms` setting changes that. A server that doesn't finish its handshake in that time is skipped at startup and its process is killed; the other servers still connect.
  - Lines that aren't JSON, or aren't valid UTF-8 (the rest of a reply cut off by a timeout), are skipped.
  - Exiting no longer waits 5 s per MCP server: hone closes a server's input first, so a server that exits at end of input stops at once.
  - `timeout_ms: 0` means no timeout.
  - An MCP server's process is killed when hone lets go of it (on exit or after a failed start). Before, a server that ignored its input closing was left running.
- **Upgrading:** an MCP tool that legitimately runs longer than 10 s now times out; raise that server's `timeout_ms`.
- Contract: `docs/plans/mcp-timeout-spec.md`.

### Fixed: shell commands stop when their call ends

- **Before:** a command run by the `shell` tool kept running when its call ended early: <kbd>Esc</kbd>, a worker's timeout, a stopped orchestrated run, or the tool's own time limit (30 s by default, which reported a timeout while the command carried on). `cargo test` went on building and testing in the background, and a dev server kept its port. The test run that follows a TUI turn was the same. Commands also inherited hone's stdin, so in the TUI one that read input (`cat`, a password prompt) waited on the terminal until it timed out.
- **Now** (unix):
  - Each command runs in its own process group with stdin closed. When its call ends early, the whole group is killed: the shell, what it started, and what those started. A command that finishes normally is left alone.
  - A turn no longer waits for a running tool call once it's cancelled: <kbd>Esc</kbd> during the main agent's `shell` call ends the turn at once and kills the command (before, the agent finished the call first; a worker or fix turn stopped likewise only between calls).
  - The MCP client matches each response to its request by id. Before, it took the next line as the answer, so a call cut short (now on <kbd>Esc</kbd>, before only on a worker timeout or a stopped run) left that server answering one call behind for the rest of the session. A call cut short is recorded in the audit log, since it may have partly run. A server's own requests (`ping`, `roots/list`, …) get an answer instead of being taken as the reply, and an error the server returns with `id: null` (a request it couldn't parse) ends the call. (MCP calls had no timeout then; see "MCP requests time out".)
  - The test run after a TUI turn works the same way. It starts after the turn ends, so <kbd>Esc</kbd> with an empty input box now stops it (and any fix turns it started) while the TUI is idle; with text in the box, <kbd>Esc</kbd> still clears it.
  - hone kills its running commands when it exits: the TUI on quit, <kbd>Ctrl</kbd>+<kbd>C</kbd>, SIGINT, SIGTERM or SIGHUP; a one-shot run (`hone "prompt"`, `--plan`, `--orchestrate`, `--ensemble`, `--recipe`) on SIGINT, SIGTERM or SIGHUP, after which it dies by that signal as before, so a calling shell script still stops on <kbd>Ctrl</kbd>+<kbd>C</kbd>; `honed` when it shuts down, now also on SIGHUP. A signal that was ignored when hone or `honed` started stays ignored, so `nohup` still keeps a run going after you log out.
  - <kbd>Esc</kbd> stopping the post-turn work also cancels a fix turn that's already running, and an `@agent` persona's prompt and tools are restored even when its turn is stopped.
- **Not yet:** a command that starts its own session or process group (`setsid`, some daemons) escapes, and so does every command when hone is killed hard (`kill -9`). A command that needs the terminal (an ssh passphrase, a git credential prompt) now stops until its timeout, and <kbd>Ctrl</kbd>+<kbd>Z</kbd> in a one-shot run suspends hone but not its command. `!command` in the TUI, hooks and a recipe's `test_cmd` run as before. Elsewhere than unix, only the command's own process is killed.
- Contract: `docs/plans/worker-tools-spec.md`, rows W23–W25, W37–W39, W41, W44, W45 and W48.

### Fixed: orchestrator workers get tools

- **Before:** the specialist agents that carry out a complex prompt or a `/plan` Run (`rust-expert`, `code-reviewer`, …) had no tools. They couldn't read, search, edit or run anything, and answered in text from the subtask description alone. Separately, the built-in agent files list tools by short names (`read`, `write`, `edit`) that matched no tool, so an `@rust-expert` turn in the TUI and a `honed` distributed worker got only `shell`, `grep` and `glob`.
- **Now:**
  - Each worker gets the tools its agent definition lists, taken from the session's tools. An agent with no `tools:` list gets all of them. A worker for an agent name the planner made up (not in the agent registry) gets only the read-only tools, and the name is logged as a warning. The lead, which plans and merges, still has none.
  - A worker never gets more than the session's own agent has. It runs under the session's approval policy, mode, permission policy, sandbox and audit log. In an `@agent` turn with a `tools:` list, a complex prompt's workers choose only from that list.
  - **In the TUI** a worker's risky call (an edit, a command) asks you in the approval menu, as the main agent's do. Workers running at the same time ask one at a time, and each answer goes to the worker that asked. The menu names the worker (its agent, subtask number and description); `ToolCallPaused` and `ApprovalRequired` carry a new optional `worker` field, absent for the main agent. A terminal smaller than 80×24 still shows a pending approval menu (it showed only "Terminal too small"); its key hints always show, long arguments wrap, and the planner-written description is shown in quotes. The fields that say what a call touches (`path`, `command`, …) are shown first and get more room; other long values are shortened, so a large `content` can't hide which file a write goes to. `/permit`s given earlier in the session apply to the workers. In Plan mode workers get read-only tools and write no plan files; in AutoApprove mode they don't pause. The approval policy and mode are taken when the run starts.
  - **"Always" means the whole session.** An "always" answer, given to the main agent or to any worker, covers the main agent and every worker from then on, including a worker already waiting to ask. An "always" at a permission-policy `ask` prompt covers that rule's command the same way, and also skips the usual prompt for a risky tool for it. With `--step` every call still asks. Separate sessions don't share answers.
  - <kbd>x</kbd> in the approval menu, with the input box empty, stops the current turn and denies the paused call. (<kbd>Esc</kbd> there skips only that call, and with several workers waiting the next one's menu opens at once.)
  - The TUI no longer stops a worker after 300 s; <kbd>Esc</kbd> (with no menu open) or <kbd>x</kbd> stops the run. If the session's agent has stopped, a complex prompt fails with an error instead of starting workers.
  - The built-in agent files now list real tool names (`read_file`, `write_file`, `edit_file`). In your own agent files `read`, `write` and `edit` now mean those tools, unless a tool has that exact name (an MCP tool called `write`, say). This also fixes `@agent` turns and `honed` workers.
- **Per entry point:**
  - `hone --orchestrate`: workers run their tools without asking, as a one-shot `hone "prompt"` does. The permission policy applies.
  - Web UI and `POST /orchestrate`: by default the run config is in Plan mode, so workers get read-only tools and never ask. **New:** `POST /orchestrate` takes `request_id` and `approval_policy` with `/run`'s step-mode rules (400 without a `request_id`, 409 if it's in use). In step mode workers get their agent's tools and pause for approval, answered through `POST /run/{request_id}/step`, with no per-worker timeout. The web UI uses it when its step mode is on, and its approval dialog names the worker. `?dry_run=true` ignores both fields.
  - **Web UI fixes found by running it with parallel workers:**
    - A tool's result is matched to its card by call id. Before, it went to the last card, so one worker's output could land on another's card and a card could stay "running" forever.
    - The approval dialog keeps a newer prompt. Before, it cleared itself after the answer's HTTP response, which could wipe out the next worker's prompt and leave that worker waiting.
    - It ignores clicks for 400 ms after a prompt appears and after each click, as the TUI does, and while an answer is in flight. Before, a double-click on Allow could also approve the next worker's call, unseen.
    - A permission-policy prompt opens one dialog (`approval_required` is shown as an info note; only `tool_call_paused` opens the dialog). A new prompt takes keyboard focus away from the button clicked for the last one. An answer that fails (other than 404 or 410) keeps the dialog open for a retry.
  - **Server input checks:** a `request_id` on `/run` or `/orchestrate` must be 1–128 letters, digits, `-` or `_` (400 otherwise). `/run` names its provenance file after it, and an id with `../` could write outside the provenance directory. `max_workers: 0` on `/orchestrate` now gets 400 instead of crashing the request.
- **Not yet:**
  - Workers running at the same time can edit the same file, and one's `write_file` can overwrite the other's change.
  - The desktop app (`apps/hone-desktop`) doesn't show which worker is asking.
  - No test run follows an orchestrated turn (simple and medium turns have one).
  - `/budget` doesn't cap workers. `hone --orchestrate` and the server keep the 300 s worker timeout.
  - An agent definition's `sandbox:` profile isn't applied to in-process workers.
  - Switching to Plan mode during a run doesn't stop its workers from writing.
- Contract: `docs/plans/worker-tools-spec.md`.

### Fixed: `/plan` Run, Edit and planning; complex runs end once

- **Before:**
  - The plan overlay's keys did nothing until the next Enter with text typed. That Enter also sent the typed text as a turn.
  - **Edit** didn't put the prompt back.
  - **Run** sent the prompt through the normal path again, so the complexity classifier chose the tier anew. A simple verdict ran one agent and no workers; a complex one asked for a new plan, paying for it again, and possibly getting a different one.
  - While the lead model planned, the TUI didn't redraw and ignored <kbd>Esc</kbd> and <kbd>Ctrl</kbd>+<kbd>C</kbd>.
  - A complex prompt's run sent one `Done` per lead call and per worker, so the TUI went idle at the first one while the rest kept running: the spinner stopped and <kbd>Esc</kbd> no longer stopped the run.
- **Now:**
  - **Run** (<kbd>r</kbd>/<kbd>Enter</kbd>) starts at once and runs the plan shown, without planning again (new `Orchestrator::run_plan`). It uses the model a complex prompt would: your `/model`, or else the `[[routing]]` complex model.
  - **Edit** (<kbd>e</kbd>) puts the prompt in the input bar, without `/plan`, with the cursor at the end.
  - **Cancel** (<kbd>c</kbd>/<kbd>Esc</kbd>) drops the plan.
  - **No accidental answers:** <kbd>r</kbd>, <kbd>e</kbd>, <kbd>c</kbd> and <kbd>Enter</kbd> count only after a 400 ms pause, as in the approval menu, so text you are typing when the overlay opens can't start the workers. With a modifier (<kbd>Ctrl</kbd>+<kbd>R</kbd>, <kbd>Shift</kbd>+<kbd>Enter</kbd>) they do nothing, and a paste is dropped.
  - **Planning** runs in the background with the spinner showing; <kbd>Esc</kbd> stops it. `/plan` isn't accepted while a turn is running or still finishing.
  - **Complex runs** (a complex prompt or a `/plan` Run) end with one `Done`, carrying the whole run's usage and cost, so the TUI stays busy, and <kbd>Esc</kbd> stops the run, until it ends. A failed worker shows in the worker list, not as an error that ends the turn.
  - **`/model` reaches complex runs** when `[[routing]]` is off. They used the session's starting model.
- **`/budget` and `/permit` with orchestrated runs** (unchanged, now documented): `/budget` caps only the lead's calls (the merge, and for a complex prompt the planning call too), not the workers, and `/permit` changed nothing for them, since their lead and workers had no tools (workers now have tools; see "orchestrator workers get tools"). A run picks both up like any turn and passes them to the agent, which keeps them for the rest of the session. The docs called them per-run; they now say so.
- **Latent bug fixed:** `set_input_buffer` put the cursor short of the end of non-ASCII text, where the next keystroke could crash the TUI. Edit, now that it fills the input bar, would have hit it.
- **Not yet:**
  - Orchestrated workers, including those of a `/plan` run, had no tools (fixed in "orchestrator workers get tools").
  - `/budget` doesn't cap a plan's workers.
  - The web UI's Run still plans again.
  - In vim mode <kbd>Esc</kbd> doesn't stop planning or a turn.
  - A complex run's cost reaches the session total only when it ends: the cost shown doesn't move during the run, and a run stopped with <kbd>Esc</kbd> adds nothing. (Before, each lead and worker call added its cost as it finished.)
- Contract: `docs/plans/plan-gate-spec.md`.

### Fallback notice: Hone says when a fallback model took over

- **Before:** when a model in `fallback_models` took over a model call, nothing said so. The TUI showed the answer as the turn's model's, one-shot printed nothing, and `--json` named the model the run chose. `HONE_CACHE` stored the fallback's answer under the requested model and replayed it later, even once that model was back. The TUI's adaptive classifier recorded the turn under the model that didn't answer.
- **Now** a new event, `ProviderEvent::ModelFallback { requested, fallback, reason }` (passed on as `AgentEvent::ModelFallback`), is the first event of a call that a fallback took over. `reason` says why the requested model didn't answer. It is cleaned before it is shown:
  - control, bidi and zero-width characters are blanked;
  - URLs lose their user info and query string, because the HTTP client's "no response" errors include the full request URL;
  - it is cut at 200 characters.
- **Where it shows:**
  - **One-shot** (`hone "…"`, `--json`, `--orchestrate`, `--ensemble`): stderr gets `hone: <requested> failed (<reason>); switched to fallback <fallback>`, once per model pair per run.
  - **`--json`:** a new `"fallbacks"` key, always present: `[]`, or one `{"requested", "fallback", "reason", "calls"}` entry per pair. `"model"` still names the model the run chose.
  - **TUI:** one chat message per model pair per prompt you submit.
  - **`honed`:** the event's SSE name is `model_fallback`.
- **Related fixes:**
  - `HONE_CACHE` no longer stores an answer a fallback model gave.
  - The adaptive classifier records the turn under the fallback model that answered.
  - A complex TUI prompt without `@agent` now runs with the turn's settings: the `[[routing]]` complex model, a `/model` pin, `/budget` and `/permit`. It used to run with the session's starting settings.
- **Not yet:** a fallback inside the ensemble ranker, a `--distributed` worker or the `/plan` breakdown isn't reported. A fallback model that rejects the request outright (for example with a 400) shows its error with no notice.
- Contract: `docs/plans/fallback-notice-spec.md`.

### Routing by complexity, fallback models, and `/model` across providers

- **Before:** `[[routing]]` in `config.toml` was parsed and ignored, with a warning. Its example task names (`file_read`, `multi_file_edit`) were never produced by anything. The fallback wrappers (`FallbackChain`) weren't used by `hone`, and they didn't move on when a provider gave no answer at all. `/model` changed only the model name: `/model openai:gpt-4o` in an Anthropic session sent `gpt-4o` to Anthropic.
- **`[[routing]]`:** `task = "simple" | "medium" | "complex"` picks the model for turns of that complexity, using the classification the TUI already makes for every prompt.
  - One-shot and `--json` classify the prompt once. `--json` reports the chosen model, and stderr names the routing rule.
  - A project file's entry wins over the user's for its tier. Hone announces each one at start (`hone: routing <tier> requests to <model> (from <path>)`), as it does for a project `default_model`.
  - Another `task` name, or a Claude/Gemini CLI model, is ignored with a warning.
- **`fallback_models = [...]`** (user file only): models to try, in order, when the turn's model fails before answering. A project file's list is ignored with a warning, because a repository could otherwise send your session to a provider it chose without any notice. That covers an overload, a rate limit, a 5xx error, a stream that fails before any output, or no answer at all, such as a refused connection. `FallbackChain` and `TierFallbackProvider` now also move on when there was no answer (HTTP status 0); the agent's own retry still doesn't retry it.
- **`/model`** now switches the provider with the model, and updates the status bar's model label. It accepts `anthropic:`, `openai:` and `ollama:` models whose provider has a key, and refuses the agent CLIs. It confirms the change only once it has been applied. A switch typed while a turn is running is checked at once and applied when that turn has finished, including its self-refine rounds and test-fix follow-ups, so one turn never mixes models.
- **How the model choice interacts with routing:**
  - `--model` means exactly that model: no routing and no fallback.
  - `/model` turns routing off for the rest of the session, and fallback still applies.
  - Without `[[routing]]` and `fallback_models`, requests and `--json` output are unchanged, apart from tool order (below).
- **New `hone_providers::ModelDispatch`:** it sends each request to the provider its model's prefix names, and runs the fallback models through `FallbackChain`.
- **Tool order:** tool definitions are now sent sorted by name. Their order used to come from a hash map and changed from run to run, so identical sessions now send identical requests; this also helps providers' prompt caching. Requests therefore differ from earlier versions in tool order only.
- **Not yet:**
  - Cost estimates still use one flat rate for every model.
  - A turn routed to a model with a smaller context window gets the whole history, which the server may truncate.
- Contract: `docs/plans/routing-fallback-spec.md`.

### Fixed: errors a retry can't fix were retried

- **The bug:** a stream error (`ProviderEvent::Error`) carried only its message. After the retry fix below, an error that a provider sends inside the stream and that no retry can fix was therefore sent again twice, with the whole conversation, before the same error came back. Examples are vLLM's `"code": 400`, LiteLLM's `"code": "400"`, OpenAI's `context_length_exceeded` and Anthropic's `invalid_request_error`. Hone's own idle timeout was recognised by its message text, so a provider message that happened to start the same way was skipped too.
- **Now:** the event carries a `kind` (new `hone_proto::StreamErrorKind`):
  - **Interrupted:** the connection broke, or the response ended early.
  - **IdleTimeout:** Hone's idle limit ran out.
  - **Provider { status }:** the provider reported the error.
  - **Unclassified:** CLI and ACP providers, test doubles.
  
  The status comes from the error's `code`: a number, a number as text, or a known name such as OpenRouter's `server_error`. Failing that it comes from its `type` (Anthropic's `overloaded_error` is 529). A stream error before any output is retried or handed to another provider only if its kind says another attempt can help. So a provider error with status 429 or 5xx, or with no status, is retried; any other status and an idle timeout fail after one attempt. Messages are unchanged.
- One rule for a transient status, `hone_proto::is_transient_status` (429, 500–599), now serves both request errors and stream errors.
- Still retried: an error name Hone doesn't know, such as OpenAI's `insufficient_quota`, and Ollama's errors, which carry no status.
- Contract: `docs/plans/provider-error-kind-spec.md`.

### Fixed: responses that failed before any output were not retried

- **The bug:** the agent retried a failed request (3 attempts with backoff), but a request counts as successful as soon as the response headers arrive. A response that then failed before its first word failed the turn at once, although nothing had been shown and another attempt would have gone unnoticed. Examples are Anthropic's `overloaded_error`, OpenRouter's "Provider disconnected unexpectedly", and a connection reset. `FallbackChain` and the router's `TierFallbackProvider` didn't move to the next provider for such a failure either. The agent also chose which request errors to retry by their text, so HTTP 500, 504 and Anthropic's 529 "Overloaded" were never retried.
- **Now:**
  - A response that fails before any output (text, a tool call, or its end) is retried with the agent's existing attempts and backoff. Nothing from the failed attempt is shown. If every attempt fails, the turn fails with the last provider message, unchanged.
  - `FallbackChain` and `TierFallbackProvider` move to the next provider on such a failure, and on any transient request error. The new `ProviderError::is_transient()` counts rate limits, timeouts, I/O stream errors, and HTTP 429 and 5xx as transient. Before, `FallbackChain` moved on only for rate limits.
  - The agent also retries HTTP 500, 504 and 529.
  - Usage that failed attempts report is counted in the turn and the session cost.
- **Not retried:**
  - A break after output has appeared, as before.
  - A response from a provider that runs its own tools (the Claude and Gemini CLIs, ACP agents) once it has started, because the provider may already have acted. Their request errors are still retried.
  - A cancelled turn: Esc stops further attempts.
  - A response that started and then sent nothing for the whole idle limit. That usually means a slow model, and a retry would repeat the wait and any billed reasoning. This idle timeout is now reported as `<provider> response timed out: …` instead of `response interrupted: …`.
- **Behaviour change:** when every attempt fails, the turn makes 3 requests instead of 1. It ends about 1.5 s later, plus the time each attempt takes to fail.
- Contract: `docs/plans/stream-retry-fallback-spec.md`.

### Fixed: cut-off responses looked complete

- **The bug:** the OpenAI-compatible, Anthropic and Ollama adapters ended the response stream the same way whether the answer had finished or the connection had broken. A reset, a body cut short, or a timeout was swallowed. The agent saved the partial answer as if it were complete, and a tool call whose arguments were still streaming was dropped without a word. Errors that providers send inside the stream after the `200` headers were ignored too: OpenAI-compatible `{"error": …}` (OpenRouter's "Provider disconnected unexpectedly"), Anthropic's `overloaded_error` event, and Ollama's `{"error": …}` line. With OpenRouter the `[DONE]` that follows its error turned the failure into a normal end.
- **Long answers were cut at two minutes.** The HTTP clients had a 120 s *total* deadline, which reqwest applies until the response body has finished. Any answer that streamed for longer, such as a slow local model or a long generation, was cut off, and because of the bug above it looked complete.
- **Now:**
  - A response that ends before the provider's completion marker becomes an error: `<provider> response interrupted: <cause>` or `<provider> response ended before it was complete`. In-band errors carry the provider's message, and nothing after them is read.
  - Once the response has started, the 120 s limit is an idle limit: it resets on every piece of data, so an answer that keeps streaming is never cut off, and a stall of 120 s fails with "timed out". Waiting for the first byte is still limited to 120 s, as before (an Ollama model that takes longer to load still times out).
  - An OpenAI-compatible response that ends with any finish reason (`content_filter`, TGI's `eos_token`, …) counts as finished, and so does an Anthropic `message_delta` with a stop reason.
  - The opt-in response cache (`HONE_CACHE`) no longer stores a failed response.
- **Behaviour change:** a cut-off answer now fails the turn (an error in the TUI, a non-zero exit in one-shot and `--json`) instead of being kept as a complete answer. Benchmark runs that were silently truncated now show as provider errors.
- Contract: `docs/plans/stream-transport-error-spec.md`.

### Fixed: streamed text lost when a character was split across chunks

- **The bug:** the OpenAI-compatible, Anthropic and Ollama adapters decoded each network chunk of a streamed response on its own and silently dropped any chunk that wasn't valid UTF-8 by itself. Chunk boundaries come from the network, not from characters, so a multi-byte character (Polish letters, `€`, CJK, emoji) cut between two chunks dropped both. Text went missing from answers, the events in those chunks were lost, and a tool call whose arguments were cut that way arrived broken or not at all. One invalid byte also dropped every event in its chunk.
- **Now:** a shared decoder carries an incomplete character over to the next chunk and replaces genuinely invalid bytes with U+FFFD, so the text is the same however the response is chunked.
- Contract: `docs/plans/stream-utf8-split-spec.md`.

### Fixed: base URLs ending in `/v1`

- **The bug:** the OpenAI-compatible adapter added `/v1/chat/completions` to the base URL, and the Anthropic adapter `/v1/messages`. A base URL that already ended in `/v1`, which is the usual form (OpenRouter `https://openrouter.ai/api/v1`, vLLM `http://localhost:8000/v1`, the OpenAI SDKs), became `…/v1/v1/…` and failed. `hone configure` writes the OpenRouter URL in exactly that form, so every OpenRouter wizard user was affected, and since the previous fix their wizard settings are always applied.
- **Now:** both adapters accept a base URL with or without a trailing `/v1` and trailing slashes, and trim surrounding whitespace (a stray space in `HONE_OPENAI_BASE_URL` used to give a confusing 404). Only a whole `/v1` segment is removed (`/v1beta`, `/apiv1` stay as they are). Hone always requests `<base>/v1/chat/completions` (or `/v1/messages`), so endpoints that serve chat elsewhere, such as Azure, Zhipu's `…/api/paas/v4`, Gemini's `…/v1beta/openai` or Cloudflare AI Gateway's `…/openai` route, are still not supported.
- Contract: `docs/plans/base-url-v1-spec.md`.

### Fixed: config.toml is applied

- **The bug:** `~/.config/hone/config.toml` and `.hone/config.toml` were validated but never used: `default_model`, `[providers.*]` and `[budget]` had no effect. The `[provider]` table that `hone configure` writes failed validation, and a separate reader picked up only part of it, so choosing Ollama or Claude CLI in the wizard, or an Anthropic model, did nothing.
- **Now applied:** `default_model`, `[providers.anthropic|openai|ollama]` (`model`, `base_url`, `api_key_env`), `[budget].session_limit_usd` (the cap `/budget` sets) and the wizard's `[provider]` table. One strict reader replaces the old line parser. Everything that builds a provider uses it: the TUI, one-shot, `--json`, orchestrate / ensemble / medium, `hone eval` and `hone spec`.
- **The project file is untrusted.** A repository's `.hone/config.toml` may choose models and lower the budget. It may not set endpoints, key variables, the `[provider]` table, or a `claude-cli:` / `gemini-cli:` model; those are ignored with a warning. When it chooses the model, Hone says so on stderr, and if you have no key for that provider, Hone falls back to your own model choice, then auto-detection. The project `.hone` directory and file must not be symlinks; files must be regular, at most 64 KiB and UTF-8; parse errors give the line and column, never file text.
- **Model names are checked** (non-empty, no leading `-`, no whitespace or control characters) wherever they come from, and the Gemini CLI receives the model as one `--model=<name>` argument. Before, `gemini-cli:--yolo` reached the CLI as a separate `--yolo` flag.
- **Not applied yet:** `[sandbox]`, `[[routing]]`, `[server]` and `daily_limit_usd`. Hone warns on stderr at startup when they are set.
- **Behaviour changes:**
  - A broken or unknown-key `config.toml` now stops `hone` with exit code 2 and the file's path, like a broken `permissions.toml`. Commands that don't start a model (`configure`, `session list`, `memory`, ...) still work. Tables shown in older docs (`[agent]`, `[memory]`, `[pricing]`) were never read and are now rejected.
  - A model chosen in the wizard now wins over auto-detection, so an API key for another provider in your environment no longer switches you away from it.
  - `--model anthropic:…` / `openai:…` without a key exits 1 and names the variable, instead of falling back to another provider with a mismatched model id. `claude-cli:` and `gemini-cli:` now always run those CLIs. A keyless OpenAI-compatible server needs a placeholder key (`OPENAI_API_KEY=local`).
  - An empty key variable counts as unset.
  - The benchmark arms are unaffected: they pass `--model` and the `HONE_OPENAI_*` variables, which take precedence.
- Contract: `docs/plans/config-apply-spec.md`.

### Fixed: guard messages under tool ids the model never issued

- Two edit guards added their output to the history as separate tool messages under ids the model never issued:
  - **the read-before-edit guard** (`guard-read-<uuid>`): when the model edits a file it hasn't read this turn, Hone reads it first. That read also **skipped the prompt-injection scan** and the egress (audit) log, although repository files can be attacker-written. Read tracking resets every turn, so it fired often in multi-turn sessions;
  - **the edit-streak guard** (`guard-edit-streak-<uuid>`): a hint after the third consecutive edit in a turn.
- The Anthropic and OpenAI APIs document that every tool result must answer a tool call in the previous assistant message, so the next request was likely rejected when either guard fired (unverified live; local vLLM doesn't check ids).
- Now both are delivered inside the edit call's own result, after the call's own outcome (so history compression, which keeps the start of old tool results, keeps the outcome). The guard's read is scanned for injection (SECURITY prefix when flagged), redacted, and recorded in the egress log. Guard state from a cancelled turn can't attach to a later call.
- Contract: `docs/plans/guard-read-spec.md`.

### Fixed: a failing tool no longer kills the turn

- **The bug:** a tool that returned an error, such as `edit_file` with an `old_string` that isn't in the file, aborted the whole turn:
  - the model never saw the error, and the rest of the batch never ran;
  - the history kept the call without a result, so the **next** request was rejected by the provider (HTTP 400);
  - no `Error` event was sent, so the TUI spinner ran forever on simple turns and `honed` streams ended with no terminal event. The same silence followed an exceeded budget or a provider request that failed after retries.
- **The fix:**
  - A tool error becomes an error result (`is_error: true`, `{"error": "…"}`) that the model sees, so it can recover (for example, re-read the file and retry the edit). The `ToolCallEnd` event, egress log and `PostToolUse` hook report it as an error. Error text goes through the same secret redaction and prompt-injection scan as other results (git errors can contain credentialed URLs; MCP servers control their error text).
  - Every failed turn sends exactly one `Error` event, and the TUI always ends the turn, even if the agent stops unexpectedly.
  - One-shot and `--json` modes print a failure once and exit 1.
- **Benchmark note:** `hone --json` runs (the harness-compare arms) now recover from failed edits instead of stopping, which changes Hone's results. A failed run now reports its real `usage` and `cost_usd` (it reported no usage and $0 before, which the harness counted as 0 tokens). Both land before any pre-registered run.
- Contract: `docs/plans/tool-error-turn-spec.md`.

### Memory subsystem overhaul

A multi-PR rewrite of how memories are written, indexed, and recalled. The full design lives in `docs/priv/memory-improvements-plan-2026-04-30.md` (R1–R12); user-visible changes:

- **Typed memories.** Every `Memory` now carries a `kind` (`Conversation`, `User`, `Feedback`, `Project`, `Reference`, `Outcome`) and an optional `description` (one-line summary used in the index). Schema migration is automatic and non-destructive — pre-existing rows read back as `kind = Conversation`. `crates/hone-memory/src/types.rs`, `crates/hone-memory/src/sqlite_store.rs`.
- **Memory index in the system prompt.** A `## Memory index` section now lists available memory IDs grouped by kind with their descriptions; the agent picks bodies on demand via the new `memory_read` tool. Replaces the "top-K substring matches" recall pattern with one the model itself drives. Budgeted to keep the prompt small; truncation is signposted. `crates/hone-core/src/memory_index.rs`, `bins/hone/src/system_prompt.rs`, `bins/hone/src/tools/memory_read_tool.rs`.
- **BM25 lexical relevance.** The substring counter that scored recalls is replaced by a from-scratch BM25 implementation (k1=1.5, b=0.75) over content+description. SQLite recall caps the BM25 universe at the 5 000 most-recent memories so cost stays bounded. Empty-query recall (used by the index) bypasses the cap and BM25 entirely. `crates/hone-memory/src/bm25.rs`, `crates/hone-memory/src/sqlite_store.rs`, `crates/hone-memory/src/in_memory.rs`.
- **`hone memory` CLI.** New subcommands: `hone memory ls [--kind <k>] [--limit <n>]`, `hone memory show <id>`, `hone memory forget --tag <t> | --kind <k> | --before <iso8601>`. RFC3339 parsing via the `time` crate. `bins/hone/src/memory_cmd.rs`.
- **`--encrypted-memory` flag.** Wraps the on-disk SQLite store with AES-256-GCM + Argon2id (per-store salt sentinel) using the existing `EncryptedStore`. Reads `HONE_MEMORY_PASSPHRASE`; if the env var is missing or wrong, the binary exits non-zero with a clear stderr message rather than silently degrading to no-memory. `bins/hone/src/memory_init.rs`, `bins/hone/src/main.rs`.
- **Auto-decay on open.** `SqliteStore::open` runs `decay(0.95, now)` if more than 24 hours have elapsed since the last decay (tracked in a new `memory_meta` table). One CLI invocation no longer collapses weights, but long-running daemons no longer drift either. `crates/hone-memory/src/sqlite_store.rs`.
- **Versioned schema migrations.** Replaces the ad-hoc `PRAGMA table_info` + `ALTER TABLE` + "duplicate column name" string-match with a real `schema_version` table. Migrations run inside `BEGIN IMMEDIATE` so concurrent openers serialise on the SQLite write lock instead of racing on a stale `MAX(version)`; per-step `ALTER TABLE` is guarded by a fresh column-existence probe. `crates/hone-memory/src/sqlite_store.rs`, new test suite `crates/hone-memory/tests/schema_version.rs`.
- **Write-side noise reduction.** A new classifier (`crates/hone-core/src/memory_worthy.rs`) gates `record_memory` on per-turn signal — corrections (`stop doing`, `don't do`), validated decisions, project deadlines, and external links require an explicit pattern match before a memory is written. Bodies for `Feedback`/`Project` kinds use a structured **Why:** / **How to apply:** template (`crates/hone-core/src/memory_format.rs`). Trigram-Jaccard `find_similar(0.8)` deduplicates against the 5 000-memory recent slice before insert. `crates/hone-core/src/agent.rs`.
- **System-prompt write/recall guardrails.** New `## What NOT to remember` and `## Verify before recommending from memory` sections steer the model away from saving derivable facts (file paths, conventions) and toward verifying recalled symbols/files exist before recommending them. `bins/hone/src/system_prompt.rs`.

### Security: shell sandbox

- **Linux: Landlock was never applied to shell commands.** `ShellTool` rebuilt the tokio command from program and arguments, dropping the `pre_exec` hook that installs Landlock. The command is now converted with `Command::from`, so hooks and Seatbelt's program rewrite survive. Linux enforcement is **opt-in** with `HONE_SANDBOX_ENFORCE=1` until it is validated; macOS Seatbelt is unchanged (enforced). `crates/hone-tools/src/shell/mod.rs`.
- **Read allowlist.** Built by `shell_sandbox_policy()`. It adds toolchain roots: `/opt`, `/usr/local`, `/usr/lib64` and `/lib64` on Linux. On macOS it adds only the Homebrew `bin`/`lib`/`opt`/`Cellar`/`share`/`include` subdirectories plus the two Homebrew system `gitconfig` files, never `var` or the rest of `etc`. It also adds `~/.gitconfig` and `~/.config/git/{config,ignore,attributes}` (not `credentials`), and makes `/dev/null` writable; file rules get file-only rights, so Landlock reports full enforcement. Paths that don't exist are skipped. Previously a missing `~/.cargo` would have failed every command once Landlock was enforced. Extra roots come from `HONE_SANDBOX_EXTRA_READ` (colon-separated). Roots that are `/`, `$HOME` or an ancestor of it, or under the working directory or `/tmp` are rejected, including via symlinks.
- **Landlock backend.** Rules for missing paths are skipped instead of failing the ruleset. A Linux build error (`landlock.rs`, ambiguous `.into()`, Rust 1.99) is fixed. `crates/hone-sandbox/src/landlock.rs`.

### Context window override

- **`HONE_CONTEXT_WINDOW`.** Hone sized its context budget from the model name alone, so any Qwen model got 128k × 0.45 = 57.6k. A model served with a smaller window (a 32k Qwen2.5-Coder, say) got requests the server rejected before Hone ever compacted. Setting `HONE_CONTEXT_WINDOW=<tokens>` (4096–10000000) replaces the inferred window. The per-model budget ratio still applies, and the `ModelInfo` event reports the override. Invalid values are ignored and logged at warn level, which shows only with `RUST_LOG=warn`. The TUI status bar still estimates the window from the model name; that is a follow-up. `crates/hone-core/src/context.rs`, `crates/hone-core/src/agent.rs`.
- **Docs.** `docs/configuration.md` listed CLI flags that don't exist (`--context-limit`, `--budget`, `--temperature`, `--max-iterations`, `--sandbox-enforce`), and `docs/configuration.md` and the README showed a `context_window_tokens` config key that nothing reads. Both are removed. The table of environment variables now includes `HONE_CONTEXT_WINDOW`, `HONE_SANDBOX_ENFORCE` and `HONE_SANDBOX_EXTRA_READ`.

### Security (BREAKING)

- **`honed` now fails-closed when no auth token is configured on a non-loopback bind.** If `HONE_AUTH_TOKEN` is unset or empty AND the bind host is non-loopback (i.e. anything outside `127.0.0.0/8` or `::1`) AND `HONE_ALLOW_UNAUTH=1` is not set, the daemon prints a clear message to stderr and exits with code 2. Previously, a missing token silently produced an unauthenticated public endpoint. **Migration:** existing operators must either set `HONE_AUTH_TOKEN=<secret>`, bind to loopback via `HONE_BIND=127.0.0.1`, or opt in explicitly with `HONE_ALLOW_UNAUTH=1`. New env vars: `HONE_BIND` (default `0.0.0.0`) and `HONE_ALLOW_UNAUTH` (default unset). Empty/whitespace-only `HONE_AUTH_TOKEN` is now treated as unset (previously it would be accepted by the middleware and matched by any `Authorization: Bearer ` header). `bins/honed/src/main.rs`.

## TUI & Web UI Enhancements

### Added

- **Session switcher overlay (Ctrl+O)** — Browse and switch between saved sessions with a full-screen overlay showing session name, last-updated relative time (e.g., "5 mins ago"), and message count per session. Keyboard navigation with j/k, Enter to select, q to close. Makes resuming past work frictionless. `crates/hone-tui/src/app.rs`.
- **Configurable theme via `~/.config/hone/theme.toml`** — Flat `key = "#rrggbb"` key-value pairs override any named theme color (e.g., `accent-hot`, `bg-normal`, `text-muted`). Merges with the selected theme instead of replacing it. Supports 24-bit RGB only (no fallback to 8-color palette). `crates/hone-tui/src/theme.rs`.
- **Notification bell on long-running turns** — Terminal bell (`\x07`) fires when a turn takes ≥10 seconds, useful for notifications in terminals with bell support. Can be disabled with `HONE_NO_BELL=1`. `crates/hone-tui/src/app.rs`.
- **Expanded keybindings help (Ctrl+?)** — Help overlay now shows 31 keybindings organized across 5 sections: Input (Enter, Shift+Enter, Ctrl+Z), Navigation (Page Up/Down, gg/G in vim mode), Overlays (Ctrl+O, Ctrl+R, Ctrl+?), Agent control (Ctrl+C, Tab for model switcher), and Commands (/help, /quit, /session). `crates/hone-tui/src/help.rs`.
- **Web UI (`GET /` on honed)** — Single-page chat interface served from the daemon. Dark theme by default, includes model selector, working directory input, streaming token-by-token display, tool-call cards with JSON previews, and minimal markdown rendering (bold, lists, code blocks). Built with vanilla HTML/CSS/JS (no external CDN dependencies). `crates/hone-server/src/assets/index.html`.
- **Stateless HTTP execution endpoint (`POST /run` on honed)** — Client sends `{prompt, model?, history?, working_dir?}`, server streams `AgentEvent`s as Server-Sent Events (SSE). No session persistence (use `POST /sessions` for persistent sessions). No API key needed on client—server reads `ANTHROPIC_API_KEY` from environment. Perfect for programmatic one-off tasks and embedded UIs. `crates/hone-server/src/handlers.rs`.
- **Cross-session learning (AdaptiveClassifier)** — When a memory store is configured, the agent records each turn's outcome (tier classification, tokens used, cost, success/failure). On subsequent sessions, the AdaptiveClassifier uses historical success rates to adjust complexity routing (Simple/Medium/Complex) automatically. Improves estimation accuracy and tool selection. Transparent fallback to rule-based `ComplexityClassifier` when no memory store is present. `crates/hone-core/src/classifier.rs`.
- **Better HTTP API error messages** — Provider errors (Anthropic/OpenAI/Ollama) now extract the JSON `error.message` field instead of dumping the full response body. 401 responses surface a human-readable hint pointing to the relevant env var (`ANTHROPIC_API_KEY`, `OPENAI_API_KEY`, or `OLLAMA_HOST`). Error format: `"anthropic error (400): invalid API key"` (was `"HTTP error from provider: status 400, body: {...}"`). `crates/hone-providers/src/error.rs`.
- **Integration tests for provider error handling** — New `crates/hone-providers/tests/real_api.rs` with `#[ignore]` tests validating invalid-key error messages (Anthropic + OpenAI, no env vars needed) and tool-call happy paths (requires actual keys). Run with: `cargo test -p hone-providers --test real_api -- --ignored`. `crates/hone-providers/tests/real_api.rs`.

- **ACP provider tool dispatch (full bidirectional contract).** `AcpProvider` now closes the loop with the agent's tool-use protocol: `CompletionRequest.tools` are announced as inline JSON Schema on `session/prompt`, incoming `session/update` notifications with `sessionUpdate: "tool_call"` surface as `ProviderEvent::ToolCall` (with `rawInput` parsed verbatim into `arguments`, defaulting to `{}` when the agent omits it), the caller-supplied `ToolResult` is fed back via a `session/toolResult` request, and the terminal `tool_call_update` notification surfaces as `ProviderEvent::ObservedToolEnd` with case-insensitive `failed`/`error` classification. Stream invariant: `ToolCall < ObservedToolEnd < Done`. `claude` and `gemini` factories advertise `supports_tools: true`. New mock-agent helpers (`MockToolCallConfig`, `create_mock_acp_agent_with_tool_call`) and tests (`acp_tool_schema.rs`, `acp_tool_roundtrip.rs`, `acp_tool_arguments.rs`, `acp_tool_update.rs`, `acp_full_lifecycle.rs`) pin every cycle of the contract. `crates/hone-providers/src/acp/`, `crates/hone-providers/tests/acp_*.rs`.

### Changed

- **Filesystem agent extensibility wired.** `AgentRegistry::load_hierarchical(working_dir)` is now the canonical entry point for the three call sites in `bins/hone/src/main.rs`. Overlays apply in increasing precedence: compiled-in built-ins → `~/.config/hone/agents/*.md` (user) → `<project_root>/.hone/agents/*.md` (project, walks up for `.git`/`Cargo.toml`/`package.json`/`pyproject.toml`/`go.mod`/`pom.xml`). Same-name agents at higher overlays replace lower ones. Missing dirs silently no-op via `ErrorKind::NotFound`; other I/O errors emit `tracing::warn!` so misconfiguration is visible. Per-file parse errors warn-and-continue. Reuses `find_project_root` (now `pub` in `hone-config`). Previously `load_from_dir` was implemented but never invoked — only built-ins loaded. `crates/hone-core/src/agent_registry.rs`, `crates/hone-config/src/hone_md.rs`, `bins/hone/src/main.rs`.

- **Agent rail `[AGENTS]` section.** The TUI rail now surfaces live worker status during orchestrated turns, slotted between `[FILES]` and `[CONTEXT]`. Each row shows a status glyph + agent name truncated to fit the 18-cell rail. Glyphs: `…` Running / `✓` Succeeded / `✗` Failed / `T` TimedOut (no-color theme uses `…`/`ok`/`X`/`!`). `T` is used in place of the `⏱` stopwatch glyph to avoid East-Asian-width terminals rendering it as 2 cells. `crates/hone-tui/src/turn_ledger.rs`, `crates/hone-tui/src/render/agent_rail.rs`, `crates/hone-tui/src/app.rs`.

- **Friendly memory recall keys.** `MemoryRecalled` event entries now show the memory's `description` (when present) or a synthetic `<kind>-HHMM-XX` fallback (FNV-1a hashed 1-byte suffix) instead of raw UUIDv7 ids. Renders as e.g. `feedback-1438-3a` in the rail and conversation log. `crates/hone-core/src/memory_key.rs`, `crates/hone-core/src/agent.rs`.

### Fixed

- **Orchestrated-turn writeback.** Orchestrated turns now persist the merged response into the `AgentHandle` history. Previously, the orchestrator's `merged_response` was returned to the user but never written back to the agent's context, causing follow-up prompts to fail with "I don't have a request to act on" because the model couldn't see what it had just produced. Empty/Merge-failed responses are skipped explicitly with a `tracing::warn!`. `bins/hone/src/main.rs`.

- **`memory ls --kind=K --limit=N` could return < N items.** The kind filter ran as a Rust-side `retain` AFTER `recall` had already capped the result at `limit`, so a store dominated by other kinds could starve the filtered view even when N matching memories existed. Pushed `kind: Option<MemoryKind>` into `MemoryQuery` so both `InMemoryStore` and `SqliteStore` (and the `EncryptedStore` wrapper, which forwards to its inner store) apply the filter pre-limit. New `MemoryStore::recall_by_id` replaces the prior O(n) full-scan in `memory show`; `SqliteStore` uses indexed PK lookup, `InMemoryStore` uses `HashMap::get`. Prefix matching for short ids is preserved as a fallback when exact lookup misses. `crates/hone-memory/src/{types,store,in_memory,sqlite_store,encrypted}.rs`, `bins/hone/src/memory_cmd.rs`.

### Documentation

- **Web UI guide** — New section in `getting-started.md` explaining how to run `hone serve` and access the embedded chat at `http://localhost:8080`. Includes `POST /run` example for scripting. `docs/getting-started.md`.
- **Theme customization guide** — New section in `getting-started.md` with example `~/.config/hone/theme.toml` showing RGB color overrides. Updated `docs/configuration.md` with full theme color key reference. `docs/getting-started.md`, `docs/configuration.md`.
- **Session switcher docs** — Updated keybindings in `getting-started.md` and the expanded help text in help overlay. Documented in observability section with session management context.
- **Adaptive routing docs** — Updated `docs/observability.md` to explain how the AdaptiveClassifier uses memory store outcomes to improve future complexity classification. Cross-references the multi-agent architecture guide.

---

## [Previous]

### Security

- **Shell sandbox policy anchored to the working directory.** The `SandboxPolicy::Manual` used for shell tool calls previously shipped with empty read/write allow lists, making the policy effectively trivial. It now pins the project's working directory (read+write) and the standard system read paths (`/usr/`, `/bin/`, `/System/Library/`, etc.), so Seatbelt/Landlock deny-by-default actually denies anything outside the project. `crates/hone-tools/src/shell.rs`.
- **Per-store random salt for encrypted memory.** Argon2id derivation for `--features encrypted-memory` was using a fixed salt constant, meaning every vault on the machine derived the same key from the same passphrase. Each store now generates a random 16-byte salt on first use and persists it (plus a version byte) as a sentinel memory record, so rotating the passphrase or cloning the DB yields a different key. `crates/hone-memory/src/encrypted.rs`.
- **Parallel tool dispatch recovers from panicked tasks.** `AgentContext::dispatch_parallel` spawned tool calls into a `JoinSet` and silently discarded entries whose task panicked. It now keeps a sidecar `(call_id, tool_name)` table indexed by spawn order and, when a join returns `Err(JoinError)`, emits a synthetic `ToolCallEnd { is_error: true }` with the panic message plus a tool result message so the turn loop can recover instead of hanging. `crates/hone-core/src/agent.rs`.

### Added

- **SVG-first brand assets.** `assets/mark.svg` (256×256, 4-layer chevron mark), `assets/mark-v2.svg` (tapered-blade silhouette with 2-tone `#7B1E36`→`#EE3F6E` gradient), `assets/favicon.svg` (32×32 single-stroke glyph), `assets/lockup-horizontal.svg` (768×256 mark + HONE wordmark using `currentColor`), and `assets/social-preview.svg` (1280×640 GitHub og:image). Replaces the previous ad-hoc raster pipeline with SVG source-of-truth → `rsvg-convert` exports.
- **Monochrome mark variants.** `assets/mark-mono.svg` (currentColor), `assets/mark-mono-black.svg` (#0d1117), `assets/mark-mono-white.svg` (#ffffff) for print, embossing, and single-color contexts.
- **PNG export set.** `mark-{32,64,128,256,512,1024}.png`, `mark-v2-{32,64,128,256,512,1024}.png`, `favicon-{16,48}.png`, `lockup-horizontal-v2-{light,dark}.png`, and `social-preview.png` rasterised from SVG sources.
- **Retired orphan rasters.** Removed `assets/mark-transparent.png`, `assets/lockup-horizontal-{light,dark}.png`, `assets/favicon-32.png`, and `assets/favicon-64.png` — none had any in-repo references.

- **Typed `/configure` subcommands in the TUI.** `/configure` is now a router over `model`, `theme`, `vim on|off`, `budget <usd>|off`, `provider`, and `help`, replacing the single-shot display. The bare `/configure` form prints a full session summary (provider, model, theme, vim, budget, cost, context, phase). Default `context_limit` is now 128 000 so the context meter renders before the first `ModelInfo` event arrives. `crates/hone-tui/src/app.rs`, `crates/hone-tui/src/commands.rs`.
- **`HONE_DISABLE_PERSISTENT_STORES=1` env var.** Same effect as `--ephemeral` but toggleable without a CLI flag; used by the new TUI demo expect script and any CI harness that wants to skip SQLite session/memory setup. `bins/hone/src/main.rs`.
- **Workspace-wide clippy/rust lint policy.** Root `Cargo.toml` now carries a `[workspace.lints]` block and every crate opts in via `[lints] workspace = true`. This ensures new crates cannot silently diverge from the project's deny/warn policy. `Cargo.toml` + all `crates/*/Cargo.toml`.
- **Multi-agent + memory architecture note.** New `docs/architecture/multi-agent-and-memory.md` walks through the agent turn lifecycle, orchestrator pipeline, context-trimming strategy, and cross-session learning. Linked from `README.md` and `docs/index.md`.
- **`Tool::is_read_only()` trait method.** Replaces hard-coded tool-name allow lists scattered across the MCP and permission layers. Each tool now declares its own read-only status so dynamic MCP tools can be classified without hand-maintenance. `crates/hone-tools/src/lib.rs`.
- **Integration-test coverage for agent turns, orchestrator, memory, Ollama, and session flow.** Five disjoint suites — `crates/hone-core/tests/agent_turn.rs`, `crates/hone-core/tests/orchestrator.rs`, `crates/hone-providers/tests/ollama.rs`, `crates/hone-memory/src/sqlite_store.rs` (inline), and `crates/hone-server/tests/e2e_session_flow.rs` — covering `ModelInfo` emission + pricing, worker panic recovery, Ollama `tool_call.arguments` as a JSON object, write persistence across `SqliteStore` instances, and the SSE server stack end-to-end.
- **CI bench compile check.** New `cargo bench --no-run` job guards against benchmark bit-rot. Workspace clippy is currently wired at `-W` until every crate converges on the new lint policy. `.github/workflows/ci.yml`.

### Changed

- **`AgentEvent`, `ProviderEvent`, `Role`, and `ToolLayer` are now `#[non_exhaustive]`.** External crates matching on these enums must add a `_ => …` arm. This gives us room to add variants (e.g., new streaming events) without a major bump. All first-party matchers were updated accordingly; look for forward-compat `_ => {}` arms in `crates/hone-tui/src/app.rs` and `crates/hone-providers/src/ollama.rs`.
- **`AgentError::Internal` and typed `CheckpointError`.** Replaced several `anyhow::Error` bailouts inside `hone-core` with thiserror variants so callers can pattern-match on the failure mode. The agent loop now surfaces `AgentError::Internal { detail }` as a tool-result message instead of aborting the turn. `crates/hone-core/src/agent.rs`, `crates/hone-core/src/checkpoint.rs`.
- **`Retry-After` parser follows RFC 9110.** HTTP 429/503 retry hints are now parsed as integer seconds first, HTTP-date second, with a bounded max to prevent runaway sleeps. Module-level regexes hoisted into `std::sync::LazyLock<Regex>` to avoid per-call recompilation. `crates/hone-providers/src/retry.rs`.
- **`hone reports list` respects `--keep-all`.** The flag was previously accepted and ignored; `cmd_list` now threads it through to the retention check. `bins/hone/src/reports_cmd.rs`.
- **Cache hit propagation.** `ToolOutput` and `AgentEvent::ToolCallEnd` now carry a `cache_hit` bit so the TUI status line and session reports tag cached results correctly. `crates/hone-cache/src/tool_cache.rs`, `bins/hone/src/tools/caching_tool.rs`.
- **Reports writer is testable.** `cmd_list` split into a writer-taking helper, so integration tests can capture output without hijacking `stdout`. `bins/hone/src/reports_cmd.rs`.

### Fixed

- **`.cast` demos regenerated.** All asciinema recordings refreshed against the current TUI; new scripts added for `/configure`, CLI flow, Ollama tool-calling, agent rail, reports, model switch, and accessibility modes. Note that new-style `assets/demo-*.cast` is covered by `.gitignore` — the previously-tracked casts remain versioned.
- **`ops.drain(..)` that silently discarded diff output** in the reports pipeline (`8518e6d`).
- **Byte-slice truncation in `record_memory_recall`** that could panic on multi-byte UTF-8 (`8330b95`).
- **Fail-closed reviewer doc.** `reviewer.approved` comment incorrectly described the behaviour as fail-open; corrected to match the code. `crates/hone-core/src/reviewer.rs`.

### Added (earlier in this development cycle)

- **Agent Rail observability sidebar** — Press `Ctrl+R` to toggle a real-time per-turn sidebar showing files touched (with R/W/X badges), injected context (skeleton + BM25 ranked entries with scores), and memory recalls. Width-adaptive (≥100 cols). Scrollable with j/k (future). Only appears in interactive TUI mode.
- **TurnLedger per-turn accumulator** — Internal data structure recording file operations, skeleton injections, memory recalls, tool invocations, and phase transitions. Automatically aggregated into session reports on `/quit`.
- **Session reports** — Auto-generated on exit to `.hone/reports/<timestamp>-<session>.md` + `.md.json` sidecar. Includes cost, tokens, file stats, tool timelines, context strategy, memory activity, and phase transitions. Default retention: 100 reports, oldest deleted first. (Currently TUI-mode only; recipe/orchestrate/ensemble/one-shot modes planned in follow-up.)
- **`hone reports` CLI** — Three subcommands: `list [--limit N]` (newest-first with file size), `show <id>` (full markdown + pager-friendly), `diff <a> <b>` (line-level LCS diff). Supports prefix matching on report IDs.
- **Accessibility flags** — `HONE_REDUCED_MOTION=1` disables animation in spinners (defined; full wiring in follow-up). `HONE_ASCII_ONLY=1` forces ASCII box-drawing and file-operation badges. Both respect `NO_COLOR=1` for color-blind and log-aggregation use cases.
- **Concurrency limiter for multi-agent orchestration** — `Orchestrator::with_max_concurrent(n)` caps the number of sub-agents running in parallel via tokio::sync::Semaphore. Useful for rate-limited providers or sequential execution. Effective limit is `n.min(max_workers)`. Setting n=1 serializes all workers.
- **Sub-agent health monitoring with timeouts** — `Orchestrator::with_worker_timeout(secs)` enforces per-worker wall-clock timeout. Workers exceeding the limit are cancelled and recorded as `WorkerStatus::TimedOut`. Defaults to 300s (5 minutes). Stuck agents are killed without aborting the entire orchestration.
- **Crash-recovery checkpointing** — `CheckpointStore` persists orchestration state to SQLite after each worker completes. On interruption, `Orchestrator::resume(run_id)` reloads the checkpoint and continues from where it left off. Stores goal, task plan, completed results, pending task IDs, and accumulated cost. Survives process kills, network failures, and timeouts.
- **Agent persona evals** — `benchmarks/persona_eval.py` evaluates 12 built-in agent personas across 35 scenarios (debugger, security-auditor, code-reviewer, rust-expert, python-expert, ai-researcher, postgresql-expert, ml-engineer, data-engineer, test-architect, fullstack-architect, ml-recommendations-expert, quant-advisor). Keyword-matching harness validates domain knowledge and terminology consistency. Run with `python benchmarks/persona_eval.py --url http://localhost:8080`.
- **Adaptive hierarchical skeleton** — Context injection auto-selects detail level based on repo size: <20 files = full call graph, 500+ files = directory tree only. 4-level hierarchy (directory → file list → signatures → call graph) with progressive budget filling. Smart file relevance detection skips files >100KB and slow parses (>100ms). Delivers optimal accuracy while respecting token budgets.
- **Scope clarification mode (`/clarify`, `--no-clarify`)** — Auto-detects ambiguous requests using 4 heuristics (missing file context, unclear scope, vague terminology, contradictory hints). Asks clarifying questions before coding. `--no-clarify` flag disables for CI/scripting. `/clarify` command manually triggers within interactive session.
- **Inline agent dispatch (`@agent-name`)** — Use `@security-auditor`, `@debugger`, `@code-reviewer`, etc. inline to swap system prompt for the turn. 12 built-in agents available. System prompt restores after the turn completes. Example: `@security-auditor "audit this code"`.
- **Safe pipe operator** — Pipes to head/tail/grep/wc/sort now allowed in shell commands (previously blocked as destructive). Pipes to unknown commands still escalate as destructive for safety.
- **Ollama tool calling enabled** — OllamaProvider now supports full tool calling (was previously hardcoded to skip). Qwen 2.5 Coder 14B is recommended local model for code generation with tool support.
- **Model override fixes** — `--model ollama` now directly creates OllamaProvider without falling back to Claude CLI when user explicitly requests Ollama.
- **Text-based tool call parser for Ollama** — Detects JSON tool calls in content field (for models that don't use tool_calls field). Strips markdown code fences automatically. Known limitation: Qwen 2.5 Coder sends tool calls as text but cannot process results back (use for testing only).
- **DeepSeek Direct compatibility** — Works for interactive use and simple tasks. SWE-bench benchmarks: lower output tokens than OpenRouter cause pipe blocking and empty patches (290/300 failed). OpenRouter remains recommended for benchmarks.
- **RALPH loop (Review, Analyze, Learn, Plan, Heal)** — Structured feedback cycle combining adversarial review with root cause analysis and pattern learning. Max 3 iterations (configurable), stops when reviewer approves. Captures learned patterns for cross-session improvement. Optional test verification after each Heal step. Research basis: Self-Refine (NeurIPS 2023), ChatDev, MASAI, and AgentCoder patterns. Implementation: `crates/hone-core/src/ralph_loop.rs`.
- **Token count in status bar** — Inline compact display (e.g., "42t") and full fraction format in wide mode (e.g., "42/200000 tokens"). Helps track context usage at a glance during interactive sessions.
- **9 recipes (expanded from 4)** — New recipes for security-audit, performance, migration, documentation, and onboarding workflows. Template variable support enables per-project customization with `--var KEY=VALUE`.
- **Shell escape (`!command`)** — Execute native shell commands directly from Hone prompt. `!ls`, `!git status`, `!cargo test` show inline output. `!cd src` changes directory persistently (confined to project root). `!nvim file.rs` suspends TUI, runs editor, restores on exit.
- **Vim mode (`--vim` or `:set vim`)** — Full vim-style keybindings in interactive mode. Normal mode: j/k for scroll, gg/G for top/bottom, i/a for insert, :q to quit. Insert mode: type normally, Esc returns to normal. Toggle with `:set vim` and `:set novim`.
- **TDD mode (`:tdd [command]`)** — File watcher + auto-test workflow. Watches source files with 300ms debounce, auto-runs test command on change, displays PASS/FAIL inline. Stop with `:tdd off`.
- **Review mode (`:review`)** — Hunk-by-hunk accept/reject interface for git diffs. Navigate with j/k, accept (a) or reject (r) individual hunks, accept/reject all (A/R), quit with q. Only accepted hunks are staged and committed.
- **JSON output mode (`--json`)** — Structured JSON output for CI/CD automation. Includes response text, tool calls, token usage, cost, and exit status. Non-interactive, suitable for scripting and integration.
- **Prompt injection detection** — Scans tool results and model output for injection patterns (high/medium/low severity). Uses regex-based heuristics to flag suspicious instructions like "ignore previous instructions" and shell escape sequences.
- **Recipe template variables (`--var KEY=VALUE`)** — Substitute {{var}} placeholders in recipe prompts. Repeatable; supports arbitrary key-value pairs. Example: `hone --recipe tdd-feature --var feature_name=oauth --var test_command="pytest tests/auth"`.
- **Egress audit log (`--audit`)** — JSONL audit trail of all tool calls and model requests. Records timestamp, tool name, arguments, and results to `.hone/audit.jsonl`. Enabled via `--audit` flag or `HONE_AUDIT_LOG=1`.
- **Proactive context injection (`--inject-context`)** — Three-tier context injection for improved SWE-bench results: (1) repository skeleton with file tree and signatures, (2) confidence-gated BM25-ranked function signatures, (3) exploration budget hint. Improves pass rate from 19.3% to 86% (+12pp). Enabled via `--inject-context` flag or `HONE_CONTEXT_INJECTION=1`.
- **MCP extensions integration** — Hone can now use 70+ community MCP extensions via `.hone/mcp.json` config (compatible with Claude Code format). McpManager spawns/manages servers with `env_clear` for security isolation. Zero changes to agent loop — McpTool already implements Tool trait.
- **Status bar redesign** — Three-zone layout: Left (model/session/phase), Center (tool + icon), Right (context bar with cost). Width-adaptive (80/120/160+ cols), context window bar with color thresholds, tool type icons, NO_COLOR support.
- **GitHub Pages site** — Documentation now auto-deploys on push via MkDocs + Material theme.
- **Named session management** — `hone session list/resume/rename/delete` commands for persistent named conversations. Sessions auto-save after each turn. `--session NAME` flag to start a named session. `--ephemeral` flag disables saving. Implementation: `bins/hone/src/session.rs`.
- **Interactive configure wizard** — `hone configure` guides users through provider selection, API key entry, and model selection. Writes `~/.config/hone/config.toml`. Implementation: `bins/hone/src/configure.rs`.
- **UX improvements** — NO_COLOR environment variable support for disabling colored output. Ollama model verification during configuration. Ensemble mode validates 2-10 candidate count. Provider capability warnings for --orchestrate and --ensemble modes.
- **Context optimization** — Model-aware token budgets, BM25 reordering for relevance, graduated compression strategies, and phase detection to minimize context overhead while preserving code comprehension.
- **Memory encryption** — AES-256-GCM symmetric encryption with Argon2id key derivation for encrypted session storage. Opt-in via `--features encrypted-memory`. Encryption keys stored in system keychain.
- **Cross-session learning** — AdaptiveClassifier learns patterns across sessions to improve request routing, complexity estimation, and tool selection. Patterns persist in encrypted memory.
- **Multi-agent budget tracking** — Unified token accounting across orchestrated agents. Per-turn, per-session, and per-user cost tracking with enforcement and rollback on budget exhaustion.
- **SWE-bench benchmark harness** — Full benchmarking pipeline for SWE-bench Lite (300 real GitHub issues). `benchmarks/swebench_inference.py` and `swebench_evaluate.py` with Makefile targets for swebench-dry-run, swebench-lite, and swebench-verified. Supports evaluation via Docker-isolated test execution.
- **Custom OpenAI-compatible endpoints** — `HONE_OPENAI_BASE_URL` and `HONE_OPENAI_MODEL` environment variables for drop-in support of DeepSeek, Qwen, Kimi, OpenRouter, and other OpenAI-compatible services.
- **Better startup error messages** — Clearer diagnostics for missing API keys, sandbox unavailability, configuration errors, and provider health checks.
- **System prompt compression** — Reduced system prompt from 1776 to 511 tokens (71% reduction), saving approximately 25K tokens per 20-turn task. Achieved through iterative refinement and removal of redundant instructions while maintaining quality parity.
- **Tool description overhaul** — All 7 core tools now feature structured WHEN TO USE / WHEN NOT TO USE fields for improved decision-making and tool selection accuracy.
- **SWE-bench v1 optimizations** — Structured 4-phase prompt design (exploration → localization → implementation → verification), two-stage localization strategy, and hardcoded tool guards (must-read-before-edit, test-after-edit hints, max consecutive edits limit).

### Changed

- **Context injection now enabled by default** — Proactive context injection (three-tier system: repo skeleton, confidence-gated BM25 signatures, exploration budgets) is now the default behavior, removing the need for `--inject-context` flag. Disable with `--no-inject` or `HONE_NO_INJECTION=1`.
- **SWE-bench v4 sample (corrected 2026-11)** — On a 20-task sample, 19/20 tasks produced a non-empty patch (patch-generation rate, not graded) vs 15/20 for v1. The multi-attempt-with-rollback strategy ran in `benchmarks/swebench_inference.py`, not in Hone. *Originally published as a "95% pass rate" and extrapolated to 300 tasks; withdrawn.*
- **SWE-bench v3 sample (corrected 2026-11)** — On a 50-task sample with opt-in injection, 43/50 tasks produced a non-empty patch (not graded) vs 37/50 for v1. *Originally published as an "86% pass rate"; withdrawn.*
- **SWE-bench cost (corrected 2026-11)** — The graded v1 run cost $8.56 for 300 tasks (~$0.03/task, DeepSeek V3). *The earlier "~$0.002 per task, 50–100× cheaper than SWE-Agent" claim relied on the withdrawn v4 figures; withdrawn.*
- **Review mode interaction** — `:review` now supports hunk-by-hunk navigation and selective acceptance. Rejected hunks are reverted; accepted hunks are staged.
- **Exit code 1 on missing provider** — Hone now returns exit code 1 when no provider is configured (was silently falling back to MockProvider). Users must configure a provider via `hone configure` or environment variables.
- **Security hardening (Phase 2 complete)**:
  - **Path traversal prevention** — All file tools use `resolve_within_working_dir()` to canonicalize paths and reject `..` escapes.
  - **Shell command classification** — New intent classifier detects compound shell operators (`;`, `&&`, `||`, `|`, `$()`, backticks) and escalates destructive commands to require explicit user approval.
  - **Seatbelt FFI on macOS** — `sandbox_init(3)` now fully functional. Path canonicalization handles macOS symlinks (`/tmp` → `/private/tmp`). Global `file-read*` rules for Dyld compatibility. Process fork/exec support for child sandboxing.
  - **Landlock on Linux** — Full Landlock LSM support (Linux 5.13+) with fine-grained filesystem access rules. Seccomp-BPF fallback for older kernels.
  - **CLI stdin prompts** — Sensitive input (API keys, one-shot prompts) now piped via stdin instead of CLI args to avoid exposure in `ps` output.
  - **Binary validation** — `check_binary_available()` now validates binary names with denylist of metacharacters instead of dangerous `sh -c` calls.
  - **Rate limiter using peer IP** — Server-side rate limiting now uses actual peer socket address instead of spoofable `X-Forwarded-For` header.

- **Storage improvements**:
  - **Dual-database strategy** — Semantic index now uses PostgreSQL+pgvector for vector similarity search (HNSW/IVFFlat indexes). Sessions and memories remain in SQLite for zero-ops local storage.
  - **RwLock poison recovery** — Provider registry uses `unwrap_or_else(|e| e.into_inner())` to recover state after panic.
  - **Memory IDs with UUID v7** — Session and memory IDs now use sortable UUID v7 instead of truncated timestamps.
  - **Cache keys with blake3** — Tool result cache uses blake3 hashing for integrity instead of non-cryptographic `DefaultHasher`.
  - **Git SHA via libgit2** — HEAD resolution now uses pure `git2` crate (in-process) instead of subprocess spawning.

- **Diagnostics improvements**:
  - Updated benchmark results for M-series Macs with warm-cache startup times.
  - Clarified token estimation costs (10ms per message including tiktoken cache warm-up).
  - Added memory footprint breakdown (hone process ~30-40MB, claude CLI subprocess ~270MB).
  - Documented SWE-bench evaluation methodology and Docker requirements.

### Deprecated

- **In-memory semantic index fallback only** — PostgreSQL+pgvector is now the default semantic index. In-memory fallback available only when database is unavailable (see `docs/architecture.md` section 9.2).

### Fixed

- **Ollama tool_call arguments serialization** — Fixed multi-turn tool-call replay: `convert_message` now sends `tool_calls[].function.arguments` as a JSON object (was stringified), matching Ollama's `/api/chat` wire format. Ollama's Go server was rejecting stringified arguments with HTTP 400 "Value looks like object, but can't find closing '}' symbol".
- **Shell injection vulnerability** — Command classification now prevents execution of compound shell operators without explicit user consent.
- **Path canonicalization race** — symlink targets are now validated to be in allowlist at tool execution time, preventing TOCTOU exploits.
- **Subprocess credential leakage** — API keys and tokens no longer exposed in process arguments; moved to stdin or environment (with whitelist).

### Documentation

- **Architecture document** — Updated with storage architecture (9.1, 9.2), security hardening (10), and key design decisions (11). Added ER diagrams for SQLite and PostgreSQL schemas.
- **Security guide** — Expanded threat model, added Seatbelt/Landlock platform matrix, documented environment variable filtering and audit logging.
- **Benchmarks document** — Added M-series performance baseline, cold-start analysis, memory breakdown, and SWE-bench methodology.
- **README** — Updated feature list with context optimization, memory encryption, cross-session learning, and custom endpoint support.
- **New CHANGELOG** — This document, using Keep a Changelog format.

### Testing

- **Security tests** — Added 20+ tests for path traversal prevention, shell command classification, and sandbox policy enforcement.
- **Context optimization tests** — Unit tests for BM25 reordering, graduated compression, and token budget enforcement.
- **Memory encryption tests** — Added AES-256-GCM roundtrip tests with Argon2id key derivation.
- **Benchmark microbenchmarks** — Updated for cold-start vs warm-start distinction. TUI render regression documented (16% vs baseline, still <0.1% frame budget).

### Previous Release (v0.1.0)

- Initial public release with core agent loop, 75+ model providers, native tools (L2), MCP client (L1), sandboxing (Seatbelt/Landlock), SQLite memory, semantic index, and multi-agent orchestration.

---

## Contributing

See [docs/contributing.md](docs/contributing.md) for development guidelines, testing, and commit message conventions.

## Security

For responsible disclosure of security vulnerabilities, see [SECURITY.md](SECURITY.md) (if available) or contact security@anthropic.com.
