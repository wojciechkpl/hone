"""Spec section 11.17, round 15b, K129-K133: classify the fork's grading failures.

Contract choices made here (the spec leaves them open):
* K129/K130 are tested through ``hcbench.grade.main`` with ``subprocess.run`` faked. The fake
  writes the fork-style report and the logs under
  ``<grading dir>/logs/run_evaluation/<run_id>/<model>/<instance>/`` the way the fork does.
  Statuses are read from ``grading/outcomes.csv``: ``grader_infra`` and ``ambiguous`` have a
  blank ``resolved``, ``ambiguous_repeated`` has ``0``.
* K132: ``hcbench.grade.package_version(name) -> str`` reads a package version with
  ``importlib.metadata.version``; ``grading/grader.json`` gets ``swebench_version`` and
  ``docker_version``.
* K131: ``prereg/<name>/preregistration.toml`` holds the stage-a ``[grader]`` table
  (``repo``, ``commit``, ``namespace``). The environment values count as set only when the
  env file or the process sets them, so these tests use an env file without them.
"""
# ruff: noqa: F811, F401  (the fixtures `stage_a_env` and `hub` are imported by name)

import csv
import importlib.metadata
import json
import subprocess

import pytest
from fakebin import ROOT, calls, env_file_without, make_fake_bin, make_rig, run_script
from test_campaign1_prereg import EXAMPLE, pin
from test_campaign1_round14 import run_a, stage_a_env
from test_campaign1_round14b import preflight
from test_campaign1_round15 import FORK, IDS3, NS, REF, fork_inputs, grade_args, make_campaign
from test_campaign1_round15 import flag as cmd_flag

from hcbench import grade, prereg

# ---- a fake run_evaluation that leaves logs behind ----


class ForkEvaluate:
    """One scripted step per call; the last step repeats. A step is
    {"report": {...}, "logs": {instance: {filename: text}}}."""

    def __init__(self, *steps):
        self.steps, self.cmds = steps, []

    def __call__(self, cmd, **kw):
        step = self.steps[min(len(self.cmds), len(self.steps) - 1)]
        self.cmds.append(list(cmd))
        cwd = kw["cwd"]
        report = {"resolved_ids": [], **step.get("report", {})}  # the fork always writes it
        (cwd / "hcbench.report.json").write_text(json.dumps(report))
        run_id = cmd_flag(cmd, "--run_id")
        for iid, files in step.get("logs", {}).items():
            d = cwd / "logs" / "run_evaluation" / run_id / "model" / iid
            d.mkdir(parents=True, exist_ok=True)
            for name, text in files.items():
                (d / name).write_text(text)
        return subprocess.CompletedProcess(cmd, 0)


def outcomes(camp):
    with (camp / "grading" / "outcomes.csv").open() as fh:
        return {
            r["run_id"].split(".", 3)[-1]: (r["resolved"], r["status"]) for r in csv.DictReader(fh)
        }


def run_fork(tmp_path, monkeypatch, steps, ids=("a__a-1",), *extra, fork=True, camp=None):
    camp = camp or make_campaign(tmp_path, list(ids))
    d, _ = fork_inputs(tmp_path, list(ids)) if not (tmp_path / "prereg").exists() else (
        tmp_path / "prereg", None)  # fmt: skip
    ev = ForkEvaluate(*steps)
    monkeypatch.setattr(grade.subprocess, "run", ev)
    if fork:
        args = grade_args(camp, d)
    else:
        args = ["--campaign", str(camp), "--dataset", str(d / "dataset.json"), "--split", "test"]
    rc = grade.main([*args, *extra])
    return rc, camp, ev


def step(bucket, iid, text, filename="test_output.txt", **more):
    return {"report": {bucket: [iid], **more}, "logs": {iid: {filename: text}}}


AMB = "Timeout error: 1800 seconds exceeded."
INFRA = "Error creating container"

# ---- K129 the signatures ----

