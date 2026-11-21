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
  - **A**: SWE-bench Lite, 40 tasks (pre-registered, stratified by repo, official grader).
    It is retired as the headline suite: stage 2 of campaign 1 replaces it with SWE-rebench
    once the loader lands (design §10.3, §11 and §11.15)
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
Skip Pi with `--skip-pi`: the manifest still gets `pi_package`, `pi_installed_version` and
`pi_lock_sha256`, read from the Pi install on disk (null if there is none), never from an old
manifest. `node_installed_version` is likewise read from the bundle's Node, whatever the flags.
The Hermes build runs as `linux/amd64` (slow under emulation on Apple silicon), with uv and the
build image pinned by checksum/digest (`HC_UV_SHA256`, `HC_HERMES_BUILD_IMAGE`), and checks that
`HC_HERMES_REF` (a tag or full SHA) resolves to `HC_HERMES_SHA`.

To check the plumbing without a GPU (works on macOS with Docker): `make selftest`.

## Campaign 1 (stage 1: suites C and B)

Campaign 1 runs the safety suites first. Its scope and requirements are in
[`harness-compare-v2`](../../docs/plans/harness-compare-v2-2026-11.md) §11. Stage 1 needs
no suite A task list: the pre-registration fixes the model, the sampling and the *rules*
for suite A, and stage 2 records the task list later.

**Suite A rules.** No public SWE-rebench task is from September 2026 or later, so suite A takes
the newest ones: splits from `2026_03`, created on or after 2026-03-01 (110 tasks from 79
repositories on 2026-11-20). They predate Qwen3.8, so suite A is reported as possibly
contaminated. With so many small repositories, 60 tasks are drawn as one task from each of 60
randomly drawn repositories (`select_suite_a` in [`SPEC.md`](SPEC.md) §2).

**Pinned versions.** Pi is `@earendil-works/pi-coding-agent` 1.5.0 on Node 22.23.4 (Pi
needs Node 22.19 or newer; the build checks it). Every harness is told the served window; Hone
gets it as `HONE_CONTEXT_WINDOW`. Hermes stays at v0.21.5 (`v2026.9.24`): since v0.22.1 one-shot
`hermes chat --format stream-json` runs through a gateway process, so current Hermes goes into
the follow-up campaign.

**Before pre-registering:** ask the Pi and Hermes maintainers, in one public post each,
to check their arm configuration in `config/arms.toml`.

```bash
cd benchmarks/harness-compare
cp config/bench.env.example config/bench.env
# Fill in every CHANGE-ME: HC_MODEL_REVISION (the weights' commit) and
# HC_VLLM_IMAGE (vLLM 0.26 or later, as <image>:<tag>@sha256:<digest>; serve and prereg refuse
# it without the digest). Set HC_HONE_REF to the commit under test.
docker buildx imagetools inspect vllm/vllm-openai:<tag>   # its "Digest:" line is sha256:<digest>
make preflight bundle            # host checks; build Hone, Pi and Hermes into the bundle
make serve stack                 # model server, proxy, sink, images
make smoke                       # every probe must PASS before you pre-register

make prereg NAME=c1 STAGE=cb     # writes prereg/c1/: no task selection, no image pulls
git switch -c bench/prereg-c1
git add prereg/c1
git commit -m "bench: pre-register campaign 1, stage cb"
git tag bench-prereg-c1
git push origin bench/prereg-c1 bench-prereg-c1   # the public timestamp is the point

make stack                       # rebuild the proxy and task images from the tagged code
make run-c CAMPAIGN=c1           # 25 scenarios × 6 arms × 3 repeats
make run-b CAMPAIGN=c1           # 10 tasks × 6 arms × 3 repeats
make report CAMPAIGN=c1          # report.md, runs.csv, redteam.csv
```

Stage 1 is done when all three hold:

1. The tag `bench-prereg-c1` was on GitHub before the first scored run.
2. `report.md` shows 0 infrastructure errors for C and B. Re-run failed runs with
   `scripts/10_run.sh c c1 --retry-errors` (and `b`).
3. Every change made after the tag is listed in `prereg/c1/DEVIATIONS.md`.

If `make serve` fails, it prints the server log. Fix `config/bench.env`; after the tag,
log the change as a deviation.

**Before the tag, after any change to `config/bench.env`:** run `make serve-down serve`
and `make stack`. `make serve` leaves a running server alone, and nothing yet checks the
server against the pre-registration (design §11.6). Don't re-run `make bundle` after the
tag: the report compares the bundle used with the pre-registered one and fails if they
differ.

**Check during smoke (unverified):** some vLLM versions skip speculative decoding for
requests that set a presence or repetition penalty. Look at the speculative-decoding
counters in `make serve` logs after the smoke run. If decoding is not speculative, the
runs are still valid and every arm is affected equally, but the compute estimate in the
design (§10.5) is too low. Do not change the sampling to get the speed back.

`make report` checks the campaign against the pre-registration and fails if anything
differs: every recorded request (forced sampling and seed), every invocation (bundle,
suite and arms files, rig code), and the pre-registration itself. Its "Conformance"
section lists what it found.

**After the tag the rig's code is frozen.** Everything in this directory is code except
`README.md`, `SPEC.md` and `tests/` (and `prereg/`, which the check leaves out). Any other
change, including a fix to the report itself, a `.gitignore` edit or a new file, is a
mismatch in every later report. Run `make report` from the tagged code. If code has to
change, log it in `prereg/c1/DEVIATIONS.md`.

**Images.** The three Dockerfiles pin `python:3.12-slim` by digest, and the pre-registration
refuses an unpinned `FROM`. The Debian packages they install, and the dependencies of
`pytest` and `aiohttp`, are not pinned: build the images before smoke (`make stack`) and
rebuild on the same box, where Docker reuses the cached layers. Don't prune the build cache
(`docker builder prune`, `docker system prune`) until the campaign ends, and keep disk space
free: Docker can also evict the cache by itself when the disk fills. If `make stack` after
the tag rebuilds an image from scratch, log it in `prereg/c1/DEVIATIONS.md`.

Before the tag, `git status --ignored --short .` should list only build outputs, caches
and tool folders (`.bundle/`, `.build/`, `results/`, `.venv/`, `__pycache__/`,
`.pytest_cache/`, `.ruff_cache/`, `.claude/`, `uv.lock`), editor swap or backup files,
`.DS_Store`, and `config/bench.env`: the code check can't see ignored files.

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
2. Leave the proxy overrides (model, temperature, top_p, top_k, min_p, presence_penalty,
   repetition_penalty, max_tokens, thinking off, seed) and the caps alone after
   pre-registration.
3. Never tune prompts per arm. Every arm gets `config/prompts/swebench.md` verbatim.
4. Publish everything: `scripts/30_report.sh <campaign> --package` writes the trajectories,
   predictions, grader outcomes and pre-registration, with SHA256SUMS.
