"""Spec section 11.10, K56-K58 (pre-tag follow-ups).

K56 replaces the K54 path list: every path of the rig is code except README.md and SPEC.md at
the rig root and anything under tests/ (prereg/ is excluded upstream). K57: every FROM in
docker/*.Dockerfile is pinned by digest; a non-draft hcbench.prereg refuses an unpinned one and
names file and line (the Dockerfiles are found under the rig directory, as the pre-registered
files are). K58: the vLLM / llama.cpp server image must carry @sha256:<64 lowercase hex>.
"""

import re
import subprocess
import tomllib

import pytest
from fakebin import (
    LLAMA_PINNED,
    ROOT,
    SHA40,
    VLLM_PINNED,
    harmful,
    make_fake_bin,
    make_rig,
    run_script,
)
from test_campaign1_prereg import pin, run_cb
from test_campaign1_report import section
from test_campaign1_round3 import CLEAN
from test_campaign1_round4 import git, git_rig, prereg_cli
from test_campaign1_round5 import (
    code_campaign,
    code_rig,
    code_rows,
    commit_change,
    inv,
    run_cli,
)

from hcbench import report

# ---- K56 everything is code except README.md, SPEC.md and tests/ (pure) -------------------

CODE_NOW = [
    ".gitignore",
    "json.py",
    "setup.cfg",
    "newdir/x.py",
    "newdir/deep/y.sh",
    "hcbench/README.md",
    "hcbench/SPEC.md",
    "docs/a.md",
    "Makefile.bak",
    "hcbench_notes.md",
    "README.md.orig",
    "tests_helper.py",
]
INFO_ONLY = ["README.md", "SPEC.md", "tests/x.py", "tests/sub/deep/y.py"]


def evidence(changed=(), dirty=()):
    return {
        "preregistration": {"head": "h1", "resolves": True},
        "changed": {"h1": [], "h2": list(changed)},
        "report": {"head": "h1", "dirty_paths": list(dirty)},
    }


def check(invocations, code):
    return report.check_conformance(
        [],
        {**CLEAN, "repo": {"head": "h1", "dirty": False}},
        invocations=invocations,
        code=code,
    )


@pytest.mark.parametrize("path", CODE_NOW)
def test_k56_a_changed_path_outside_the_docs_and_tests_is_code(path):
    res = check([inv(head="h2")], evidence(changed=[path]))
    (m,) = res["mismatches"]
    assert (m["invocation"], m["field"], m["found"]) == (0, "code.changed", [path])
    assert res["other"] == []


@pytest.mark.parametrize("path", CODE_NOW)
def test_k56_a_dirty_path_outside_the_docs_and_tests_is_code(path):
    res = check([inv(dirty_paths=[path])], evidence())
    (m,) = res["mismatches"]
    assert (m["invocation"], m["field"], m["found"]) == (0, "code.dirty", [path])


@pytest.mark.parametrize("path", CODE_NOW)
def test_k56_a_dirty_path_at_report_time_is_code(path):
    (m,) = check([inv()], evidence(dirty=[path]))["mismatches"]
    assert (m["invocation"], m["field"], m["found"]) == ("report", "code.dirty", [path])


@pytest.mark.parametrize("path", INFO_ONLY)
def test_k56_readme_spec_and_tests_are_information_only(path):
    res = check([inv(head="h2", dirty_paths=[path])], evidence(changed=[path], dirty=[path]))
    assert res["mismatches"] == []
    assert {"invocation": 0, "kind": "changed", "paths": [path]} in res["other"]
    assert {"invocation": 0, "kind": "dirty", "paths": [path]} in res["other"]
    assert {"invocation": "report", "kind": "dirty", "paths": [path]} in res["other"]


def test_k56_a_readme_or_spec_below_the_rig_root_is_code():
    res = check([inv(head="h2")], evidence(changed=["hcbench/README.md", "docker/SPEC.md"]))
    (m,) = res["mismatches"]
    assert m["found"] == ["docker/SPEC.md", "hcbench/README.md"]


def test_k56_mixed_paths_split_into_mismatch_and_information():
    res = check(
        [inv(head="h2")],
        evidence(changed=["README.md", ".gitignore", "tests/a.py", "json.py", "SPEC.md"]),
    )
    (m,) = res["mismatches"]
    assert m["found"] == [".gitignore", "json.py"]
    assert res["other"] == [
        {"invocation": 0, "kind": "changed", "paths": ["README.md", "SPEC.md", "tests/a.py"]}
    ]


