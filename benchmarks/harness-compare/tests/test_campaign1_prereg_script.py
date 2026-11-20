"""Spec section 11.2, K9/K10: scripts/05_preregister.sh and `make prereg ... STAGE=cb`.

Interface assumption: the stage reaches the script as `--stage cb` among its extra arguments
(`05_preregister.sh <name> --stage cb [--draft]`), like `--draft` does today.
"""

import json
import subprocess
import tomllib

from fakebin import calls, docker_calls, make_fake_bin, make_rig, run_script

from hcbench.config import load_env_file


def prereg_env(rig, tmp_path):
    vals = load_env_file(
        rig / "config" / "bench.env.example", base={"HOME": "/h", "HC_ROOT": str(rig)}
    )
    bundle = tmp_path / "bundle"
    bundle.mkdir()
    (bundle / "manifest.json").write_text(
        json.dumps(
            {
                "hermes_ref": vals["HC_HERMES_REF"],
                "hermes_sha": vals["HC_HERMES_SHA"],
                # K73: Pi fields a non-draft pre-registration needs
                "pi_package": "@earendil-works/pi-coding-agent",
                "pi_installed_version": vals["HC_PI_VERSION"],
                "pi_lock_sha256": "ab" * 32,
                "node_installed_version": "v" + vals["HC_NODE_VERSION"],
            }
        )
    )
    # Dataset revision pinned so the script never expands an empty array (bash 3.2 + set -u).
    return {"HC_BUNDLE_DIR": str(bundle), "HC_SWE_DATASET_REVISION": "abc"}


def test_k10_stage_cb_script_should_skip_selection_and_pulls_but_write_the_files(tmp_path):
    rig = make_rig(tmp_path)
    bindir, log = make_fake_bin(tmp_path)
    r = run_script(
        rig,
        "05_preregister.sh",
        ["t1", "--stage", "cb", "--draft"],
        bindir,
        prereg_env(rig, tmp_path),
    )
    assert r.returncode == 0, r.stderr
    d = rig / "prereg" / "t1"
    data = tomllib.loads((d / "preregistration.toml").read_text())
    assert data["stage"] == "cb"
    assert (d / "DEVIATIONS.md").read_text().startswith("# Deviations")
    assert not (d / "selection.json").exists()
    assert not (d / "images.list").exists()
    assert not any("hcbench.select_tasks" in " ".join(c) for c in calls(log)), (
        "task selection must not run"
    )
    assert not any(c[:1] == ["pull"] for c in docker_calls(log)), "no image pulls"


def test_k10_stage_cb_script_should_keep_an_existing_deviations_file(tmp_path):
    rig = make_rig(tmp_path)
    bindir, _ = make_fake_bin(tmp_path)
    d = rig / "prereg" / "t2"
    d.mkdir(parents=True)
    (d / "DEVIATIONS.md").write_text("mine\n")
    r = run_script(
        rig,
        "05_preregister.sh",
        ["t2", "--stage", "cb", "--draft"],
        bindir,
        prereg_env(rig, tmp_path),
    )
    assert r.returncode == 0, r.stderr
    assert (d / "DEVIATIONS.md").read_text() == "mine\n"


def test_k10_stage_a_script_should_still_select_tasks_and_pull_images_by_default(tmp_path):
    # K30: the return code is checked. select_tasks needs the network, so the fake uv stands in
    # for it and the files it would write are put in place beforehand.
    rig = make_rig(tmp_path)
    bindir, log = make_fake_bin(tmp_path)
    d = rig / "prereg" / "t3"
    d.mkdir(parents=True)
    (d / "selection.json").write_text(json.dumps({"dataset": "d", "n": 1, "digest": "x"}))
    (d / "tasks.txt").write_text("a__b-1\n")
    (d / "instances.jsonl").write_text(json.dumps({"instance_id": "a__b-1"}) + "\n")
    env = {**prereg_env(rig, tmp_path), "FAKE_UV_STUB_MODULES": "hcbench.select_tasks"}
    r = run_script(rig, "05_preregister.sh", ["t3", "--draft"], bindir, env)
    assert r.returncode == 0, r.stderr
    assert any("hcbench.select_tasks" in " ".join(c) for c in calls(log))
    assert any(c[:1] == ["pull"] for c in docker_calls(log))
    assert tomllib.loads((d / "preregistration.toml").read_text())["stage"] == "a"


def make_n(args: list[str]) -> str:
    from fakebin import ROOT

    r = subprocess.run(
        ["make", "-n", "prereg", *args], cwd=ROOT, capture_output=True, text=True, check=False
    )
    assert r.returncode == 0, r.stderr
    return r.stdout


def test_k9_make_prereg_should_pass_the_stage_through():
    out = make_n(["NAME=x", "STAGE=cb"])
    line = next(ln for ln in out.splitlines() if "05_preregister.sh" in ln and "test -n" not in ln)
    assert "--stage cb" in line
    assert " x" in line


def test_k9_make_prereg_without_stage_should_not_select_stage_cb():
    out = make_n(["NAME=x"])
    assert "--stage cb" not in out
    assert "05_preregister.sh x" in out
