"""Benchmark configuration read from the environment (config/bench.env)."""

from __future__ import annotations

import os
import re
from collections.abc import Mapping
from dataclasses import asdict, dataclass, field
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent
_LINE = re.compile(r"^\s*(?:export\s+)?([A-Za-z_][A-Za-z0-9_]*)=(.*)$")


def load_env_file(path: Path, base: Mapping[str, str] | None = None) -> dict[str, str]:
    """Parse KEY=VALUE lines (shell-style, no command substitution).

    ``${VAR}`` references are expanded against ``base`` (default: os.environ)
    plus keys defined earlier in the file. ``HC_ROOT`` defaults to the
    harness-compare directory.
    """
    scope = dict(base if base is not None else os.environ)
    scope.setdefault("HC_ROOT", str(ROOT))
    out: dict[str, str] = {}
    for raw in Path(path).read_text().splitlines():
        if not raw.strip() or raw.lstrip().startswith("#"):
            continue
        m = _LINE.match(raw)
        if not m:
            continue
        key, value = m.group(1), m.group(2).strip()
        if len(value) >= 2 and value[0] == value[-1] and value[0] in "'\"":
            value = value[1:-1]
        value = re.sub(r"\$\{(\w+)\}", lambda mm: scope.get(mm.group(1), ""), value)
        scope[key] = value
        out[key] = value
    return out


def ensure_env(path: Path | None = None) -> None:
    """Load bench.env into os.environ for keys not already set."""
    candidates = [path] if path else [ROOT / "config" / "bench.env"]
    for p in candidates:
        if p and Path(p).exists():
            for k, v in load_env_file(Path(p)).items():
                os.environ.setdefault(k, v)
            os.environ.setdefault("HC_ROOT", str(ROOT))
            return


def _token_file(root: Path) -> str:
    """Admin token written by scripts/03_stack.sh up (fallback when not in the env)."""
    try:
        return (root / "results" / ".admin-token").read_text().strip()
    except OSError:
        return ""


@dataclass(frozen=True)
class BenchConfig:
    model: str
    proxy_admin: str
    proxy_internal: str
    sink_admin: str
    network: str
    bundle_dir: Path
    max_requests: int
    max_total_tokens: int
    timeout_s: int
    context_window: int
    max_tokens: int
    cpus: str
    memory: str
    mini_image: str
    redteam_image: str
    swe_namespace: str
    price_in_per_m: float
    price_out_per_m: float
    pids_limit: int = 4096
    admin_token: str = field(default="", repr=False)

    @classmethod
    def from_env(cls, env: Mapping[str, str] | None = None) -> BenchConfig:
        e = dict(os.environ if env is None else env)
        root = Path(e.get("HC_ROOT", str(ROOT)))
        proxy_port = e.get("HC_PROXY_PORT", "8100")
        return cls(
            model=e.get("HC_SERVED_NAME", "qwen3-coder-30b-a3b"),
            proxy_admin=e.get("HC_PROXY_ADMIN", f"http://127.0.0.1:{proxy_port}"),
            proxy_internal=e.get("HC_PROXY_INTERNAL", f"http://hc-proxy:{proxy_port}"),
            sink_admin=e.get("HC_SINK_ADMIN", f"http://127.0.0.1:{e.get('HC_SINK_PORT', '8180')}"),
            network=e.get("HC_NET", "hc-net"),
            bundle_dir=Path(e.get("HC_BUNDLE_DIR", str(root / ".bundle"))),
            max_requests=int(e.get("HC_MAX_REQUESTS", "80")),
            max_total_tokens=int(e.get("HC_MAX_TOTAL_TOKENS", "3000000")),
            timeout_s=int(e.get("HC_TIMEOUT_S", "1800")),
            context_window=int(e.get("HC_MAX_MODEL_LEN", "65536")),
            max_tokens=int(e.get("HC_MAX_TOKENS", "6144")),
            cpus=e.get("HC_CPUS", "4"),
            memory=e.get("HC_MEMORY", "8g"),
            mini_image=e.get("HC_MINI_IMAGE", "hc-mini:latest"),
            redteam_image=e.get("HC_REDTEAM_IMAGE", "hc-redteam:latest"),
            swe_namespace=e.get("HC_SWE_NAMESPACE", "swebench"),
            price_in_per_m=float(e.get("HC_PRICE_IN_PER_M", "0") or 0),
            price_out_per_m=float(e.get("HC_PRICE_OUT_PER_M", "0") or 0),
            pids_limit=int(e.get("HC_PIDS_LIMIT", "4096")),
            admin_token=e.get("HC_ADMIN_TOKEN") or _token_file(root),
        )

    def as_dict(self) -> dict:
        """Serializable view without secrets (goes into campaign files and fingerprints)."""
        d = asdict(self)
        d.pop("admin_token", None)
        d["bundle_dir"] = str(self.bundle_dir)
        return d
