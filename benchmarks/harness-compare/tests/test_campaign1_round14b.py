"""Spec section 11.17, round 14b, K114-K123: what the review of round 14 found.

Contract choices made here (the spec leaves them open):
* K117: ``python -m hcbench.prereg --stage a --rules <file> --preflight [--accept-diff KEY]...``;
  neither ``--selection`` nor ``--out`` is needed, nothing is written. The script runs it
  before it creates the directory, selects or pulls.
* K114: ``HC_A_N=40`` with ``n = 40`` in the selection is accepted without ``--accept-diff``
  when the stage-cb ``cut_order`` holds exactly "suite A tasks 60 -> 40"; the stage-a file
  then has ``[rules_check] cut``. It is not recorded when n is the recorded 60.
* K120: ``--accept-diff files_sha256.<key>`` (the key as the file spells it).
* K121: the versions come from ``importlib.metadata.version("datasets")`` and
  ``importlib.metadata.version("huggingface_hub")``.
"""
# ruff: noqa: F811, F401  (the fixtures `hub` and `stage_a_env` are imported by name)

import hashlib
import importlib.metadata
import json
import re
import sys
import tomllib
import types

import pytest
from fakebin import calls, docker_calls, run_script
from test_campaign1_prereg import EXAMPLE, ROOT, pin, run_cb
from test_campaign1_round14 import (
    A_ARGS,
    DATASET,
    IDS,
    SHA,
    STAGE_IDS,
    Docker,
    check,
    do_select,
    find,
    good,
    hub,
    image_of,
    make_sel_dir,
    pinned,
    pull_index,
    rebench,
    rewrite_selection,
    run_a,
    stage_a_data,
    stage_a_env,
    stage_a_rig,
    write_selection,
)

from hcbench import prereg, sampling

CUT = "suite A tasks 60 -> 40"


def edit_cb(s, old, new, name="edited.toml"):
    """A copy of the stage-cb file with `old` replaced; returns its path."""
    text = s["cb"].read_text()
    assert old in text, old
    path = s["env"]["tmp"] / name
    path.write_text(text.replace(old, new, 1))
    return path


# ---- K114 the pre-registered cut ---------------------------------------------------------


def test_k114_n40_with_the_recorded_cut_should_be_accepted_and_recorded(stage_a_env):
    assert CUT in prereg.CUT_ORDER
    stage_a_env["env"]["mp"].setenv("HC_A_N", "40")
    rewrite_selection(stage_a_env, n=40)
    data, _ = stage_a_data(stage_a_env)
    assert data["rules_check"]["cut"] == CUT
    assert data["tasks"]["n"] == 40


def test_k114_the_recorded_n_should_not_record_a_cut(stage_a_env):
    data, _ = stage_a_data(stage_a_env)
    assert "cut" not in data["rules_check"]
    assert data["tasks"]["n"] == 60


@pytest.mark.parametrize("n", [50, 41, 39, 61, 1])
def test_k114_any_other_n_should_be_refused_even_with_accept_diff(stage_a_env, n):
    stage_a_env["env"]["mp"].setenv("HC_A_N", str(n))
    rewrite_selection(stage_a_env, n=n)
    with pytest.raises(SystemExit):
        run_a(stage_a_env)
    with pytest.raises(SystemExit):
        run_a(stage_a_env, "--accept-diff", "n", "--accept-diff", "HC_A_N", "--accept-diff", "cut")
    assert not (stage_a_env["env"]["tmp"] / "a.toml").exists()


@pytest.mark.parametrize(
    "replacement",
    [
        "suite A tasks 60 -> 45",  # another cut
        "suite A tasks 60 -> 40 ",  # not exactly the entry
        "Suite A tasks 60 -> 40",
        "suite A tasks 60 -> 4",
    ],
)
def test_k114_n40_without_the_exact_entry_should_be_refused_even_with_accept_diff(
    stage_a_env, replacement
):
    cb = edit_cb(stage_a_env, f'"{CUT}"', f'"{replacement}"')
    stage_a_env["env"]["mp"].setenv("HC_A_N", "40")
    rewrite_selection(stage_a_env, n=40)
    with pytest.raises(SystemExit):
        run_a(stage_a_env, cb=cb)
    with pytest.raises(SystemExit):
        run_a(stage_a_env, "--accept-diff", "n", "--accept-diff", "HC_A_N", cb=cb)