ENVIRONMENT = [
    ("Error building image", "error_ids", "run_instance.log"),
    ("Error occurred while pulling image x", "unresolved_ids", "run_instance.log"),
    ("Error creating container", "error_ids", "run_instance.log"),
    ("Read timed out", "unresolved_ids", "run_instance.log"),
    ("UnixHTTPConnectionPool(host='localhost')", "error_ids", "run_instance.log"),
    ("Failed to launch the browser", "error_ids", "test_output.txt"),
    ("Failed to connect to the bus", "unresolved_ids", "test_output.txt"),
    ("cannot open display", "error_ids", "test_output.txt"),
    ("Missing X server", "unresolved_ids", "run_instance.log"),
    ("unable to open X display", "error_ids", "run_instance.log"),
    ("Cannot allocate memory", "error_ids", "test_output.txt"),
    ("OutOfMemoryError", "unresolved_ids", "test_output.txt"),
    ("something\nKilled\nelse", "error_ids", "test_output.txt"),
    ("Error response from daemon: conflict", "error_ids", "run_instance.log"),
    ("Cannot connect to the Docker daemon", "unresolved_ids", "run_instance.log"),
    ("Could not resolve host: pypi.org", "error_ids", "test_output.txt"),
    ("Temporary failure in name resolution", "unresolved_ids", "test_output.txt"),
]
AMBIGUOUS = [
    ("Cannot find module 'x'", "error_ids", "test_output.txt"),
    ("MODULE_NOT_FOUND", "unresolved_ids", "test_output.txt"),
    ("ModuleNotFoundError: No module named 'y'", "error_ids", "run_instance.log"),
    ("no tests ran in 0.1s", "unresolved_ids", "test_output.txt"),
    ("collected 0 items", "error_ids", "test_output.txt"),
    ("Timeout error: 1800 seconds exceeded.", "error_ids", "test_output.txt"),
]


@pytest.mark.parametrize(("text", "bucket", "filename"), ENVIRONMENT)
def test_k129_environment_signature_should_be_grader_infra(
    tmp_path, monkeypatch, text, bucket, filename
):
    rc, camp, _ = run_fork(tmp_path, monkeypatch, [step(bucket, "a__a-1", text, filename)])
    assert outcomes(camp)["a__a-1"] == ("", "grader_infra")
    assert rc == 1  # to be re-graded


@pytest.mark.parametrize(("text", "bucket", "filename"), AMBIGUOUS)
def test_k129_ambiguous_signature_should_be_ambiguous(
    tmp_path, monkeypatch, text, bucket, filename
):
    rc, camp, _ = run_fork(tmp_path, monkeypatch, [step(bucket, "a__a-1", text, filename)])
    assert outcomes(camp)["a__a-1"] == ("", "ambiguous")
    assert rc == 1


@pytest.mark.parametrize("bucket", ["error_ids", "unresolved_ids"])
@pytest.mark.parametrize("filename", ["run_instance.log", "test_output.txt"])
def test_k129_both_buckets_and_both_files_should_be_read(tmp_path, monkeypatch, bucket, filename):
    _, camp, _ = run_fork(tmp_path, monkeypatch, [step(bucket, "a__a-1", INFRA, filename)])
    assert outcomes(camp)["a__a-1"][1] == "grader_infra"


@pytest.mark.parametrize(
    ("bucket", "want"), [("error_ids", "error"), ("unresolved_ids", "unresolved")]
)
@pytest.mark.parametrize(
    "text",
    [
        "AssertionError: 1 != 2",
        "Killed in action",  # ^Killed$ is a whole line
        "Timeout error: seconds exceeded",  # needs digits
        "",
    ],
)
def test_k129_no_signature_should_keep_the_status(tmp_path, monkeypatch, bucket, want, text):
    rc, camp, _ = run_fork(tmp_path, monkeypatch, [step(bucket, "a__a-1", text)])
    assert outcomes(camp)["a__a-1"] == ("0", want)
    assert rc == 0


