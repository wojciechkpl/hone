"""Spec section 11.14, K78-K82 (review of round 10, 2026-11-20).

Uses the offline build of test_campaign1_round10.py. Its fake npm container now fails when the
Pi prefix exists as it starts and logs ["npm-node", <bundle node --version>], so a build that
keeps part of an old prefix, or probes/replaces Node after npm, cannot pass (K80).

K78: a build that stops part-way leaves no manifest.json. K79: manifest field
node_installed_version (probe output, null without a working Node, whatever the flags) and
check_pi_bundle refuses a Pi arm when it is not "v" + HC_NODE_VERSION. K81: check-node on a
broken package.json exits with the "by hand" message. K82: SPEC.md names the new fields.
"""

import json
import os
import subprocess
import sys

import pytest
from fakebin import ROOT
from test_campaign1_prereg import pin, run_cb
from test_campaign1_round10 import (
    Build,
    good_manifest,
    pi_env,
    run_main,
    set_manifest,
    write_exec,
    write_manifest,
    write_pkg_json,
)

from hcbench import prereg


@pytest.fixture
def build(tmp_path):
    return Build(tmp_path)


def npm_nodes(build):
    return [c[1] for c in build.calls("npm-node")]


# ---- K80 the fake npm container catches the round-9 bugs -----------------------------------


def test_k80_npm_ran_under_the_pinned_node_after_a_stale_node_was_replaced(build):
    build.node("22.11.0")
    r = build.run()
    assert r.returncode == 0, r.stderr
    assert npm_nodes(build) == ["v22.23.4"]


def test_k80_npm_ran_under_the_pinned_node_when_there_was_no_node(build):
    assert build.run().returncode == 0
    assert npm_nodes(build) == ["v22.23.4"]


def test_k80_a_sentinel_in_the_old_node_tree_is_gone_after_the_download(build):
    build.node("22.11.0")
    (build.B / "node" / "lib").mkdir(parents=True)
    (build.B / "node" / "lib" / "SENTINEL").write_text("old tree")
    assert build.run().returncode == 0
    assert not (build.B / "node" / "lib" / "SENTINEL").exists()
    assert (build.B / "node" / "bin" / "node").is_file()


def test_k80_a_whole_old_pi_prefix_is_removed_not_only_its_node_modules(build):
    pi = build.B / "pi"
    pi.mkdir()
    (pi / "package.json").write_text('{"old": true}')
    (pi / "package-lock.json").write_text("old lock")
    (pi / "SENTINEL").write_text("x")
    r = build.run()
    assert r.returncode == 0, r.stderr
    assert not (pi / "SENTINEL").exists()
    assert npm_nodes(build) == ["v22.23.4"]


def test_k80_the_matching_node_is_kept_and_npm_still_runs_under_it(build):
    build.node("22.23.4")
    (build.B / "node" / "lib").mkdir(parents=True)
    (build.B / "node" / "lib" / "SENTINEL").write_text("kept")
    assert build.run().returncode == 0
    assert build.calls("curl") == []
    assert (build.B / "node" / "lib" / "SENTINEL").exists()
    assert npm_nodes(build) == ["v22.23.4"]


# ---- K78 a failed build leaves no manifest -------------------------------------------------


def test_k78_a_build_that_fails_at_npm_leaves_no_manifest(build):
    assert build.run().returncode == 0
    assert (build.B / "manifest.json").is_file()  # a valid manifest from the first build
    r = build.run(FAKE_NPM_FAIL="1")
    assert r.returncode != 0
    assert npm_nodes(build)[-1] == "v22.23.4"  # npm did run (and fail)
    assert not (build.B / "manifest.json").exists()


def test_k78_a_build_that_fails_the_engines_check_leaves_no_manifest(build):
    assert build.run().returncode == 0
    r = build.run(FAKE_PI_ENGINES=">=99.0.0")
    assert r.returncode != 0
    assert not (build.B / "manifest.json").exists()


def test_k78_the_helpers_only_run_keeps_the_manifest_fields(build):
    assert build.run().returncode == 0
    first = build.manifest()
    r = subprocess.run(
        ["bash", str(build.rig / "scripts" / "01_build_bundle.sh"), "--helpers-only"],
        capture_output=True,
        text=True,
        env={
            "PATH": f"{build.bindir}:/usr/bin:/bin",
            "HOME": str(build.rig.parent),
            "HC_ENV_FILE": str(build.rig / "config" / "bench.env.example"),
            "HC_BUNDLE_DIR": str(build.B),
        },
        cwd=build.rig,
    )
    assert r.returncode == 0, r.stderr
    again = build.manifest()
    for k in ("pi_package", "pi_installed_version", "pi_lock_sha256", "node_installed_version"):
        assert again[k] == first[k]


# ---- K79 the manifest's Node is the one on disk --------------------------------------------


def test_k79_the_manifest_records_the_node_the_probe_reports(build):
    assert build.run().returncode == 0
    m = build.manifest()
    assert m["node_installed_version"] == "v22.23.4"
    assert m["node_version"] == "22.23.4"  # still the env value


