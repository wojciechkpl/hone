# Starter eval suite

Twenty small, offline tasks for `hone eval` (format: `docs/plans/eval-profiles-spec.md`; design:
`docs/plans/eval-starter-suite-spec.md`). Every task has a deterministic check, and every check is
verified without a model: it fails on the untouched task directory and passes on a reference
solution (`sh eval/suites/starter/verify.sh`).

## Layout

| Path | What it is |
|---|---|
| `tasks.toml` | the tasks: `id`, `prompt`, `success_check`, `working_dir`, optional `memory_fixture`, `timeout_secs` |
| `<id>/` | the task's working directory; the runner works on a fresh copy |
| `fixtures/<id>.jsonl` | memory tasks: one memory per line (`id`, `kind`, `content`, `tags`, `weight`, `created_at`), loaded into the run's fresh memory store |
| `checks/<id>.sh` | the success check: runs with the working-directory copy as its current directory and `EVAL_SUITE_DIR` set to this directory |
| `checks/data/`, `checks/lib/` | expected outputs and shared helpers for checks |
| `reference/<id>/` | the reference solution: files that overwrite the working copy (an optional `.remove` lists files to delete first) |

References, checks and fixtures live outside every working directory, so the agent never sees
them. All paths in `tasks.toml` are relative to that file.

## Tasks

**Coding (8)**: fix a bug, add a function, or refactor with the tests green. Measures basic
read, edit and test-loop competence per language. Checks run the project's own tests with
`python3 -m unittest`, `node --test` or `cargo test --offline` (no external crates).

| Task | Language | What |
|---|---|---|
| `c01-py-fix-median` | Python | `median()` is wrong for even-length input |
| `c02-py-add-slugify` | Python | add `slugify()` so the existing tests pass |
| `c03-py-refactor-discount` | Python | one `apply_discount()` instead of three copies |
| `c04-js-fix-flatten` | JavaScript | `flatten()` ignores nesting depth |
| `c05-js-add-chunk` | JavaScript | add `chunk(arr, size)` |
| `c06-js-refactor-money` | JavaScript | one `formatMoney()` instead of three copies |
| `c07-rs-fix-kvparse` | Rust | `parse_kv()` drops part of a value containing `=` |
| `c08-rs-add-roman` | Rust | add `to_roman()` |

**Memory (4)**: the needed fact exists only in the task's memory fixture, never in the prompt or
the working directory (`verify.sh` enforces that through `checks/<id>.facts`). Measures whether
recall surfaces the right memory among distractors and the agent applies it.

| Task | Fact in the fixture |
|---|---|
| `m01-mem-report-filename` | the team's report file naming convention |
| `m02-mem-staging-port` | the agreed staging service port |
| `m03-mem-deadline-format` | the preferred date format for deadlines |
| `m04-mem-retry-rule` | the reviewer's retry rule (attempts and pause) |

**Non-coding (4)**: checked exactly, with no model judging.

| Task | What | Check |
|---|---|---|
| `n01-summarise-notes` | summarise five meeting notes into `summary.md` | required facts by case-insensitive grep (final decisions, not earlier proposals) |
| `n02-table-to-csv` | markdown table to `quarterly.csv` | parsed CSV equals the expected rows |
| `n03-rename-photos` | rename files to a convention | directory listing equals the expected names, contents intact |
| `n04-fill-config` | fill `service.ini` from a meeting note | parsed values equal the final decisions |

**Context / retrieval (4)**: a 44 to 60 file project where the prompt describes behaviour in
plain English and the change belongs in one file the prompt does not name, among decoys with
similar names and constants. Measures whether the context strategies (skeleton, recall, search)
get the agent to the right file. The check verifies the behaviour **and** that no other file
changed (a hash manifest in `checks/data/`).

| Task | Project | Behaviour |
|---|---|---|
| `x01-ctx-free-shipping` | Python, 62 files | orders above 100 euros ship free |
| `x02-ctx-long-names` | JavaScript, 55 files | leaderboard names cut to 12 characters with an ellipsis |
| `x03-ctx-blank-timeout` | Python, 48 files | a blank timeout in the settings means 30 seconds |
| `x04-ctx-invoice-order` | JavaScript, 47 files | the invoice export lists the newest first |

## Verify the suite (no model)

```sh
sh eval/suites/starter/verify.sh            # every task
sh eval/suites/starter/verify.sh c01-py-fix-median m02-mem-staging-port
```

One line per task: `PASS`, `FAIL` or `SKIP` (a toolchain from the check's `# requires:` line is
missing). It exits non-zero on any `FAIL`. The Rust test `crates/hone-eval/tests/starter_suite.rs`
runs it.

Each check is built to resist shortcuts: the c01-c08 checks restore the project's own tests from
`checks/data/<id>/` before running them (so deleting or weakening tests earns nothing), the
refactor checks look at behaviour (swap the helper and see the callers change), `n01` normalises
spelling variants and rejects a pasted copy of the notes, and the context tasks carry decoy files
(legacy copies, docs, look-alike helpers) that contain the prompt's words. `verify.sh` also runs
known cheats (`checks/cheats/<id>/<name>/`, must fail), correct alternative solutions
(`checks/alt/<id>/<name>/`, must pass) and a "tests deleted" run, printed as `OK` lines. Every check
run is capped at `VERIFY_TIMEOUT` seconds (default 150); a timed-out check is killed with its whole process group, and `verify.sh` self-tests that no background sleeper survives.

## Run it under two profiles

Define the profiles in a copy of `tasks.toml` (see the profile format in the F spec), then:

```sh
hone eval eval/suites/starter/tasks.toml --profile default --profile no-inject --repeats 3
```

This calls a real model: it costs API money, or local-model time (for example Ollama on a
workstation). Three repeats of 20 tasks under two profiles is 120 agent runs. Start with
`--repeats 1` and a few tasks, and use `hone eval ... --check` first to validate the files without
a model call.

## What the runner must provide

- `EVAL_SUITE_DIR`: the absolute path of this directory, when it runs a check; the check's current
  directory is the run's working-directory copy.
- Memory fixtures are JSONL, one object per line with at least `id`, `kind` and `content`
  (the same fields as a stored memory; `tags`, `weight`, `created_at` are included).

## Residual risk

Eval agents run in a copy of the task directory, but they are not sandboxed from reading the rest
of the disk, and the suite's path is in the process table while a check runs (the parent's argv
and environment). A determined agent could find the suite, its reference solutions and the
expected values. The checks are built so that nothing the child can reach decides a verdict, but
that is a mitigation, not a proof: run suites from a location the model is not told about, and
treat results as measurements, not proofs. Run the agent in a container or a separate user if the
numbers matter.