def test_k129_an_instance_without_any_log_should_keep_the_status(tmp_path, monkeypatch):
    _, camp, _ = run_fork(tmp_path, monkeypatch, [{"report": {"error_ids": ["a__a-1"]}}])
    assert outcomes(camp)["a__a-1"] == ("0", "error")


def test_k129_the_first_signature_should_win_so_the_environment_tier_beats_ambiguous(
    tmp_path, monkeypatch
):
    text = "ModuleNotFoundError: x\nTimeout error: 5 seconds exceeded.\nCannot allocate memory"
    _, camp, _ = run_fork(tmp_path, monkeypatch, [step("error_ids", "a__a-1", text)])
    assert outcomes(camp)["a__a-1"][1] == "grader_infra"


def test_k129_signatures_may_be_split_over_the_two_files(tmp_path, monkeypatch):
    s = {"report": {"error_ids": ["a__a-1"]},
         "logs": {"a__a-1": {"run_instance.log": "ModuleNotFoundError: x",
                             "test_output.txt": "Error response from daemon"}}}  # fmt: skip
    _, camp, _ = run_fork(tmp_path, monkeypatch, [s])
    assert outcomes(camp)["a__a-1"][1] == "grader_infra"  # the environment tier wins


def test_k129_apply_failure_should_keep_precedence_over_every_signature(tmp_path, monkeypatch):
    text = ">>>>> Patch Apply Failed\nError creating container\nTimeout error: 5 seconds exceeded."
    rc, camp, _ = run_fork(
        tmp_path, monkeypatch, [step("error_ids", "a__a-1", text, "run_instance.log")]
    )
    assert outcomes(camp)["a__a-1"] == ("0", "apply_failed")
    assert rc == 0


def test_k129_resolved_and_empty_patch_should_never_be_classified(tmp_path, monkeypatch):
    ids = ("a__a-1", "b__b-2")
    s = {
        "report": {"resolved_ids": ["a__a-1"], "empty_patch_ids": ["b__b-2"]},
        "logs": {i: {"test_output.txt": INFRA + "\n" + AMB} for i in ids},
    }
    rc, camp, _ = run_fork(tmp_path, monkeypatch, [s], ids)
    assert outcomes(camp) == {"a__a-1": ("1", "resolved"), "b__b-2": ("0", "empty_patch")}
    assert rc == 0


def test_k129_each_instance_should_be_classified_from_its_own_logs(tmp_path, monkeypatch):
    ids = ("a__a-1", "b__b-2", "c__c-3")
    s = {
        "report": {"error_ids": list(ids)},
        "logs": {
            "a__a-1": {"run_instance.log": INFRA},
            "b__b-2": {"test_output.txt": AMB},
            "c__c-3": {"test_output.txt": "AssertionError"},
        },
    }
    _, camp, _ = run_fork(tmp_path, monkeypatch, [s], ids)
    assert outcomes(camp) == {
        "a__a-1": ("", "grader_infra"),
        "b__b-2": ("", "ambiguous"),
        "c__c-3": ("0", "error"),
    }


def test_k129_the_lite_path_should_not_classify_logs(tmp_path, monkeypatch):
    s = {
        "report": {"error_ids": ["a__a-1"], "unresolved_ids": ["b__b-2"]},
        "logs": {
            "a__a-1": {"run_instance.log": INFRA},
            "b__b-2": {"test_output.txt": "Cannot allocate memory"},
        },
    }
    rc, camp, _ = run_fork(tmp_path, monkeypatch, [s], ("a__a-1", "b__b-2"), fork=False)
    assert outcomes(camp) == {"a__a-1": ("0", "error"), "b__b-2": ("0", "unresolved")}
    assert rc == 0


def test_k129_the_lite_path_should_keep_the_report_infra_lists(tmp_path, monkeypatch):
    s = {"report": {"infra_failure_ids": ["a__a-1"], "ambiguous_failure_ids": ["b__b-2"]}}
    _, camp, _ = run_fork(tmp_path, monkeypatch, [s], ("a__a-1", "b__b-2"), fork=False)
    assert outcomes(camp) == {"a__a-1": ("", "grader_infra"), "b__b-2": ("", "ambiguous")}