# ---- K56 with real repositories -----------------------------------------------------------


def commit_new(rig, rel, text="x = 1\n"):
    path = rig / rel
    path.parent.mkdir(parents=True, exist_ok=True)
    path.write_text(text)
    git(rig, "add", "-A")
    git(rig, "commit", "-q", "-m", f"add {rel}")
    return git(rig, "rev-parse", "HEAD").stdout.strip()


@pytest.mark.parametrize("rel", [".gitignore", "hcbench/README.md"])
def test_k56_a_committed_change_to_a_dotfile_or_a_nested_readme_exits_3(tmp_path, rel):
    rig, h1 = code_rig(tmp_path)
    if rel == "hcbench/README.md":
        (rig / rel).write_text("notes\n")
        git(rig, "add", "-A")
        git(rig, "commit", "-q", "-m", "nested readme")
        h1 = git(rig, "rev-parse", "HEAD").stdout.strip()
    h2 = commit_change(rig, rel, "\n# changed\n")
    camp, pre = code_campaign(rig, h1, [{"head": h2}])
    r, md = run_cli(rig, camp, pre)
    assert r.returncode == 3
    assert any(rel in ln for ln in code_rows(md, "code.changed"))


@pytest.mark.parametrize("rel", ["json.py", "newdir/tool.py"])
def test_k56_a_new_root_file_or_directory_committed_after_the_head_exits_3(tmp_path, rel):
    rig, h1 = code_rig(tmp_path)
    h2 = commit_new(rig, rel)
    camp, pre = code_campaign(rig, h1, [{"head": h2}])
    r, md = run_cli(rig, camp, pre)
    assert r.returncode == 3
    assert any(rel in ln for ln in code_rows(md, "code.changed"))


@pytest.mark.parametrize("rel", ["json.py", "newdir/tool.py"])
def test_k56_an_untracked_root_file_or_directory_at_report_time_exits_3(tmp_path, rel):
    rig, h1 = code_rig(tmp_path)
    path = rig / rel
    path.parent.mkdir(parents=True, exist_ok=True)
    path.write_text("x = 1\n")
    camp, pre = code_campaign(rig, h1, [{"head": h1}])
    r, md = run_cli(rig, camp, pre)
    assert r.returncode == 3
    assert any(rel in ln for ln in code_rows(md, "code.dirty"))


def test_k56_an_uncommitted_gitignore_change_exits_3(tmp_path):
    rig, h1 = code_rig(tmp_path)
    (rig / ".gitignore").write_text((rig / ".gitignore").read_text() + "\nextra/\n")
    camp, pre = code_campaign(rig, h1, [{"head": h1}])
    r, md = run_cli(rig, camp, pre)
    assert r.returncode == 3
    assert any(".gitignore" in ln for ln in code_rows(md, "code.dirty"))


@pytest.mark.parametrize("rel", ["README.md", "SPEC.md"])
def test_k56_a_change_to_the_root_readme_or_spec_is_information_and_exits_0(tmp_path, rel):
    rig, h1 = code_rig(tmp_path)
    h2 = commit_change(rig, rel)
    camp, pre = code_campaign(rig, h1, [{"head": h2}])
    r, md = run_cli(rig, camp, pre)
    assert r.returncode == 0, md
    assert rel in section(md, "Conformance")
    assert code_rows(md, "code.changed") == []


def test_k56_a_tests_file_is_information_and_exits_0(tmp_path):
    rig, h1 = code_rig(tmp_path)
    (rig / "tests" / "new_test.py").write_text("def test_x(): pass\n")
    camp, pre = code_campaign(rig, h1, [{"head": h1}])
    r, md = run_cli(rig, camp, pre)
    assert r.returncode == 0, md
    assert "tests/new_test.py" in section(md, "Conformance")


def test_k56_the_section_says_which_paths_are_not_code(tmp_path):
    rig, h1 = code_rig(tmp_path)
    camp, pre = code_campaign(rig, h1, [{"head": h1}])
    r, md = run_cli(rig, camp, pre)
    assert r.returncode == 0, md
    text = section(md, "Conformance")
    for name in ("README.md", "SPEC.md", "tests/"):
        assert name in text
    # it no longer lists the code paths
    assert "hcbench/, harness_bin/" not in text


# ---- K57 base images pinned by digest -----------------------------------------------------