def test_k114_n40_with_the_entry_removed_should_be_refused(stage_a_env):
    cb = edit_cb(stage_a_env, f', "{CUT}"', "")
    stage_a_env["env"]["mp"].setenv("HC_A_N", "40")
    rewrite_selection(stage_a_env, n=40)
    with pytest.raises(SystemExit):
        run_a(stage_a_env, cb=cb)


# ---- K115 Docker's own errors -------------------------------------------------------------


@pytest.mark.parametrize("code", [126, 127, 1, 2])
def test_k115_other_failing_exit_codes_should_still_exclude_the_task(tmp_path, monkeypatch, code):
    heads = make_sel_dir(tmp_path, IDS[:2])
    docker = Docker({pinned("a__a-1"): good(heads["a__a-1"]), pinned("b__b-2"): (code, "")})
    assert check(tmp_path, monkeypatch, docker) == 0
    excluded = json.loads((tmp_path / "selection.json").read_text())["excluded"]
    assert list(excluded) == ["b__b-2"]
    assert str(code) in excluded["b__b-2"]  # K123: the reason names the exit code


def test_k115_exit_125_on_a_later_image_should_still_write_nothing(tmp_path, monkeypatch):
    heads = make_sel_dir(tmp_path, IDS)
    before = (tmp_path / "selection.json").read_bytes()
    results = {pinned(i): good(heads[i]) for i in IDS}
    results[pinned("d__d-4")] = (125, "")
    with pytest.raises(SystemExit) as exc:
        check(tmp_path, monkeypatch, Docker(results))
    assert "d__d-4" in str(exc.value)
    assert (tmp_path / "selection.json").read_bytes() == before
    assert (tmp_path / "tasks.txt").read_text() == "".join(f"{i}\n" for i in sorted(IDS))
    assert not (tmp_path / "images.check.json").exists()


# ---- K116 no stage-a file without a finished image check ---------------------------------


def corrupt(s, how):
    d = s["dir"]
    sel = json.loads((d / "selection.json").read_text())
    if how == "no_check_file":
        (d / "images.check.json").unlink()
    elif how == "no_run_digest":
        del sel["run_digest"]
    elif how == "wrong_run_digest":
        sel["run_digest"] = "0" * 64
    elif how == "excluded_task_still_listed":
        sel["excluded"] = {STAGE_IDS[0]: "head differs"}
    elif how == "task_dropped_but_not_excluded":
        run = [i for i in sorted(STAGE_IDS) if i != STAGE_IDS[0]]
        (d / "tasks.txt").write_text("".join(f"{i}\n" for i in run))
        sel["run_digest"] = sampling.manifest_digest(run)
    elif how == "excluded_not_in_drawn_list":
        sel["excluded"] = {"x__y-9": "head differs"}
    elif how == "digest_changed":
        sel["digest"] = "1" * 64
    (d / "selection.json").write_text(json.dumps(sel))


@pytest.mark.parametrize(
    ("how", "word"),
    [
        ("no_check_file", "images.check.json"),
        ("no_run_digest", "run_digest"),
        ("wrong_run_digest", "run_digest"),
        ("excluded_task_still_listed", "excluded"),
        ("task_dropped_but_not_excluded", "digest"),
        ("excluded_not_in_drawn_list", "digest"),
        ("digest_changed", "digest"),
    ],
)
def test_k116_an_unfinished_image_check_should_be_refused(stage_a_env, how, word):
    corrupt(stage_a_env, how)
    with pytest.raises(SystemExit) as exc:
        run_a(stage_a_env)
    assert word in str(exc.value)
    assert not (stage_a_env["env"]["tmp"] / "a.toml").exists()


def test_k116_a_finished_check_with_exclusions_should_be_accepted(stage_a_env):
    s = stage_a_env
    drawn = [*STAGE_IDS, "x__y-9"]
    write_selection(
        s["dir"], s["rules"], run=STAGE_IDS, drawn=drawn, excluded={"x__y-9": "head differs"}
    )
    assert run_a(s)[0] == 0


# ---- K117 preflight ------------------------------------------------------------------------


def preflight(s, *extra, cb=None):
    return prereg.main(
        ["--stage", "a", "--rules", str(cb or s["cb"]), "--preflight", *extra,
         "--env-file", str(EXAMPLE)]
    )  # fmt: skip


def tree(path):
    return sorted(str(p.relative_to(path)) for p in path.rglob("*"))