# ---- K130 the final rule for ambiguous results ----


def amb_step():
    return step("error_ids", "a__a-1", AMB)


def test_k130_a_single_attempt_should_stay_ambiguous(tmp_path, monkeypatch):
    rc, camp, ev = run_fork(tmp_path, monkeypatch, [amb_step()])
    assert outcomes(camp)["a__a-1"] == ("", "ambiguous")
    assert rc == 1 and len(ev.cmds) == 1


def test_k130_ambiguous_on_every_attempt_should_become_ambiguous_repeated(tmp_path, monkeypatch):
    rc, camp, ev = run_fork(
        tmp_path, monkeypatch, [amb_step(), amb_step()], ("a__a-1",), "--regrade"
    )
    assert len(ev.cmds) == 2
    assert outcomes(camp)["a__a-1"] == ("0", "ambiguous_repeated")  # counts as unresolved
    assert rc == 0  # nothing left to re-grade


def test_k130_ambiguous_repeated_should_persist_and_not_be_regraded_again(tmp_path, monkeypatch):
    _, camp, ev = run_fork(
        tmp_path, monkeypatch, [amb_step(), amb_step()], ("a__a-1",), "--regrade"
    )
    before = len(ev.cmds)
    for extra in ((), ("--regrade",)):
        rc = grade.main(grade_args(camp, tmp_path / "prereg", **{}) + list(extra))
        assert rc == 0
        assert outcomes(camp)["a__a-1"] == ("0", "ambiguous_repeated")
    assert len(ev.cmds) == before


def test_k130_a_regrade_that_resolves_should_end_resolved(tmp_path, monkeypatch):
    ok = {"report": {"resolved_ids": ["a__a-1"]}}
    rc, camp, _ = run_fork(tmp_path, monkeypatch, [amb_step(), ok], ("a__a-1",), "--regrade")
    assert outcomes(camp)["a__a-1"] == ("1", "resolved")
    assert rc == 0


def test_k130_a_regrade_without_a_signature_should_end_as_a_plain_failure(tmp_path, monkeypatch):
    plain = step("error_ids", "a__a-1", "AssertionError")
    _, camp, _ = run_fork(tmp_path, monkeypatch, [amb_step(), plain], ("a__a-1",), "--regrade")
    assert outcomes(camp)["a__a-1"] == ("0", "error")


def test_k130_a_first_grade_that_was_infra_should_not_count_towards_repeated(tmp_path, monkeypatch):
    infra = step("error_ids", "a__a-1", INFRA)
    rc, camp, _ = run_fork(tmp_path, monkeypatch, [infra, amb_step()], ("a__a-1",), "--regrade")
    assert outcomes(camp)["a__a-1"] == ("", "ambiguous")
    assert rc == 1


def test_k130_grader_infra_should_never_become_final(tmp_path, monkeypatch):
    infra = step("error_ids", "a__a-1", INFRA)
    rc, camp, ev = run_fork(tmp_path, monkeypatch, [infra], ("a__a-1",), "--regrade")
    assert outcomes(camp)["a__a-1"] == ("", "grader_infra") and rc == 1
    for _ in range(3):  # more re-grades, still infra
        rc = grade.main([*grade_args(camp, tmp_path / "prereg"), "--regrade"])
        assert outcomes(camp)["a__a-1"] == ("", "grader_infra") and rc == 1
    assert len(ev.cmds) >= 5


def test_k130_ambiguous_after_infra_after_ambiguous_should_not_be_repeated(tmp_path, monkeypatch):
    steps = [amb_step(), step("error_ids", "a__a-1", INFRA), amb_step()]
    _, camp, _ = run_fork(tmp_path, monkeypatch, steps, ("a__a-1",), "--regrade")
    again = grade.main([*grade_args(camp, tmp_path / "prereg"), "--regrade"])
    assert outcomes(camp)["a__a-1"][1] != "ambiguous_repeated"
    assert again == 1