PY_DIGEST = "sha256:5e2bdb1b0d542d5039cffb8181c65930093949194a65c365dfb1b0fe13a4fa5b"
FROM_RE = re.compile(r"^\s*FROM\s+(?:--\S+\s+)*(\S+)(?:\s+AS\s+(\S+))?\s*$", re.IGNORECASE)
PINNED_RE = re.compile(r".+@sha256:[0-9a-f]{64}$")


def from_lines(text):
    out, stages = [], set()
    for n, line in enumerate(text.splitlines(), 1):
        m = FROM_RE.match(line)
        if m:
            out.append((n, m.group(1), m.group(1).lower() in stages | {"scratch"}))
            if m.group(2):
                stages.add(m.group(2).lower())
    return out


DOCKERFILES = sorted((ROOT / "docker").glob("*.Dockerfile"))


def test_k57_there_are_three_dockerfiles_to_check():
    assert [p.name for p in DOCKERFILES] == [
        "mini.Dockerfile",
        "redteam.Dockerfile",
        "tools.Dockerfile",
    ]


@pytest.mark.parametrize("path", DOCKERFILES, ids=lambda p: p.name)
def test_k57_every_from_in_the_real_dockerfiles_is_pinned_by_digest(path):
    froms = from_lines(path.read_text())
    assert froms, "no FROM found"
    for n, image, exempt in froms:
        assert exempt or PINNED_RE.match(image), f"{path.name}:{n}: {image}"


@pytest.mark.parametrize("path", DOCKERFILES, ids=lambda p: p.name)
def test_k57_the_real_dockerfiles_use_the_pinned_python_index_digest(path):
    images = [image for _, image, exempt in from_lines(path.read_text()) if not exempt]
    assert images == [f"python:3.12-slim@{PY_DIGEST}"]


def dockerfile_rig(tmp_path, **files):
    """A clean throwaway repository whose docker/*.Dockerfile files have the given text."""
    rig = make_rig(tmp_path)
    for name, text in files.items():
        (rig / "docker" / name).write_text(text)
    git(rig, "init", "-q")
    git(rig, "add", "-A")
    git(rig, "commit", "-q", "-m", "rig")
    return rig


PINNED = f"python:3.12-slim@{PY_DIGEST}"


def test_k57_unpinned_from_is_refused_naming_file_and_line(tmp_path):
    rig = dockerfile_rig(
        tmp_path, **{"tools.Dockerfile": "# c\n\nFROM python:3.12-slim\nRUN true\n"}
    )
    r, out = prereg_cli(rig, tmp_path)
    assert r.returncode != 0
    assert re.search(r"docker/tools\.Dockerfile\D{0,12}3\b", r.stderr), r.stderr
    assert not out.exists()


def test_k57_every_unpinned_file_and_line_is_named(tmp_path):
    rig = dockerfile_rig(
        tmp_path,
        **{
            "mini.Dockerfile": f"FROM {PINNED} AS a\nFROM python:3.11\n",
            "redteam.Dockerfile": "FROM python:3.12-slim\n",
        },
    )
    r, _ = prereg_cli(rig, tmp_path)
    assert r.returncode != 0
    assert re.search(r"docker/mini\.Dockerfile\D{0,12}2\b", r.stderr), r.stderr
    assert re.search(r"docker/redteam\.Dockerfile\D{0,12}1\b", r.stderr), r.stderr


def test_k57_a_draft_records_an_unpinned_from_and_goes_on(tmp_path):
    rig = dockerfile_rig(tmp_path, **{"tools.Dockerfile": "FROM python:3.12-slim\n"})
    r, out = prereg_cli(rig, tmp_path, "--draft")
    assert r.returncode == 0, r.stderr
    assert tomllib.loads(out.read_text())["files_sha256"]["docker/tools.Dockerfile"]


def test_k57_stage_names_scratch_and_platform_are_allowed(tmp_path):
    text = (
        f"FROM --platform=linux/amd64 {PINNED} AS base\n"
        "FROM base AS second\n"
        "FROM scratch AS empty\n"
        "FROM scratch\n"
        "FROM second\n"
        f"FROM --platform=$BUILDPLATFORM {PINNED}\n"
    )
    rig = dockerfile_rig(tmp_path, **{"tools.Dockerfile": text})
    r, out = prereg_cli(rig, tmp_path)
    assert r.returncode == 0, r.stderr
    assert out.exists()