def test_k117_preflight_should_pass_and_write_nothing(stage_a_env):
    tmp = stage_a_env["env"]["tmp"]
    before = tree(tmp)
    assert preflight(stage_a_env) == 0
    assert tree(tmp) == before


def test_k117_preflight_should_not_need_a_selection_or_an_out_file(stage_a_env, capsys):
    out = stage_a_env["env"]["tmp"] / "never.toml"
    code = prereg.main(
        [
            "--stage",
            "a",
            "--rules",
            str(stage_a_env["cb"]),
            "--preflight",
            "--out",
            str(out),
            "--env-file",
            str(EXAMPLE),
        ]
    )
    assert code == 0
    assert not out.exists()


def test_k117_preflight_should_refuse_a_missing_rules_file(stage_a_env):
    with pytest.raises(SystemExit) as exc:
        preflight(stage_a_env, cb=stage_a_env["env"]["tmp"] / "nope.toml")
    assert "rules" in str(exc.value).lower()


def test_k117_preflight_should_refuse_a_file_that_is_not_stage_cb(stage_a_env):
    other = edit_cb(stage_a_env, 'stage = "cb"', 'stage = "a"')
    with pytest.raises(SystemExit):
        preflight(stage_a_env, cb=other)


def test_k117_preflight_should_refuse_a_draft_rules_file(stage_a_env):
    _, draft = run_cb(stage_a_env["env"], "--draft")
    with pytest.raises(SystemExit) as exc:
        preflight(stage_a_env, cb=draft)
    assert "draft" in str(exc.value).lower()


@pytest.mark.parametrize(
    ("key", "bad"),
    [
        ("HC_A_DATASET", "other/dataset"),
        ("HC_A_MIN_SPLIT", "2026_04"),
        ("HC_A_MIN_CREATED", "2026-04-01"),
        ("HC_A_GRADER", "someone/else"),
        ("HC_SELECTION_SEED", "1"),
        ("HC_A_N", "50"),
    ],
)
def test_k117_preflight_should_refuse_an_environment_that_differs_from_the_rules(
    stage_a_env, key, bad
):
    stage_a_env["env"]["mp"].setenv(key, bad)
    with pytest.raises(SystemExit) as exc:
        preflight(stage_a_env)
    assert key in str(exc.value)
    with pytest.raises(SystemExit):  # nothing overrides a rules difference
        preflight(stage_a_env, "--accept-diff", key)


def test_k117_preflight_should_accept_the_recorded_cut(stage_a_env):
    stage_a_env["env"]["mp"].setenv("HC_A_N", "40")
    assert preflight(stage_a_env) == 0


def test_k117_preflight_should_check_config_and_honour_accept_diff(stage_a_env):
    tmp = stage_a_env["env"]["tmp"]
    stage_a_env["env"]["mp"].setenv("HC_TOP_K", "21")
    with pytest.raises(SystemExit) as exc:
        preflight(stage_a_env)
    assert "HC_TOP_K" in str(exc.value)
    before = tree(tmp)
    assert preflight(stage_a_env, "--accept-diff", "HC_TOP_K") == 0
    assert tree(tmp) == before


def test_k117_preflight_should_check_the_bundle_and_honour_accept_diff(stage_a_env):
    env = stage_a_env["env"]
    (env["tmp"] / "bundle" / "manifest.json").write_text(json.dumps({**env["manifest"], "x": 2}))
    with pytest.raises(SystemExit) as exc:
        preflight(stage_a_env)
    assert "bundle" in str(exc.value)
    assert preflight(stage_a_env, "--accept-diff", "bundle") == 0


def tamper(path, key, new="0" * 64):
    text = path.read_text()
    pat = re.compile(rf'(?m)^("?{re.escape(key)}"?) = "[0-9a-f]{{64}}"$')
    assert pat.search(text), key
    path.write_text(pat.sub(lambda m: f'{m.group(1)} = "{new}"', text, count=1))


def test_k117_preflight_should_check_the_k120_files(stage_a_env):
    tamper(stage_a_env["cb"], "arms_toml")
    with pytest.raises(SystemExit) as exc:
        preflight(stage_a_env)
    assert "files_sha256.arms_toml" in str(exc.value)
    assert preflight(stage_a_env, "--accept-diff", "files_sha256.arms_toml") == 0


