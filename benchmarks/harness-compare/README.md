# harness-compare: Hone vs Pi vs Hermes on one locally served model

This directory benchmarks coding-agent *harnesses* against a single locally served model.
The model, sampling and caps are held fixed; only the harness changes. It automates
Phase 1 of [`docs/plans/trust-defaults-daemon-2026-11.md`](../../docs/plans/trust-defaults-daemon-2026-11.md)
and serves as the regression gate for Phase 2.

```
hc-llm (vLLM) ◀── hc-proxy (metering, forced sampling, caps, records) ◀── arm containers
                                                                     (internal network, no internet)
```

- **Suites**
  - **A**: SWE-bench Lite, 40 tasks (pre-registered, stratified by repo, official grader)
  - **B**: the 10-task mini-bench (graded by check commands; never part of the headline)
  - **C**: red team, 20 risky calls + 5 planted injections, judged by tripwires
- **Arms** (`config/arms.toml`): `pi`, `hone-noinject`, `hone-inject`, `hermes`, plus
  `pi-guarded`, `hone-guarded`, `hermes-guarded` and `hone-callgraph`. The `selftest` arm
  checks plumbing only. `hermes` runs Hermes Agent with `--yolo`; `hermes-guarded` keeps its
  approval prompts on (probe P-H4 records what that does headless).
- **Contracts**: [`SPEC.md`](SPEC.md). Tests: `tests/` (pytest, no plugins).

## Quick start on the GPU box (Ubuntu + RTX 5090)

```bash
cd benchmarks/harness-compare
cp config/bench.env.example config/bench.env    # fill in every CHANGE-ME
make preflight bundle serve stack                # P1.1–P1.4 (bundle: Hone, Pi, Hermes)
make smoke                                       # P1.5 capability probes incl. Hermes P-H1..P-H7 (must PASS)
make prereg NAME=2026-11                         # P1.6, then commit + tag + push
make run-c run-b run-a CAMPAIGN=2026-11          # P1.7–P1.9, resumable
make grade report CAMPAIGN=2026-11               # P1.10–P1.11
```

**Hermes in the bundle.** `scripts/01_build_bundle.sh` installs Hermes Agent at
`HC_HERMES_REF` (a release tag; its SHA goes into `manifest.json`) from its `uv.lock`, with its
own python-build-standalone interpreter (`HC_HERMES_PYTHON`, installed by `uv`) and a venv built
at the final path `/opt/harness/hermes/venv`, because arm containers are SWE-bench images with
their own Python and the bundle is mounted read-only. `bin/hermes` runs the venv's Python on a
console script kept alone in `hermes/entry/` (Hermes prepends that dir to the tool shells' PATH,
so the venv's `bin/` must not be it). Skip it with `--skip-hermes` (the manifest then keeps the existing build's ref and SHA).
The Hermes build runs as `linux/amd64` (slow under emulation on Apple silicon), with uv and the
build image pinned by checksum/digest (`HC_UV_SHA256`, `HC_HERMES_BUILD_IMAGE`), and checks that
`HC_HERMES_REF` (a tag or full SHA) resolves to `HC_HERMES_SHA`.

To check the plumbing without a GPU (works on macOS with Docker): `make selftest`.

## Day-to-day

| Need | Command |
|---|---|
| Unit tests + lint | `make test lint` |
| Continue an interrupted campaign | re-run the same `make run-a CAMPAIGN=…`; finished runs are skipped |
| Retry runs that hit infra errors | `scripts/10_run.sh a <campaign> --retry-errors` |
| Look at one run | `results/<campaign>/<suite>/<arm>/s<repeat>/<task>/`: `run.json`, `proxy.jsonl`, `stdout.txt`, `patch.diff` |
| Phase 0 claim check | `make check-claims` |
| Phase 2 policy gate | `HC_HONE_REF=<branch> make policy-gate CAMPAIGN=p2-…` |

## Rules that keep the result defensible

1. Pre-register (`make prereg`), then commit and tag **before** any suite run. Log every
   later change in `prereg/<name>/DEVIATIONS.md`.
2. Leave the proxy overrides (model, temperature, top_p, max_tokens, seed) and the caps
   alone after pre-registration.
3. Never tune prompts per arm. Every arm gets `config/prompts/swebench.md` verbatim.
4. Publish everything: `scripts/30_report.sh <campaign> --package` writes the trajectories,
   predictions, grader outcomes and pre-registration, with SHA256SUMS.