@pytest.mark.parametrize(
    "line",
    [
        "FROM ${BASE}",
        "FROM $BASE",
        "FROM --platform=linux/amd64 python:3.12-slim",
        "FROM python:3.12-slim AS base",
        f"FROM python:3.12-slim@sha256:{'a' * 63}",
        f"FROM python:3.12-slim@sha256:{'A' * 64}",
        f"FROM python:3.12-slim@sha256:{'g' * 64}",
        "FROM python:3.12-slim@sha256:",
        "FROM base",  # no earlier stage called base in this file
    ],
)
def test_k57_these_from_lines_count_as_unpinned(tmp_path, line):
    rig = dockerfile_rig(tmp_path, **{"tools.Dockerfile": f"ARG BASE=x\n{line}\n"})
    r, out = prereg_cli(rig, tmp_path)
    assert r.returncode != 0
    assert re.search(r"docker/tools\.Dockerfile\D{0,12}2\b", r.stderr), r.stderr
    assert not out.exists()


def test_k57_a_stage_name_only_counts_after_it_is_defined(tmp_path):
    text = f"FROM base\nFROM {PINNED} AS base\n"
    rig = dockerfile_rig(tmp_path, **{"tools.Dockerfile": text})
    r, _ = prereg_cli(rig, tmp_path)
    assert r.returncode != 0
    assert re.search(r"docker/tools\.Dockerfile\D{0,12}1\b", r.stderr), r.stderr


def test_k57_a_new_dockerfile_under_docker_is_checked_too(tmp_path):
    rig = dockerfile_rig(tmp_path, **{"extra.Dockerfile": "FROM alpine:3\n"})
    r, _ = prereg_cli(rig, tmp_path)
    assert r.returncode != 0
    assert "docker/extra.Dockerfile" in r.stderr


# ---- K58 the model server image pinned by digest ------------------------------------------

BAD_IMAGES = [
    "vllm/vllm-openai:v0.26.1",
    "vllm/vllm-openai",
    f"vllm/vllm-openai@sha256:{'a' * 63}",
    f"vllm/vllm-openai@sha256:{'A' * 64}",
    f"vllm/vllm-openai@sha256:{'g' * 64}",
    "vllm/vllm-openai:v0.26.1@sha256:",
    "vllm/vllm-openai:v0.26.1@md5:" + "a" * 64,
]


def serve(tmp_path, env):
    rig = make_rig(tmp_path)
    bindir, log = make_fake_bin(tmp_path)
    r = run_script(rig, "02_serve_model.sh", ["up"], bindir, env)
    return r, log


VLLM = {"HC_SERVER": "vllm", "HC_MODEL_HF": "org/model", "HC_MODEL_REVISION": SHA40}


@pytest.mark.parametrize("image", BAD_IMAGES)
def test_k58_vllm_refuses_an_unpinned_image_before_any_container_is_touched(tmp_path, image):
    r, log = serve(tmp_path, {**VLLM, "HC_VLLM_IMAGE": image})
    assert r.returncode != 0
    assert "HC_VLLM_IMAGE" in r.stderr
    assert harmful(log) == []


def test_k58_vllm_accepts_a_digest_pinned_image(tmp_path):
    r, log = serve(tmp_path, {**VLLM, "HC_VLLM_IMAGE": VLLM_PINNED})
    assert r.returncode == 0, r.stderr
    assert any(c[:1] == ["run"] for c in __import__("fakebin").docker_calls(log))


def test_k58_vllm_accepts_a_name_pinned_by_digest_alone(tmp_path):
    image = "vllm/vllm-openai@sha256:" + "0123456789abcdef" * 4
    r, _ = serve(tmp_path, {**VLLM, "HC_VLLM_IMAGE": image})
    assert r.returncode == 0, r.stderr


@pytest.mark.parametrize(
    "image", [i.replace("vllm/vllm-openai", "ghcr.io/ggml-org/llama.cpp") for i in BAD_IMAGES]
)
def test_k58_llamacpp_refuses_an_unpinned_image_before_any_container_is_touched(tmp_path, image):
    env = {"HC_SERVER": "llamacpp", "HC_LLAMACPP_IMAGE": image}
    r, log = serve(tmp_path, env)
    assert r.returncode != 0
    assert "HC_LLAMACPP_IMAGE" in r.stderr
    assert harmful(log) == []


def test_k58_llamacpp_accepts_a_digest_pinned_image(tmp_path):
    r, _ = serve(tmp_path, {"HC_SERVER": "llamacpp", "HC_LLAMACPP_IMAGE": LLAMA_PINNED})
    assert r.returncode == 0, r.stderr