def test_k130_the_rule_should_apply_per_instance(tmp_path, monkeypatch):
    ids = ("a__a-1", "b__b-2")
    both = {
        "report": {"error_ids": list(ids)},
        "logs": {i: {"test_output.txt": AMB} for i in ids},
    }
    second = {"report": {"error_ids": ["a__a-1"], "resolved_ids": ["b__b-2"]},
              "logs": {"a__a-1": {"test_output.txt": AMB}}}  # fmt: skip
    _, camp, _ = run_fork(tmp_path, monkeypatch, [both, second], ids, "--regrade")
    assert outcomes(camp) == {"a__a-1": ("0", "ambiguous_repeated"), "b__b-2": ("1", "resolved")}


def test_k130_the_lite_path_should_not_make_ambiguous_final(tmp_path, monkeypatch):
    s = {"report": {"ambiguous_failure_ids": ["a__a-1"]}}
    rc, camp, _ = run_fork(tmp_path, monkeypatch, [s, s], ("a__a-1",), "--regrade", fork=False)
    assert outcomes(camp)["a__a-1"] == ("", "ambiguous")
    assert rc == 1


def test_k130_analysis_plan_should_state_the_rule():
    text = " ".join(str(v) for v in prereg.ANALYSIS_PLAN_A.values()).lower()
    assert "ambiguous_repeated" in text
    assert "grader_infra" in text or "grader infrastructure" in text


# ---- K131 grade with the pre-registered fork ----

TOML_REF = "b" * 40


def k131_rig(tmp_path, *, toml_commit=TOML_REF, toml_ns="tomlns", env=None, toml=True):
    rig = make_rig(tmp_path)
    bindir, log = make_fake_bin(tmp_path)
    d = rig / "prereg" / "camp"
    d.mkdir(parents=True)
    (d / "selection.json").write_text(json.dumps({"grader": FORK}))
    (d / "dataset.json").write_text("[]")
    (d / "images.digests").write_text("")
    if toml:
        (d / "preregistration.toml").write_text(
            'stage = "a"\n\n[grader]\n'
            f'repo = "{FORK}"\ncommit = "{toml_commit}"\nnamespace = "{toml_ns}"\n'
        )
    envf = env_file_without(rig, "HC_A_GRADER_REF", "HC_A_NAMESPACE")
    full = {"FAKE_UV_STUB_MODULES": "hcbench.grade", "HC_ENV_FILE": str(envf), **(env or {})}
    r = run_script(rig, "20_grade.sh", ["camp"], bindir, full)
    return r, [c for c in calls(log) if c[0] == "uv"], d


def test_k131_unset_environment_should_use_the_toml_values(tmp_path):
    r, (argv,), _ = k131_rig(tmp_path)
    assert r.returncode == 0, r.stderr
    text = " ".join(argv)
    assert f"git+https://github.com/SWE-rebench/SWE-bench-fork.git@{TOML_REF}" in text
    assert argv[argv.index("--grader-commit") + 1] == TOML_REF
    assert argv[argv.index("--namespace") + 1] == "tomlns"


def test_k131_environment_values_equal_to_the_toml_should_pass(tmp_path):
    r, uv, _ = k131_rig(tmp_path, env={"HC_A_GRADER_REF": TOML_REF, "HC_A_NAMESPACE": "tomlns"})
    assert r.returncode == 0, r.stderr
    assert len(uv) == 1


def test_k131_a_differing_commit_should_stop_naming_both_values(tmp_path):
    other = "c" * 40
    r, uv, _ = k131_rig(tmp_path, env={"HC_A_GRADER_REF": other})
    assert r.returncode != 0
    out = r.stdout + r.stderr
    assert other in out and TOML_REF in out
    assert uv == []


def test_k131_a_differing_namespace_should_stop_naming_both_values(tmp_path):
    r, uv, _ = k131_rig(tmp_path, env={"HC_A_NAMESPACE": "otherns"})
    assert r.returncode != 0
    out = r.stdout + r.stderr
    assert "otherns" in out and "tomlns" in out
    assert uv == []


