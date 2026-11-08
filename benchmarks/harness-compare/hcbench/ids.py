"""Run identifiers: ``{suite}.{arm}.s{seed}.{task_id}``."""

from __future__ import annotations

import re

NAME_RE = re.compile(r"^[a-z0-9][a-z0-9_-]*$")
TASK_RE = re.compile(r"^[A-Za-z0-9_.-]+$")


def _check(suite: str, arm: str, seed: int, task_id: str) -> None:
    if not NAME_RE.match(suite):
        raise ValueError(f"invalid suite name: {suite!r}")
    if not NAME_RE.match(arm):
        raise ValueError(f"invalid arm name: {arm!r}")
    if isinstance(seed, bool) or not isinstance(seed, int) or seed < 0:
        raise ValueError(f"seed must be a non-negative int, got {seed!r}")
    if not task_id or not TASK_RE.match(task_id):
        raise ValueError(f"invalid task id: {task_id!r}")


def make_run_id(suite: str, arm: str, seed: int, task_id: str) -> str:
    _check(suite, arm, seed, task_id)
    return f"{suite}.{arm}.s{seed}.{task_id}"


def parse_run_id(run_id: str) -> tuple[str, str, int, str]:
    parts = run_id.split(".", 3)
    if len(parts) != 4:
        raise ValueError(f"malformed run id: {run_id!r}")
    suite, arm, seed_part, task_id = parts
    if not re.fullmatch(r"s\d+", seed_part):
        raise ValueError(f"malformed seed segment in run id: {run_id!r}")
    seed = int(seed_part[1:])
    _check(suite, arm, seed, task_id)
    return suite, arm, seed, task_id