def test_k58_mock_server_needs_no_image(tmp_path):
    env = {
        "HC_SERVER": "mock",
        "HC_VLLM_IMAGE": "vllm/vllm-openai:tag",
        "HC_LLAMACPP_IMAGE": "ghcr.io/ggml-org/llama.cpp:tag",
    }
    r, _ = serve(tmp_path, env)
    assert r.returncode == 0, r.stderr


def test_k58_only_the_image_of_the_chosen_server_is_checked(tmp_path):
    r, _ = serve(
        tmp_path,
        {**VLLM, "HC_VLLM_IMAGE": VLLM_PINNED, "HC_LLAMACPP_IMAGE": "ghcr.io/x/y:tag"},
    )
    assert r.returncode == 0, r.stderr
    r, _ = serve(
        tmp_path / "second",
        {"HC_SERVER": "llamacpp", "HC_LLAMACPP_IMAGE": LLAMA_PINNED, "HC_VLLM_IMAGE": "x:tag"},
    )
    assert r.returncode == 0, r.stderr


# non-draft pre-registration


@pytest.mark.parametrize("image", BAD_IMAGES)
def test_k58_non_draft_prereg_refuses_an_unpinned_vllm_image(env, image):
    pin(env)
    env["mp"].setenv("HC_VLLM_IMAGE", image)
    with pytest.raises(SystemExit) as exc:
        run_cb(env)
    assert "HC_VLLM_IMAGE" in str(exc.value)


def test_k58_draft_prereg_goes_on_with_an_unpinned_vllm_image(env):
    pin(env)
    env["mp"].setenv("HC_VLLM_IMAGE", "vllm/vllm-openai:v0.26.1")
    rc, out = run_cb(env, "--draft")
    assert rc == 0
    assert tomllib.loads(out.read_text())["config"]["HC_VLLM_IMAGE"] == "vllm/vllm-openai:v0.26.1"


def test_k58_non_draft_prereg_refuses_an_unpinned_llamacpp_image(env):
    pin(env)
    env["mp"].setenv("HC_SERVER", "llamacpp")
    env["mp"].setenv("HC_LLAMACPP_IMAGE", "ghcr.io/ggml-org/llama.cpp:server-cuda")
    with pytest.raises(SystemExit) as exc:
        run_cb(env)
    assert "HC_LLAMACPP_IMAGE" in str(exc.value)


def test_k58_draft_prereg_goes_on_with_an_unpinned_llamacpp_image(env):
    pin(env)
    env["mp"].setenv("HC_SERVER", "llamacpp")
    env["mp"].setenv("HC_LLAMACPP_IMAGE", "ghcr.io/ggml-org/llama.cpp:server-cuda")
    rc, _ = run_cb(env, "--draft")
    assert rc == 0


def test_k58_non_draft_prereg_checks_only_the_image_of_the_chosen_server(env):
    pin(env)
    env["mp"].setenv("HC_SERVER", "llamacpp")
    env["mp"].setenv("HC_LLAMACPP_IMAGE", LLAMA_PINNED)
    env["mp"].setenv("HC_VLLM_IMAGE", "vllm/vllm-openai:v0.26.1")
    rc, _ = run_cb(env)
    assert rc == 0


def test_k58_mock_server_prereg_needs_no_image(env):
    pin(env)
    env["mp"].setenv("HC_SERVER", "mock")
    env["mp"].setenv("HC_VLLM_IMAGE", "vllm/vllm-openai:tag")
    env["mp"].setenv("HC_LLAMACPP_IMAGE", "ghcr.io/x/y:tag")
    rc, _ = run_cb(env)
    assert rc == 0


def test_k58_a_pinned_vllm_image_is_accepted_by_a_non_draft_prereg(env):
    pin(env)
    rc, out = run_cb(env)
    assert rc == 0
    assert tomllib.loads(out.read_text())["config"]["HC_VLLM_IMAGE"] == VLLM_PINNED


# the example file


def example_text():
    return (ROOT / "config" / "bench.env.example").read_text()


def test_k58_the_example_documents_the_tag_at_digest_form():
    assert "vllm/vllm-openai:<tag>@sha256:<digest>" in example_text()


def test_k58_the_vllm_image_default_stays_a_placeholder():
    (line,) = [ln for ln in example_text().splitlines() if ln.startswith("HC_VLLM_IMAGE=")]
    assert "CHANGE-ME" in line


def test_k58_the_example_says_the_llamacpp_image_is_pinned_the_same_way():
    text = example_text()
    start = text.index("llama.cpp alternative")
    assert "@sha256:" in text[start : start + 400]


def test_helpers_are_importable():
    assert subprocess and git_rig and make_rig
