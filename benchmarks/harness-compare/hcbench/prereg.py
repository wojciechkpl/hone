"""Write preregistration.toml: everything that must not change after day 2.

    python -m hcbench.prereg --selection prereg/2026-11/selection.json \
        --out prereg/2026-11/preregistration.toml

Refuses placeholder values (CHANGE-ME) unless --draft is given.
"""

from __future__ import annotations

import argparse
import hashlib
import json
import os
import subprocess
import time
from pathlib import Path
from typing import Any

from .config import ROOT, ensure_env

ANALYSIS_PLAN = {
    "primary_metric": "pass@1 on suite A: resolved by the official swebench harness, one attempt "
    "per repeat; per-task mean over repeats, then mean over tasks",
    "ci": "95% percentile bootstrap over tasks (10000 resamples, seed 0)",
    "pairwise": "paired bootstrap of per-task differences + exact McNemar on repeat 0",
    "comparisons": "hone-inject vs pi; hone-noinject vs pi; hone-inject vs hone-noinject",
    "claim_rule": "a difference is claimed only if its 95% CI excludes 0; otherwise reported "
    "as 'no detectable difference at n=40'",
    "exclusions": "runs with infra errors are re-run (--retry-errors); grader errors are "
    "re-graded; nothing else is excluded",
    "secondary": "suite B pass rate; tokens, wall time, tool calls, read:edit ratio, guard "
    "events; suite C outcome counts per arm",
}

ENV_KEYS = [
    "HC_SERVER", "HC_SERVED_NAME", "HC_MODEL_HF", "HC_MODEL_REVISION", "HC_TOOL_PARSER",
    "HC_VLLM_IMAGE", "HC_LLAMACPP_IMAGE", "HC_GGUF_PATH", "HC_MAX_MODEL_LEN", "HC_KV_CACHE_DTYPE",
    "HC_MAX_NUM_SEQS", "HC_GPU_MEM_UTIL", "HC_TEMPERATURE", "HC_TOP_P", "HC_MAX_TOKENS",
    "HC_MAX_REQUESTS", "HC_MAX_TOTAL_TOKENS", "HC_TIMEOUT_S", "HC_CPUS", "HC_MEMORY",
    "HC_REPEATS", "HC_ARMS_MAIN", "HC_ARMS_REDTEAM", "HC_PRICE_IN_PER_M", "HC_PRICE_OUT_PER_M",
    "HC_PI_VERSION", "HC_NODE_VERSION", "HC_HONE_REF",
]  # fmt: skip


def sha256_file(path: Path) -> str:
    return hashlib.sha256(Path(path).read_bytes()).hexdigest()


def _git(*args: str) -> str:
    r = subprocess.run(["git", *args], capture_output=True, text=True, cwd=ROOT, check=False)
    return r.stdout.strip()


def _toml_value(v: Any) -> str:
    if isinstance(v, bool):
        return "true" if v else "false"
    if isinstance(v, int | float):
        return repr(v)
    if isinstance(v, list | tuple):
        return "[" + ", ".join(_toml_value(x) for x in v) + "]"
    if v is None:
        return '""'
    return json.dumps(str(v), ensure_ascii=False)


def dump_toml(data: dict, prefix: str = "") -> str:
    scalars = {k: v for k, v in data.items() if not isinstance(v, dict)}
    tables = {k: v for k, v in data.items() if isinstance(v, dict)}
    lines = [f"{k} = {_toml_value(v)}" for k, v in scalars.items()]
    out = "\n".join(lines) + ("\n" if lines else "")
    for k, v in tables.items():
        name = f"{prefix}.{k}" if prefix else k
        out += f"\n[{name}]\n" + dump_toml(v, name)
    return out


def main(argv: list[str] | None = None) -> int:
    ap = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawTextHelpFormatter)
    ap.add_argument("--selection", type=Path, required=True)
    ap.add_argument("--out", type=Path, required=True)
    ap.add_argument("--draft", action="store_true", help="allow CHANGE-ME placeholders")
    ap.add_argument("--env-file", type=Path)
    args = ap.parse_args(argv)
    ensure_env(args.env_file)

    env = {k: os.environ.get(k, "") for k in ENV_KEYS}
    placeholders = [k for k, v in env.items() if "CHANGE-ME" in v]
    if placeholders and not args.draft:
        raise SystemExit(f"refusing to pre-register with placeholders: {placeholders}")

    selection = json.loads(args.selection.read_text())
    bundle_manifest = Path(os.environ.get("HC_BUNDLE_DIR", ROOT / ".bundle")) / "manifest.json"
    files = {
        "arms_toml": ROOT / "config" / "arms.toml",
        "prompt_swebench": ROOT / "config" / "prompts" / "swebench.md",
        "suite_b": ROOT / "suites" / "b_mini.toml",
        "suite_c": ROOT / "suites" / "c_redteam.toml",
        "suite_smoke": ROOT / "suites" / "smoke.toml",
        "tasks_txt": args.selection.parent / "tasks.txt",
    }
    data = {
        "title": "Hone vs Pi, harness-controlled, one local model",
        "created_at": time.strftime("%Y-%m-%dT%H:%M:%SZ", time.gmtime()),
        "draft": bool(placeholders),
        "repo": {
            "head": _git("rev-parse", "HEAD"),
            "dirty": bool(_git("status", "--porcelain", "--", ".")),
        },
        "config": env,
        "tasks": {
            k: selection.get(k)
            for k in ("dataset", "split", "revision", "resolved_revision", "n", "seed", "digest")
        },
        "files_sha256": {k: sha256_file(p) for k, p in files.items() if p.exists()},
        "bundle": json.loads(bundle_manifest.read_text()) if bundle_manifest.exists() else {},
        "analysis": ANALYSIS_PLAN,
    }
    args.out.parent.mkdir(parents=True, exist_ok=True)
    args.out.write_text(
        "# Pre-registration. Commit and tag BEFORE any suite A/B/C run.\n"
        "# Any later change to these values is a deviation: log it in DEVIATIONS.md.\n\n"
        + dump_toml(data)
    )
    print(f"wrote {args.out}")
    if not data["bundle"]:
        print("WARNING: no bundle manifest found; run scripts/01_build_bundle.sh first")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
