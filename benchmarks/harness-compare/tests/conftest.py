"""Shared fixtures for the campaign 1 tests."""

import json
import os
import subprocess
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
    # K42: a non-draft pre-registration is refused when the rig directory is dirty, and the
    # rig under test has uncommitted files. Point git at a private snapshot of the rig so the
    # in-process tests see a clean tree whatever state the working copy is in.
    gitdir = tmp_path / "snapshot" / ".git"
    base = ["git", f"--git-dir={gitdir}", f"--work-tree={ROOT}"]
    ident = ["-c", "user.name=t", "-c", "user.email=t@t", "-c", "commit.gpgsign=false"]
    subprocess.run(["git", "init", "-q", str(gitdir.parent)], check=True, capture_output=True)
    subprocess.run([*base, "add", "-A", "."], cwd=ROOT, check=True, capture_output=True)
    subprocess.run(
        [*base, *ident, "commit", "-q", "-m", "snapshot"], cwd=ROOT, check=True, capture_output=True
    )
    monkeypatch.setenv("GIT_DIR", str(gitdir))
    monkeypatch.setenv("GIT_WORK_TREE", str(ROOT))
    # prereg.main() -> ensure_env() writes os.environ directly; undo that for later tests.
    saved = dict(os.environ)
    yield {"placeholders": placeholders, "manifest": manifest, "tmp": tmp_path, "mp": monkeypatch}
    os.environ.clear()
    os.environ.update(saved)
