# A panic stops what hone started: spec

Branch `fix/panic-stops-servers`. This is the contract for the TDD rounds: rows C1–C6.

## Problem

Neither `hone` nor `honed` installs a panic hook. Release builds use `panic = "abort"` (root `Cargo.toml`), so a panic there ends the process at once. No destructor runs, nothing is signalled and no registry entry is touched. Since PR #90/#92, shell commands and MCP servers run in their own process groups, so the terminal's signals don't reach them either. A panic therefore leaves behind:
- **MCP servers** that ignore their input closing;
- **shell commands** that are still running (a `cargo test`, a dev server).

PR #92 left this open: a crash (panic) can still leave a server running.

There's a second problem in the TUI, found while here. It enters raw mode, bracketed paste, mouse capture and the alternate screen, and restores them only after `run_tui` returns. A panic skips the restore in any build, so it leaves the terminal in raw mode on the alternate screen. The panic message is lost and the shell is unusable until `reset`.

```mermaid
%%{init: {"themeVariables": {"fontSize": "13px"}, "flowchart": {"nodeSpacing": 25, "rankSpacing": 30, "padding": 8}, "sequence": {"actorMargin": 30, "messageMargin": 25, "boxMargin": 6}, "gantt": {"barHeight": 16, "barGap": 3}}}%%
sequenceDiagram
  participant T as Panicking thread
  participant H as Panic hook
  participant R as Group registry
  participant S as MCP server / command
  T->>H: panic (one that ends hone)
  H->>H: TUI on screen? restore terminal (C5)
  H->>H: previous hook prints the message (C2)
  H->>R: lock, at most ~100 ms (C4)
  H->>S: commands SIGKILL, MCP SIGTERM
  H->>S: SIGKILL what is alive ~1 s later (C1)
  H-->>T: return; abort (release) or unwind (main thread)
```

## Rows

| Row | Behaviour |
|---|---|
| C1 | **A panic that ends hone stops what it started.** A panic counts as ending the process when either:<br>• it happens in a build where panics abort (release), on any thread;<br>• it happens on the main thread, in any build.<br><br>This covers `hone` (TUI, one-shot, every subcommand) and `honed`. Before the process ends, the registered process groups are stopped the way an abrupt exit stops them (`hone_tools::kill_running_commands()`):<br>• shell-command groups get SIGKILL at once;<br>• MCP-server groups get SIGTERM, then SIGKILL for any still alive about 1 s later.<br><br>Before, a release-build panic stopped nothing. |
| C2 | **The panic still reads and ends as before.** The hook that was installed before (by default, the message and a backtrace when `RUST_BACKTRACE` is set) still runs, and it runs before the wait of up to 1 s. The process still ends the same way: SIGABRT in a release build, exit code 101 for a main-thread panic in a debug build. |
| C3 | **A panic that doesn't end hone stops nothing.** In a build where panics unwind (debug, tests), a caught panic off the main thread leaves every process group and the terminal alone. Examples: a tokio task's panic, which tokio turns into a `JoinError`, or a joined `std::thread`. |
| C4 | **The hook can't hang hone or cut a cleanup short.** The panic may strike while the group registry is locked, even on the panicking thread itself.<br>• The hook waits at most about 100 ms for the registry. If it can't get it, the hook skips stopping groups and lets the panic proceed.<br>• If several threads panic at once, the cleanup runs once. The other threads wait for it to finish instead of ending the process in the middle of its SIGTERM grace. |
| C5 | **The TUI gives the terminal back.** If a panic that ends hone (C1's rule) happens while the TUI is running, the terminal is restored before the panic message is printed, as on a normal TUI exit:<br>• raw mode off;<br>• bracketed paste and mouse capture off;<br>• the alternate screen left;<br>• the cursor shown.<br><br>"While the TUI is running" means from entering raw mode until the normal restore after `run_tui`. Outside that window (one-shot runs, `honed`, before the TUI starts, after it ends) the hook writes nothing to the terminal. |
| C6 | **Docs.** `docs/configuration.md` (MCP section) and the CHANGELOG say that a panic now stops MCP servers and shell commands. Only a hard kill still leaves a server that ignores its input closing: `kill -9`, the OOM killer, or a crash that isn't a panic (a segfault, a stack overflow, an abort from C code). The CHANGELOG also notes the TUI terminal fix. |

## Testing notes

- **C1–C4 at library level.** Use a child process that panics on its real **main** thread, for example a `harness = false` integration test that re-executes itself with an argument. The child:
  - installs the hook;
  - registers scripted servers in their own groups with `ProcessGroupGuard`: one that ignores SIGTERM, and one whose SIGTERM cleanup takes about 300 ms and writes a marker;
  - registers a shell-command-style group;
  - panics.

  Check the panic message on stderr, the exit status, the marker and that every pid is gone, all with bounded waits.
- **C3.** A child whose spawned thread panics (`join` returns `Err`) then checks that its groups are still alive and exits normally.
- **The abort path.** A release build (`panic = "abort"`) can't run under `cargo test`, whose profile always unwinds. Verify it by hand with a release build of `hone` and note the result in the PR.
- **No panic trigger in production code.** No env var or flag in release builds makes hone panic. If C5 can't be driven end to end without one, test the restore sequence and the "TUI running" flag at unit level.

## Out of scope

- `kill -9`, the OOM killer, segfaults, stack overflows, aborts from C code: no hook runs. On Linux, `PR_SET_PDEATHSIG` on each server could cover these; that's a possible follow-up.
- Child processes outside the registry (LSP servers, hooks): unchanged.
- A panic in a `tokio` task in a debug build keeps hone running as before. Only release builds abort on it.
- The main-thread rule assumes tasks don't run on the main thread. `hone` and `honed` use tokio's multi-thread runtime, where they don't. On a `current_thread` runtime, a caught task panic would count as ending the process (debug builds only).
