"""Shared fixtures for the campaign 1 tests."""

import json
import os
from pathlib import Path

import pytest

from hcbench.config import load_env_file

ROOT = Path(__file__).resolve().parent.parent
EXAMPLE = ROOT / "config" / "bench.env.example"


@pytest.fixture
def env(tmp_path, monkeypatch):
    """Campaign 1 defaults from the example file, placeholders pinned, no ambient HC_*."""
    for k in [k for k in os.environ if k.startswith("HC_")]:
        monkeypatch.delenv(k)
    vals = load_env_file(EXAMPLE, base={"HOME": "/h", "HC_ROOT": str(ROOT)})
    placeholders = [k for k, v in vals.items() if "CHANGE-ME" in v]
    bundle = tmp_path / "bundle"
    bundle.mkdir()
    manifest = {"hermes_ref": vals["HC_HERMES_REF"], "hermes_sha": vals["HC_HERMES_SHA"], "x": 1}
    (bundle / "manifest.json").write_text(json.dumps(manifest))
    monkeypatch.setenv("HC_BUNDLE_DIR", str(bundle))
    # prereg.main() -> ensure_env() writes os.environ directly; undo that for later tests.
    saved = dict(os.environ)
    yield {"placeholders": placeholders, "manifest": manifest, "tmp": tmp_path, "mp": monkeypatch}
    os.environ.clear()
    os.environ.update(saved)
