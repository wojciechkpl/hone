"""Spec section 12 (pre-registration of Hermes): HC_HERMES_REF is captured like HC_PI_VERSION."""

import json
import tomllib
from pathlib import Path

import pytest

from hcbench import prereg
from hcbench.config import load_env_file

ROOT = Path(__file__).resolve().parent.parent


def test_env_keys_should_record_hermes_ref_next_to_other_harness_versions():
    assert "HC_HERMES_REF" in prereg.ENV_KEYS
    assert "HC_PI_VERSION" in prereg.ENV_KEYS  # unchanged


def test_bench_env_example_should_define_a_nonempty_hermes_ref():
    env = load_env_file(ROOT / "config" / "bench.env.example", base={"HOME": "/h"})
    assert env.get("HC_HERMES_REF", "").strip() != ""


def _run(tmp_path, monkeypatch, ref, draft=True):
    sel = tmp_path / "selection.json"
    sel.write_text("{}")
    out = tmp_path / "preregistration.toml"
    monkeypatch.setenv("HC_HERMES_REF", ref)
    monkeypatch.setenv("HC_BUNDLE_DIR", str(tmp_path / "nobundle"))
    args = ["--selection", str(sel), "--out", str(out)]
    if draft:
        args.append("--draft")
    return prereg.main(args), out


def test_prereg_should_write_hermes_ref_into_config_table(tmp_path, monkeypatch):
    rc, out = _run(tmp_path, monkeypatch, "v2026.11.5")
    assert rc == 0
    data = tomllib.loads(out.read_text())
    assert data["config"]["HC_HERMES_REF"] == "v2026.11.5"


def test_prereg_when_hermes_ref_is_placeholder_and_not_draft_should_refuse(tmp_path, monkeypatch):
    with pytest.raises(SystemExit) as exc:
        _run(tmp_path, monkeypatch, "CHANGE-ME-tag-or-sha", draft=False)
    assert "HC_HERMES_REF" in str(exc.value)


# ---- round 2: bundle manifest must agree with the pre-registered Hermes version ----

SHA = "f97608f178d1ffeca59860195ab7da295f7c8e5f"


def _prereg(tmp_path, monkeypatch, manifest, ref="v2026.9.24", sha=SHA, arms="pi,hermes"):
    bundle = tmp_path / "bundle"
    bundle.mkdir(exist_ok=True)
    if manifest is not None:
        (bundle / "manifest.json").write_text(json.dumps(manifest))
    sel = tmp_path / "selection.json"
    sel.write_text("{}")
    envf = tmp_path / "empty.env"
    envf.write_text("")
    out = tmp_path / "pre.toml"
    monkeypatch.setenv("HC_BUNDLE_DIR", str(bundle))
    monkeypatch.setenv("HC_ARMS_MAIN", arms)
    monkeypatch.setenv("HC_ARMS_REDTEAM", arms)
    monkeypatch.setenv("HC_HERMES_REF", ref)
    if sha is None:
        monkeypatch.delenv("HC_HERMES_SHA", raising=False)
    else:
        monkeypatch.setenv("HC_HERMES_SHA", sha)
    rc = prereg.main(
        ["--selection", str(sel), "--out", str(out), "--env-file", str(envf), "--draft"]
    )
    return rc, out


GOOD = {"hermes_ref": "v2026.9.24", "hermes_sha": SHA}


def test_prereg_should_record_hermes_sha_and_accept_consistent_manifest(tmp_path, monkeypatch):
    rc, out = _prereg(tmp_path, monkeypatch, GOOD)
    assert rc == 0
    assert tomllib.loads(out.read_text())["config"]["HC_HERMES_SHA"] == SHA


def test_env_keys_should_include_hermes_sha_and_example_should_define_it():
    assert "HC_HERMES_SHA" in prereg.ENV_KEYS
    env = load_env_file(ROOT / "config" / "bench.env.example", base={"HOME": "/h"})
    assert len(env.get("HC_HERMES_SHA", "")) == 40


@pytest.mark.parametrize(
    "manifest",
    [
        {"hermes_ref": "v2026.9.24"},  # no hermes_sha
        {"hermes_ref": "v2026.9.24", "hermes_sha": ""},
        {"hermes_ref": "v2026.9.23", "hermes_sha": SHA},  # ref differs from HC_HERMES_REF
        {"hermes_ref": "v2026.9.24", "hermes_sha": "0" * 40},  # SHA differs from HC_HERMES_SHA
        None,  # no manifest at all
    ],
)
def test_prereg_when_hermes_arm_listed_and_manifest_inconsistent_should_refuse(
    tmp_path, monkeypatch, manifest
):
    with pytest.raises(SystemExit):
        _prereg(tmp_path, monkeypatch, manifest)


def test_prereg_when_hermes_sha_env_differs_from_manifest_should_refuse(tmp_path, monkeypatch):
    with pytest.raises(SystemExit) as exc:
        _prereg(tmp_path, monkeypatch, GOOD, sha="1" * 40)
    assert "HC_HERMES_SHA" in str(exc.value)


def test_prereg_when_hermes_arm_only_in_redteam_list_should_still_check(tmp_path, monkeypatch):
    with pytest.raises(SystemExit):
        _prereg(tmp_path, monkeypatch, {"hermes_ref": "v2026.9.24"}, arms="pi,hermes-guarded")


def test_prereg_when_no_hermes_arm_listed_should_not_require_hermes_manifest(tmp_path, monkeypatch):
    rc, _ = _prereg(tmp_path, monkeypatch, {}, arms="pi,hone-inject")
    assert rc == 0
