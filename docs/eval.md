# `hone eval`

`hone eval <tasks.toml>` runs each task with a real agent in a fresh copy of the
task's directory, runs the task's success check there, and writes a run directory
with per-run outcomes, context receipts, transcripts and a report. Profiles
(`[profiles.<name>]`, using the `[context]` / `[memory]` schema of `config.toml`)
let you compare strategies on the same tasks; `--ablate SLOT` generates profiles
that vary one slot.

```
hone eval tasks.toml --check                  # validate, no model call (exit 0 or 2)
hone eval tasks.toml --repeats 3 --profile lean --profile default
hone eval tasks.toml --ablate context.skeleton --model ollama:qwen2.5-coder:14b
```

Output goes to `./eval-runs/<run-id>/` (override with `--output`). The directory
holds `manifest.json`, `report.json`, `report.md` and, per run,
`<profile>/<task>/r<k>/{outcome.json,receipts.json,transcript.json,check.txt}`.

## Hermetic runs

Eval runs are hermetic: no MCP servers, no language server, no tool cache, and the
model is pinned (no routing or fallback). Tasks that need MCP are not supported
yet. Each run gets its own copy of the task directory (outside the repository,
deleted right after the run unless `--keep-workdirs`), its own memory store loaded
from the task's `memory_fixture`, and its own session. The success check runs in the
copy with `EVAL_SUITE_DIR` set to the tasks file's directory.

## What runs, and what doesn't

- **Git state is isolated.** A copy never shares git state with the original. A
  `.git` directory is copied as a directory (an independent repository). If the
  task directory is a git worktree (`.git` is a `gitdir:` file) or a subdirectory of
  one, the copy gets its own repository at the same `HEAD` (it only reads the
  original's object store; everything it writes stays in the copy), with the
  working-tree contents as changes. A subdirectory's copy shows the rest of the tree
  as deleted. If that cannot be set up the run is an `infra_error`.
- **Copies are bounded.** The copy respects the task directory's `.gitignore`
  (keeping `.git`), skips `target/` and `node_modules/` only at the top level,
  clones files copy-on-write where the filesystem supports it, and a copy over
  200 MB is reported once per task in the manifest's `warnings`. A symlink whose
  target is outside the task directory makes the run an `infra_error` naming the
  link; links inside are recreated. FIFOs, sockets and devices are skipped, with a
  note in `check.txt`.
- **Agent CLIs are refused.** `claude-cli:` and `gemini-cli:` models run their own
  tools outside Hone's sandbox, permission rules and working directory, so
  `hone eval` (and `--check`) rejects them.
- **Interrupts clean up.** On SIGINT or SIGTERM every running agent's commands are
  stopped, working-directory copies are removed, unfinished runs are marked
  `interrupted`, the reports are written for what finished (manifest
  `interrupted: true`), and the exit code is 130 or 143. A second signal exits at
  once. A check's own processes die with it: each check runs in its own process
  group, which is killed when the check times out and after it finishes.
- **The manifest says what ran:** hone's version and build git SHA, the suite's
  git SHA, and the `HONE_AUDIT_LOG`, `HONE_CONTEXT_WINDOW`, `HONE_POLICY_BUILTINS`
  and `HONE_PHASE_COMPACTION` values.

## Reading the numbers

- Repeats are not independent samples of the task distribution. The per-profile
  interval treats every run as one sample, so it is narrower than it should be when
  tasks repeat; the paired comparison resamples tasks (not runs), which is the
  unit that matters. Prefer more tasks over more repeats.
- HONE.md files above the task directory are not loaded: the copy lives in the
  system temp directory, so only HONE.md files inside the task directory apply.
- A shell command that needs approval in a one-shot run needs it in eval too:
  turns are auto-approved, but `deny` permission rules (and the built-in policy)
  still apply, so a task whose commands are denied fails the same way.
- Cost depends on the provider reporting token usage and on the model's pricing
  entry; a provider that reports no usage shows zero cost and tokens.

## Limits

- A check that starts a new session (`setsid`) escapes the process-group kill:
  the runner kills the check's process group, and a session leader is in another
  one.
- A copied `.git` directory has `core.worktree` cleared in its `config`, so the
  copy's git never points at another tree. Submodules are not linked in copies:
  a submodule's own `.git` file refers to the original's module store.
- A task directory that is a subdirectory of a larger repository gets a fresh
  repository with its files in one initial commit, with no link to the enclosing
  repository, so the agent sees the same thing wherever the suite is checked out.
  A task directory that is a repository root keeps its history.

## Suite integrity

`[eval] integrity = ["checks", "reference"]` lists directories (relative to the
tasks file) that must not change during a run. The runner hashes them (every file's
path and content) before the first run and after every check; on any change the
current and remaining runs end `infra_error` ("suite modified during the run") and
`report.json` has `"integrity_violated": true` (`report.md` flags the run as
untrustworthy). `--check` rejects a path that is not a directory.
