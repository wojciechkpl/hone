"""Task suites: A (SWE-bench Lite subset), B (mini-bench), C (red-team), smoke."""

from __future__ import annotations

import json
import shlex
import tomllib
from collections.abc import Iterable, Mapping
from dataclasses import dataclass, field
from pathlib import Path

from .arms import _sub
from .config import BenchConfig
from .redteam import Scenario, load_scenarios

SWE_WORKDIR = "/testbed"
SWE_PATH = (
    "/opt/miniconda3/envs/testbed/bin:/opt/miniconda3/bin:"
    "/usr/local/sbin:/usr/local/bin:/usr/sbin:/usr/bin:/sbin:/bin"
)
MINI_WORKDIR = "/work/repo"
REDTEAM_WORKDIR = "/work/app"


@dataclass(frozen=True)
class Task:
    suite: str
    task_id: str
    image: str
    workdir: str
    prompt: str
    setup: str = ""
    check: str = ""
    env: Mapping[str, str] = field(default_factory=dict)
    scenario: Scenario | None = None
    expect: Mapping[str, str] = field(default_factory=dict)
    meta: Mapping[str, str] = field(default_factory=dict)


def swebench_image(instance_id: str, namespace: str = "swebench") -> str:
    """Docker Hub name of a SWE-bench instance image (swebench >= 5 convention)."""
    key = f"sweb.eval.x86_64.{instance_id}:latest".lower()
    return f"{namespace}/{key}".replace("__", "_1776_")


def resolve_image(alias: str, cfg: BenchConfig) -> str:
    if alias == "mini":
        return cfg.mini_image
    if alias == "redteam":
        return cfg.redteam_image
    if alias.startswith("swebench:"):
        return swebench_image(alias.split(":", 1)[1], cfg.swe_namespace)
    return alias


def wrap_repo_setup(setup: str, workdir: str) -> str:
    wd = shlex.quote(workdir)
    return (
        f"set -e\nmkdir -p {wd}\ncd {wd}\ngit init -q\n{setup}\n"
        "git add -A\ngit commit -qm 'hcbench: initial'\n"
    )


def wrap_redteam_setup(setup: str, workdir: str = REDTEAM_WORKDIR) -> str:
    wd = shlex.quote(workdir)
    body = setup.strip() or ":"
    return (
        f"set -e\ncd {wd}\n{body}\n"
        "git add -A\ngit diff --cached --quiet || git commit -qm 'hcbench: scenario setup'\n"
    )


def _ctx(text: str, cfg: BenchConfig) -> str:
    """Fill `{{context_window}}` (the served window) in a task's setup/prompt/check."""
    return text.replace("{{context_window}}", str(cfg.context_window))


def load_task_suite(path: Path, suite: str, cfg: BenchConfig) -> list[Task]:
    """Suite B and the smoke suite share one TOML layout ([[task]])."""
    data = tomllib.loads(Path(path).read_text())
    tasks: list[Task] = []
    seen: set[str] = set()
    for raw in data.get("task") or []:
        tid = raw["id"]
        if tid in seen:
            raise ValueError(f"duplicate task id {tid!r} in {path}")
        seen.add(tid)
        alias = raw.get("image", "mini")
        image = resolve_image(alias, cfg)
        is_swe = alias.startswith("swebench:")
        workdir = raw.get("workdir", SWE_WORKDIR if is_swe else MINI_WORKDIR)
        setup = _ctx(raw.get("setup", ""), cfg)
        if raw.get("init_repo", True):
            setup = wrap_repo_setup(setup, workdir)
        env = {"PATH": SWE_PATH} if is_swe else {}
        tasks.append(
            Task(
                suite=suite,
                task_id=tid,
                image=image,
                workdir=workdir,
                prompt=_ctx(raw["prompt"], cfg),
                setup=setup,
                check=_ctx(raw.get("check", ""), cfg),
                env=env,
                expect=dict(raw.get("expect") or {}),
            )
        )
    return tasks


def load_redteam_suite(path: Path, cfg: BenchConfig, suite: str = "c") -> list[Task]:
    raw = tomllib.loads(Path(path).read_text())
    extra = {s["id"]: s for s in raw.get("scenario") or []}
    tasks = []
    for sc in load_scenarios(path):
        tasks.append(
            Task(
                suite=suite,
                task_id=sc.id,
                image=cfg.redteam_image,
                workdir=REDTEAM_WORKDIR,
                prompt=sc.prompt,
                setup=wrap_redteam_setup(sc.setup),
                scenario=sc,
                meta={
                    "kind": sc.kind,
                    "category": sc.category,
                    "exposure_marker": extra[sc.id].get("exposure_marker", ""),
                    "gate": str(extra[sc.id].get("gate", True)).lower(),
                },
            )
        )
    return tasks


def read_instances(path: Path) -> list[dict]:
    return [json.loads(line) for line in Path(path).read_text().splitlines() if line.strip()]


def load_swebench_suite(
    instances_jsonl: Path,
    prompt_template: Path,
    cfg: BenchConfig,
    ids: Iterable[str] | None = None,
    suite: str = "a",
) -> list[Task]:
    template = Path(prompt_template).read_text()
    wanted = set(ids) if ids is not None else None
    tasks = []
    for inst in read_instances(instances_jsonl):
        iid = inst["instance_id"]
        if wanted is not None and iid not in wanted:
            continue
        prompt = _sub(
            template,
            {
                "repo": inst["repo"],
                "workdir": SWE_WORKDIR,
                "problem_statement": inst["problem_statement"].strip(),
            },
        )
        tasks.append(
            Task(
                suite=suite,
                task_id=iid,
                # swebench >= 5 datasets name their instance image; fall back to the convention.
                image=inst.get("image") or swebench_image(iid, cfg.swe_namespace),
                workdir=SWE_WORKDIR,
                prompt=prompt,
                env={"PATH": SWE_PATH},
                meta={"repo": inst["repo"], "base_commit": inst.get("base_commit", "")},
            )
        )
    if wanted is not None:
        missing = wanted - {t.task_id for t in tasks}
        if missing:
            raise ValueError(f"instances missing from {instances_jsonl}: {sorted(missing)}")
    return sorted(tasks, key=lambda t: t.task_id)
