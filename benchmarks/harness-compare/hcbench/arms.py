"""Harness arm definitions (argv/env/files templates) and rendering."""

from __future__ import annotations

import re
import tomllib
from collections.abc import Mapping
from dataclasses import dataclass, field
from pathlib import Path

from .ids import NAME_RE

KINDS = frozenset({"pi", "hone", "selftest"})
PLACEHOLDER_RE = re.compile(r"\{\{([a-z_][a-z0-9_]*)\}\}")


@dataclass(frozen=True)
class Arm:
    name: str
    kind: str
    argv: tuple[str, ...]
    env: Mapping[str, str] = field(default_factory=dict)
    files: Mapping[str, str] = field(default_factory=dict)


def load_arms(path: Path) -> dict[str, Arm]:
    data = tomllib.loads(Path(path).read_text())
    arms: dict[str, Arm] = {}
    for name, spec in (data.get("arms") or {}).items():
        if not NAME_RE.match(name):
            raise ValueError(f"invalid arm name: {name!r}")
        kind = spec.get("kind")
        if kind not in KINDS:
            raise ValueError(f"arm {name!r}: unknown kind {kind!r}")
        argv = spec.get("argv")
        if not isinstance(argv, list) or not argv or not all(isinstance(a, str) for a in argv):
            raise ValueError(f"arm {name!r}: argv must be a non-empty list of strings")
        env = {str(k): str(v) for k, v in (spec.get("env") or {}).items()}
        files = {str(k): str(v) for k, v in (spec.get("files") or {}).items()}
        arms[name] = Arm(name=name, kind=kind, argv=tuple(argv), env=env, files=files)
    return arms


def _sub(text: str, ctx: Mapping[str, str]) -> str:
    def repl(m: re.Match[str]) -> str:
        key = m.group(1)
        if key not in ctx:
            raise KeyError(key)
        return str(ctx[key])

    return PLACEHOLDER_RE.sub(repl, text)


def render(arm: Arm, ctx: Mapping[str, str]) -> tuple[list[str], dict[str, str], dict[str, str]]:
    argv = [_sub(a, ctx) for a in arm.argv]
    env = {k: _sub(v, ctx) for k, v in arm.env.items()}
    files = {_sub(p, ctx): _sub(c, ctx) for p, c in arm.files.items()}
    return argv, env, files