def preflight_script(tmp_path, *, stub, drop_rules=False, args=None, extra_env=None):
    rig, bindir, log, env, d = stage_a_rig(
        tmp_path, preplace=False, extra_env={"FAKE_UV_STUB_MODULES": stub, **(extra_env or {})}
    )
    if drop_rules:
        (rig / "prereg" / "c1" / "preregistration.toml").unlink()
    r = run_script(rig, "05_preregister.sh", args or [*A_ARGS, "--draft"], bindir, env)
    return rig, log, d, r


def test_k117_script_should_run_the_preflight_before_selecting_or_pulling(tmp_path):
    rig, bindir, log, env, _ = stage_a_rig(tmp_path)
    r = run_script(rig, "05_preregister.sh", [*A_ARGS, "--accept-diff", "bundle"], bindir, env)
    assert r.returncode == 0, r.stderr
    i_pf = find(log, "hcbench.prereg", "--preflight")
    i_sel = find(log, "hcbench.rebench", "select")
    assert i_pf is not None and i_sel is not None and i_pf < i_sel < pull_index(log)
    c = calls(log)[i_pf]
    assert c[c.index("--rules") + 1] == "prereg/c1/preregistration.toml"
    assert c[c.index("--stage") + 1] == "a"
    assert c[c.index("--accept-diff") + 1] == "bundle"
    assert "--selection" not in c


def test_k117_script_with_a_bad_rules_file_should_stop_before_any_work(tmp_path):
    rig, log, d, r = preflight_script(tmp_path, stub="hcbench.rebench", drop_rules=True)
    assert r.returncode != 0
    assert "rules" in (r.stdout + r.stderr).lower()
    assert find(log, "hcbench.prereg", "--preflight") is not None
    assert find(log, "hcbench.rebench") is None
    assert docker_calls(log) == []
    assert not d.exists()
    assert not (rig / "prereg" / "t").exists()


# ---- K118 stage cb unchanged -------------------------------------------------------------


def test_k118_stage_cb_tables_should_keep_the_order_of_the_tagged_code(env):
    pin(env)
    _, out = run_cb(env)
    headers = re.findall(r"(?m)^\[([^\]]+)\]$", out.read_text())
    assert headers == [
        "repo",
        "config",
        "sampling",
        "sampling.chat_template_kwargs",
        "suite_a_rules",
        "analysis",
        "files_sha256",
        "bundle",
    ]
    assert tomllib.loads(out.read_text())["stage"] == "cb"


def test_k118_stage_cb_scalar_keys_should_come_first_in_the_same_order(env):
    pin(env)
    _, out = run_cb(env)
    head = out.read_text().split("\n[repo]")[0]
    keys = re.findall(r"(?m)^([a-z_]+) = ", head)
    assert keys == ["title", "stage", "created_at", "draft", "suites_in_scope",
                    "suites_deferred", "cut_order"]  # fmt: skip


# ---- K119 every check result kept ---------------------------------------------------------


def test_k119_a_rerun_should_keep_the_earlier_exclusions_and_their_results(tmp_path, monkeypatch):
    heads = make_sel_dir(tmp_path, IDS)
    first = Docker({pinned(i): good(heads[i]) for i in IDS} | {pinned("b__b-2"): good("f" * 40)})
    assert check(tmp_path, monkeypatch, first) == 0
    first_result = json.loads((tmp_path / "images.check.json").read_text())["b__b-2"]
    assert first_result["ok"] is False
    # Second run: tasks.txt no longer lists b__b-2, so only the others are run again.
    second = Docker({pinned(i): good(heads[i]) for i in IDS if i != "b__b-2"})
    assert check(tmp_path, monkeypatch, second) == 0
    assert len(second.argvs) == 3
    res = json.loads((tmp_path / "images.check.json").read_text())
    assert set(res) == set(IDS)
    assert res["b__b-2"] == first_result
    sel = json.loads((tmp_path / "selection.json").read_text())
    assert list(sel["excluded"]) == ["b__b-2"]
    assert sel["digest"] == sampling.manifest_digest(IDS)
    assert sel["run_digest"] == sampling.manifest_digest([i for i in IDS if i != "b__b-2"])


