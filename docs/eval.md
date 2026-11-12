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