@pytest.mark.parametrize("bad", ["", "abc", TOML_REF.upper(), TOML_REF + "0", "main"])
def test_k131_a_toml_commit_that_is_not_40_lowercase_hex_should_be_refused(tmp_path, bad):
    r, uv, _ = k131_rig(tmp_path, toml_commit=bad)
    assert r.returncode != 0
    assert uv == []


# ---- K132 record what graded ----


def test_k132_grader_json_should_record_the_package_versions(tmp_path, monkeypatch):
    versions = {"swebench": "4.0.3", "docker": "7.1.0"}
    monkeypatch.setattr(grade, "package_version", lambda name: versions[name])
    camp = make_campaign(tmp_path, IDS3)
    d, _ = fork_inputs(tmp_path, IDS3)
    monkeypatch.setattr(grade.subprocess, "run", ForkEvaluate({"report": {"resolved_ids": IDS3}}))
    grade.main(grade_args(camp, d))
    rec = json.loads((camp / "grading" / "grader.json").read_text())
    assert rec["swebench_version"] == "4.0.3" and rec["docker_version"] == "7.1.0"
    assert rec["repo"] == FORK and rec["commit"] == REF and rec["namespace"] == NS
    assert "dataset_sha256" in rec


def test_k132_package_version_should_read_importlib_metadata(monkeypatch):
    monkeypatch.setattr(importlib.metadata, "version", lambda name: f"v-{name}")
    assert grade.package_version("swebench") == "v-swebench"
    assert grade.package_version("docker") == "v-docker"


# ---- K133 gaps the review found ----


@pytest.mark.parametrize(
    ("key", "value"),
    [
        ("HC_A_DATASET", "nebius/SWE-rebench-leaderboard "),
        ("HC_A_MIN_SPLIT", " 2026_03"),
        ("HC_A_MIN_CREATED", "2026-03-01 "),
        ("HC_SELECTION_SEED", " 20261109 "),
        ("HC_A_N", " 60 "),
        ("HC_A_GRADER", "SWE-rebench/SWE-bench-fork "),
    ],
)
def test_k133_preflight_should_refuse_a_padded_rule_value(stage_a_env, key, value):
    stage_a_env["env"]["mp"].setenv(key, value)
    with pytest.raises(SystemExit) as exc:
        preflight(stage_a_env)
    assert key in str(exc.value)


def test_k133_the_fork_argv_should_have_no_split(tmp_path, monkeypatch):
    camp = make_campaign(tmp_path, IDS3)
    d, _ = fork_inputs(tmp_path, IDS3)
    ev = ForkEvaluate({"report": {"resolved_ids": IDS3}})
    monkeypatch.setattr(grade.subprocess, "run", ev)
    grade.main(grade_args(camp, d))
    (cmd,) = ev.cmds
    assert "--split" not in cmd


@pytest.mark.parametrize("content", ["[]", '"text"', "3", "null", "[1, 2]"])
def test_k133_a_manifest_that_is_not_an_object_should_be_refused_in_stage_cb(env, content):
    pin(env)
    for k in ("HC_ARMS_MAIN", "HC_ARMS_REDTEAM", "HC_ARMS_B"):
        env["mp"].setenv(k, "pi")
    (env["tmp"] / "bundle" / "manifest.json").write_text(content)
    args = ["--stage", "cb", "--out", str(env["tmp"] / "p.toml"), "--env-file", str(EXAMPLE)]
    with pytest.raises(SystemExit) as exc:
        prereg.main(args)
    assert "manifest.json" in str(exc.value)


@pytest.mark.parametrize("content", ["[]", '"text"', "3", "null"])
def test_k133_a_manifest_that_is_not_an_object_should_be_refused_in_stage_a(stage_a_env, content):
    (stage_a_env["env"]["tmp"] / "bundle" / "manifest.json").write_text(content)
    with pytest.raises(SystemExit) as exc:
        run_a(stage_a_env)
    assert "manifest.json" in str(exc.value)