def test_k119_a_rerun_should_replace_the_result_of_a_task_it_checked_again(tmp_path, monkeypatch):
    heads = make_sel_dir(tmp_path, IDS[:2])
    check(tmp_path, monkeypatch, Docker({pinned(i): good(heads[i]) for i in IDS[:2]}))
    second = Docker({pinned("a__a-1"): good("e" * 40), pinned("b__b-2"): good(heads["b__b-2"])})
    check(tmp_path, monkeypatch, second)
    res = json.loads((tmp_path / "images.check.json").read_text())
    assert res["a__a-1"]["ok"] is False and res["b__b-2"]["ok"] is True
    assert "a__a-1" in json.loads((tmp_path / "selection.json").read_text())["excluded"]


def test_k123_every_task_excluded_should_stop_the_check(tmp_path, monkeypatch):
    make_sel_dir(tmp_path, IDS[:2])
    bad = Docker({pinned(i): good("f" * 40) for i in IDS[:2]})
    with pytest.raises(SystemExit) as exc:
        check(tmp_path, monkeypatch, bad)
    assert "empty" in str(exc.value).lower() or "every" in str(exc.value).lower()


# ---- K120 the same arms and images as stage cb --------------------------------------------


@pytest.mark.parametrize("key", ["arms_toml", "docker/mini.Dockerfile", "suite_b"])
def test_k120_a_files_hash_that_differs_should_be_refused_and_named(stage_a_env, key):
    tamper(stage_a_env["cb"], key)
    with pytest.raises(SystemExit) as exc:
        run_a(stage_a_env)
    assert f"files_sha256.{key}" in str(exc.value)


def test_k120_accepting_the_named_key_should_record_both_hashes(stage_a_env):
    key = "arms_toml"
    real = hashlib.sha256((ROOT / "config" / "arms.toml").read_bytes()).hexdigest()
    tamper(stage_a_env["cb"], key, "0" * 64)
    data, _ = stage_a_data(stage_a_env, "--accept-diff", f"files_sha256.{key}")
    accepted = json.dumps(data["rules_check"]["accepted"])
    assert f"files_sha256.{key}" in accepted and "0" * 64 in accepted and real in accepted
    assert data["files_sha256"][key] == real


def test_k120_accepting_another_key_should_not_let_the_difference_through(stage_a_env):
    tamper(stage_a_env["cb"], "arms_toml")
    with pytest.raises(SystemExit) as exc:
        run_a(stage_a_env, "--accept-diff", "files_sha256.suite_b", "--accept-diff", "arms_toml")
    assert "files_sha256.arms_toml" in str(exc.value)


def test_k120_a_key_only_one_file_records_should_not_be_a_difference(stage_a_env):
    cb = edit_cb(stage_a_env, "[files_sha256]\n", '[files_sha256]\nzzz_only_in_cb = "ab"\n')
    data, _ = stage_a_data_cb(stage_a_env, cb)
    assert not data["rules_check"]["accepted"]


def stage_a_data_cb(s, cb):
    rc, out = run_a(s, cb=cb)
    assert rc == 0
    return tomllib.loads(out.read_text()), out


# ---- K121 the selection's environment -----------------------------------------------------


def fake_hub_api(monkeypatch, exc=None, sha=SHA):
    seen = []

    class HfApi:
        def dataset_info(self, dataset, revision=None):
            seen.append((dataset, revision))
            if exc is not None:
                raise exc
            return types.SimpleNamespace(sha=sha)

    module = types.ModuleType("huggingface_hub")
    module.HfApi = HfApi
    monkeypatch.setitem(sys.modules, "huggingface_hub", module)
    return seen


@pytest.mark.parametrize(
    "exc", [RuntimeError("boom"), ValueError("v"), OSError("o"), KeyError("k"), Exception("e")]
)
def test_k121_resolve_revision_should_return_none_on_any_exception(monkeypatch, exc):
    fake_hub_api(monkeypatch, exc=exc)
    assert rebench().resolve_revision(DATASET, "main") is None


def test_k121_resolve_revision_should_return_the_sha_of_the_revision(monkeypatch):
    seen = fake_hub_api(monkeypatch)
    assert rebench().resolve_revision(DATASET, "abc") == SHA
    assert seen == [(DATASET, "abc")]


def test_k121_selection_should_record_the_library_versions(hub, tmp_path, monkeypatch):
    real = importlib.metadata.version
    versions = {"datasets": "9.9.9", "huggingface_hub": "8.8.8"}
    monkeypatch.setattr(
        importlib.metadata, "version", lambda name: versions.get(name) or real(name)
    )
    do_select(tmp_path / "s", n=4)
    sel = json.loads((tmp_path / "s" / "selection.json").read_text())
    assert sel["datasets_version"] == "9.9.9"
    assert sel["huggingface_hub_version"] == "8.8.8"