def test_k79_skip_pi_records_the_node_on_disk_not_the_env_value(build):
    build.node("22.20.1")
    assert build.run("--skip-pi").returncode == 0
    m = build.manifest()
    assert m["node_installed_version"] == "v22.20.1"
    assert m["node_version"] == "22.23.4"


def test_k79_the_node_field_is_null_when_the_bundle_has_no_node(build):
    assert build.run("--skip-pi").returncode == 0
    assert build.manifest()["node_installed_version"] is None


def test_k79_the_node_field_is_null_when_the_probe_fails(build):
    write_exec(build.B / "node" / "bin" / "node", "#!/bin/sh\nexit 1\n")
    assert build.run("--skip-pi").returncode == 0
    assert build.manifest()["node_installed_version"] is None


def test_k79_the_manifest_is_never_merged_from_an_earlier_one_for_node(build):
    (build.B / "manifest.json").write_text(json.dumps({"node_installed_version": "v1.0.0"}))
    assert build.run("--skip-pi").returncode == 0
    assert build.manifest()["node_installed_version"] is None


# ---- K79 check_pi_bundle checks the Node ---------------------------------------------------

BAD_NODE = [None, "", "v22.11.0", "22.23.4", "v22.23.5"]


@pytest.mark.parametrize("value", BAD_NODE, ids=repr)
def test_k79_a_node_other_than_the_pinned_one_is_refused_and_named(tmp_path, value):
    m = good_manifest(node_installed_version=value)
    with pytest.raises(SystemExit) as e:
        prereg.check_pi_bundle(pi_env(HC_ARMS_MAIN="pi"), write_manifest(tmp_path, m))
    assert "node_installed_version" in str(e.value.code)


def test_k79_a_missing_node_field_is_refused_and_named(tmp_path):
    m = good_manifest()
    del m["node_installed_version"]
    with pytest.raises(SystemExit) as e:
        prereg.check_pi_bundle(pi_env(HC_ARMS_B="pi-guarded"), write_manifest(tmp_path, m))
    assert "node_installed_version" in str(e.value.code)


def test_k79_the_node_is_compared_with_hc_node_version(tmp_path):
    env = pi_env(HC_ARMS_MAIN="pi", HC_NODE_VERSION="22.20.1")
    with pytest.raises(SystemExit):
        prereg.check_pi_bundle(env, write_manifest(tmp_path, good_manifest()))
    prereg.check_pi_bundle(
        env,
        write_manifest(tmp_path, good_manifest(node_installed_version="v22.20.1")),
    )


def test_k79_no_pi_arm_means_the_node_is_not_checked(tmp_path):
    env = pi_env(HC_ARMS_MAIN="hone-inject")
    prereg.check_pi_bundle(env, write_manifest(tmp_path, {"node_installed_version": None}))


def test_k79_a_non_draft_prereg_refuses_a_manifest_without_the_node_field(env):
    pin(env)
    set_manifest(env, {k: v for k, v in env["manifest"].items() if k != "node_installed_version"})
    with pytest.raises(SystemExit) as e:
        run_cb(env)
    assert "node_installed_version" in str(e.value.code)


def test_k79_a_non_draft_prereg_refuses_a_node_of_another_version(env):
    pin(env)
    set_manifest(env, {**env["manifest"], "node_installed_version": "v22.11.0"})
    with pytest.raises(SystemExit) as e:
        run_cb(env)
    assert "node_installed_version" in str(e.value.code)


def test_k79_a_draft_prereg_skips_the_node_check(env):
    set_manifest(env, {k: v for k, v in env["manifest"].items() if k != "node_installed_version"})
    rc, out = run_cb(env, "--draft")
    assert rc == 0 and out.exists()


# ---- K81 check-node on a broken package.json -----------------------------------------------


def write_raw(tmp_path, text):
    p = tmp_path / "package.json"
    p.write_text(text)
    return str(p)


@pytest.mark.parametrize("text", ["not json", "{", "", "[]", '"x"', "22", "null", '["engines"]'])
def test_k81_a_package_json_that_is_not_a_json_object_asks_for_a_manual_check(tmp_path, text):
    with pytest.raises(SystemExit) as e:
        run_main("check-node", "22.23.4", write_raw(tmp_path, text))
    assert "by hand" in str(e.value.code)


@pytest.mark.parametrize("text", ["not json", "[]"])
def test_k81_the_cli_process_prints_no_traceback(tmp_path, text):
    r = subprocess.run(
        [
            sys.executable,
            "-m",
            "hcbench.bundle",
            "check-node",
            "22.23.4",
            write_raw(tmp_path, text),
        ],
        cwd=ROOT,
        capture_output=True,
        text=True,
        env={**os.environ, "PYTHONPATH": str(ROOT)},
    )
    assert r.returncode != 0
    assert "Traceback" not in r.stderr
    assert "by hand" in r.stderr


def test_k81_a_valid_object_still_works(tmp_path):
    assert run_main("check-node", "22.23.4", write_pkg_json(tmp_path, {"name": "x"})) is None


# ---- K82 docs ------------------------------------------------------------------------------


@pytest.mark.parametrize(
    "needle", ["pi_installed_version", "node_installed_version", "--engine-strict"]
)
def test_k82_the_spec_names_the_installed_version_fields_and_engine_strict(needle):
    assert needle in (ROOT / "SPEC.md").read_text()