def test_k121_splits_should_list_every_eligible_name_even_without_a_task(hub, tmp_path):
    hub.splits["2026_04"] = []  # an eligible name with no row
    hub.splits["2026_05"] = [
        {**hub.splits["2026_03"][0], "instance_id": "late__l-1", "created_at": "2026-01-01"}
    ]  # a row before min_created: contributes nothing
    do_select(tmp_path / "s", n=4)
    sel = json.loads((tmp_path / "s" / "selection.json").read_text())
    assert sel["splits"] == ["2026_02", "2026_03", "2026_04", "2026_05"]  # not test, not 2025_12


# ---- K122 small fixes ---------------------------------------------------------------------


def test_k122_a_cb_config_key_missing_from_the_environment_should_compare_as_empty(stage_a_env):
    cb = edit_cb(stage_a_env, "[config]\n", '[config]\nHC_OLD_EMPTY = ""\nHC_OLD_SET = "x"\n')
    with pytest.raises(SystemExit) as exc:  # "x" is not ""
        run_a(stage_a_env, cb=cb)
    assert "HC_OLD_SET" in str(exc.value)
    assert "HC_OLD_EMPTY" not in str(exc.value)  # "" equals the missing value
    data, _ = stage_a_data_cb_args(stage_a_env, cb, "--accept-diff", "HC_OLD_SET")
    accepted = json.dumps(data["rules_check"]["accepted"])
    assert "HC_OLD_SET" in accepted and "None" not in accepted


def stage_a_data_cb_args(s, cb, *extra):
    rc, out = run_a(s, *extra, cb=cb)
    assert rc == 0
    return tomllib.loads(out.read_text()), out


def test_k122_dataset_json_rows_should_carry_no_split_field(hub, tmp_path):
    do_select(tmp_path / "s", n=100)
    rows = json.loads((tmp_path / "s" / "dataset.json").read_text())
    assert rows and all("split" not in r for r in rows)


def test_k122_readme_should_show_the_direct_script_call_with_accept_diff():
    text = (ROOT / "README.md").read_text()
    assert re.search(r"05_preregister\.sh c1-a --stage a --rules \S+ --accept-diff", text)


# ---- K123 surviving mutants ----------------------------------------------------------------


@pytest.mark.parametrize(
    "line",
    [
        "{img} MISSING",
        "{img} {img}",  # a tag, not a digest
        "{img} {ref} trailing",  # trailing text: a full match, not a prefix match
        "{img} {ref}deadbeef",  # more than 64 hex characters
        "{img} {upper}",
        "{img} {ref}\textra",
    ],
)
def test_k123_check_images_should_refuse_a_missing_tagged_or_malformed_line(
    tmp_path, monkeypatch, line
):
    heads = make_sel_dir(tmp_path, IDS[:2])
    img, ref = image_of("a__a-1"), pinned("a__a-1")
    bad = line.format(img=img, ref=ref, upper=ref.upper().replace("@SHA256", "@sha256"))
    (tmp_path / "images.digests").write_text(f"{bad}\n{image_of('b__b-2')} {pinned('b__b-2')}\n")
    before = (tmp_path / "tasks.txt").read_bytes()
    docker = Docker({pinned(i): good(heads[i]) for i in IDS[:2]})
    with pytest.raises(SystemExit) as exc:
        check(tmp_path, monkeypatch, docker)
    assert "a__a-1" in str(exc.value) or img in str(exc.value)
    assert docker.argvs == []  # refused before any container starts
    assert (tmp_path / "tasks.txt").read_bytes() == before


@pytest.mark.parametrize("suffix", [" trailing", "deadbeef", "\textra"])
def test_k123_stage_a_should_refuse_a_digest_line_with_trailing_text(stage_a_env, suffix):
    s = stage_a_env
    img = image_of(STAGE_IDS[0])
    lines = [f"{img} {pinned(STAGE_IDS[0])}{suffix}"] + [
        f"{image_of(i)} {pinned(i)}" for i in STAGE_IDS[1:]
    ]
    write_selection(s["dir"], s["rules"], digest_lines=lines)
    with pytest.raises(SystemExit) as exc:
        run_a(s)
    assert img in str(exc.value)
