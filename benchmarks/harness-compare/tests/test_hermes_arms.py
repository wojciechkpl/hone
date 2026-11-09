"""Spec section 7 (Hermes arms): config/arms.toml entries `hermes` and `hermes-guarded`.

These tests read the real, pre-registered config/arms.toml.
"""

import re
from pathlib import Path

import pytest

from hcbench.arms import load_arms, render

ROOT = Path(__file__).resolve().parent.parent
ARMS_PATH = ROOT / "config" / "arms.toml"
ENV_EXAMPLE = ROOT / "config" / "bench.env.example"

CTX = {
    "prompt": "SENTINEL prompt\nwith 'quotes' and $HOME",
    "model": "qwen3-coder-30b-a3b",
    "base_url": "http://hc-proxy:8100/run/a.b.s0.t",
    "run_id": "a.b.s0.t",
    "workdir": "/testbed",
    "context_window": "65536",
    "max_tokens": "6144",
}

COMMON_ARGV = [
    "/opt/harness/bin/hermes", "chat", "-Q", "-q", "{{prompt}}",
    "--format", "stream-json",
    "--provider", "custom", "-m", "{{model}}",
]  # fmt: skip
# `hermes chat` has no --base-url / --api-key (verified at v2026.9.24): the endpoint comes from
# config.yaml under `model:`. Never pass --ignore-user-config: it would skip that file.


@pytest.fixture(scope="module")
def arms():
    return load_arms(ARMS_PATH)


def _scalar(val: str):
    if val in ("true", "false"):
        return val == "true"
    if re.fullmatch(r"-?\d+", val):
        return int(val)
    if val.startswith("[") and val.endswith("]"):
        return [x.strip().strip("'\"") for x in val[1:-1].split(",") if x.strip()]
    return val.strip("'\"")


def mini_yaml(text: str) -> dict:
    """Minimal parser: 2-space nested mappings, scalars, flow lists and `- item` block lists."""
    root: dict = {}
    stack = [(-1, root)]
    pending = None  # (parent, key) of the last empty-valued key
    for raw in text.splitlines():
        if not raw.strip() or raw.lstrip().startswith("#"):
            continue
        indent = len(raw) - len(raw.lstrip())
        body = raw.strip()
        if body.startswith("- "):
            parent, key = pending
            if not isinstance(parent[key], list):
                parent[key] = []
            parent[key].append(_scalar(body[2:].strip()))
            continue
        key, _, val = body.partition(":")
        while stack[-1][0] >= indent:
            stack.pop()
        parent = stack[-1][1]
        val = val.strip()
        if val == "":
            child: dict = {}
            parent[key] = child
            stack.append((indent, child))
            pending = (parent, key)
        else:
            parent[key] = _scalar(val)
    return root


# ---- registration ------------------------------------------------------


def test_arms_toml_should_define_hermes_and_hermes_guarded_with_kind_hermes(arms):
    assert {"hermes", "hermes-guarded"} <= set(arms)
    assert arms["hermes"].kind == "hermes"
    assert arms["hermes-guarded"].kind == "hermes"


def test_load_arms_should_accept_kind_hermes(tmp_path):
    p = tmp_path / "arms.toml"
    p.write_text('[arms.h]\nkind = "hermes"\nargv = ["hermes"]\n')
    assert load_arms(p)["h"].kind == "hermes"


# ---- argv --------------------------------------------------------------


def test_hermes_arm_argv_should_match_preregistered_template(arms):
    assert list(arms["hermes"].argv) == [*COMMON_ARGV, "--yolo", "--ignore-rules"]


def test_hermes_guarded_argv_should_equal_hermes_without_yolo(arms):
    assert list(arms["hermes-guarded"].argv) == [*COMMON_ARGV, "--ignore-rules"]
    assert "--yolo" not in arms["hermes-guarded"].argv
    assert [a for a in arms["hermes"].argv if a != "--yolo"] == list(arms["hermes-guarded"].argv)


@pytest.mark.parametrize("name", ["hermes", "hermes-guarded"])
def test_hermes_arm_prompt_should_be_one_untouched_argv_element_with_no_extra_text(arms, name):
    arm = arms[name]
    assert arm.argv.count("{{prompt}}") == 1
    assert sum("{{prompt}}" in a for a in arm.argv) == 1  # never embedded in a longer string
    argv, _, _ = render(arm, CTX)
    assert argv.count(CTX["prompt"]) == 1
    assert sum("SENTINEL" in a for a in argv) == 1
    assert argv[argv.index("-q") + 1] == CTX["prompt"]
    assert argv[0] == "/opt/harness/bin/hermes"
    assert "-c" not in argv  # no shell wrapper


@pytest.mark.parametrize("name", ["hermes", "hermes-guarded"])
def test_hermes_arm_should_render_with_every_placeholder_filled(arms, name):
    argv, env, files = render(arms[name], CTX)
    blob = "\n".join([*argv, *env.values(), *files, *files.values()])
    assert "{{" not in blob
    assert argv[argv.index("-m") + 1] == CTX["model"]
    cfg = mini_yaml(files["/tmp/hc-hermes-home/config.yaml"])
    assert cfg["model"]["base_url"] == CTX["base_url"] + "/v1"


@pytest.mark.parametrize("name", ["hermes", "hermes-guarded"])
def test_hermes_arm_argv_should_not_use_flags_hermes_chat_lacks_or_skip_the_config(arms, name):
    argv = arms[name].argv
    for flag in ("--base-url", "--api-key", "--ignore-user-config"):
        assert flag not in argv


@pytest.mark.parametrize("name", ["hermes", "hermes-guarded"])
def test_hermes_arm_config_should_carry_the_custom_endpoint_under_model(arms, name):
    _, _, files = render(arms[name], CTX)
    model = mini_yaml(files["/tmp/hc-hermes-home/config.yaml"])["model"]
    assert model["provider"] == "custom"
    assert model["base_url"] == "http://hc-proxy:8100/run/a.b.s0.t/v1"
    assert model["api_key"] == "hcbench"
    assert model["context_length"] == 65536


# ---- env + files -------------------------------------------------------


@pytest.mark.parametrize("name", ["hermes", "hermes-guarded"])
def test_hermes_arm_env_should_point_hermes_home_at_fresh_tmp_dir(arms, name):
    _, env, _ = render(arms[name], CTX)
    assert env["HERMES_HOME"] == "/tmp/hc-hermes-home"
    # Hermes must not reach a real provider: no ambient keys are injected by the arm.
    assert not any(k.endswith("API_KEY") and v != "hcbench" for k, v in env.items())


@pytest.mark.parametrize("name", ["hermes", "hermes-guarded"])
def test_hermes_arm_files_should_write_only_config_yaml_under_hermes_home(arms, name):
    _, env, files = render(arms[name], CTX)
    assert list(files) == ["/tmp/hc-hermes-home/config.yaml"]
    assert files["/tmp/hc-hermes-home/config.yaml"].strip() != ""
    assert str(Path(next(iter(files))).parent) == env["HERMES_HOME"]


@pytest.mark.parametrize("name", ["hermes", "hermes-guarded"])
def test_hermes_arm_config_should_disable_memory_and_use_local_terminal(arms, name):
    _, _, files = render(arms[name], CTX)
    cfg = mini_yaml(files["/tmp/hc-hermes-home/config.yaml"])
    assert cfg["memory"]["memory_enabled"] is False
    assert cfg["memory"]["user_profile_enabled"] is False
    assert cfg["terminal"]["backend"] == "local"


@pytest.mark.parametrize("ctx_window", ["65536", "32768", "131072"])
def test_hermes_arm_config_should_render_the_served_context_window(arms, ctx_window):
    _, _, files = render(arms["hermes"], {**CTX, "context_window": ctx_window})
    cfg = mini_yaml(files["/tmp/hc-hermes-home/config.yaml"])
    assert cfg["model"]["context_length"] == int(ctx_window)


def test_hermes_and_guarded_should_share_env_and_config_files(arms):
    a, b = arms["hermes"], arms["hermes-guarded"]
    assert dict(a.env) == dict(b.env)
    assert dict(a.files) == dict(b.files)


def test_hermes_arms_should_use_only_runner_supported_placeholders(arms):
    allowed = set(CTX)
    for name in ("hermes", "hermes-guarded"):
        arm = arms[name]
        text = "\n".join([*arm.argv, *arm.env.values(), *arm.files, *arm.files.values()])
        assert set(re.findall(r"\{\{([a-z_][a-z0-9_]*)\}\}", text)) <= allowed


# ---- campaign arm lists (config/bench.env.example) ---------------------


def env_example() -> dict[str, str]:
    from hcbench.config import load_env_file

    return load_env_file(ENV_EXAMPLE, base={"HOME": "/h", "HC_ROOT": str(ROOT)})


def test_env_example_main_arms_should_include_hermes_and_keep_existing_arms():
    main = env_example()["HC_ARMS_MAIN"].split(",")
    assert set(main) == {"pi", "hone-noinject", "hone-inject", "hermes"}


def test_env_example_redteam_arms_should_include_both_hermes_arms():
    red = env_example()["HC_ARMS_REDTEAM"].split(",")
    assert {"pi", "pi-guarded", "hone-inject", "hone-guarded", "hermes", "hermes-guarded"} <= set(
        red
    )


def test_env_example_arm_lists_should_only_name_arms_defined_in_arms_toml(arms):
    env = env_example()
    for key in ("HC_ARMS_MAIN", "HC_ARMS_REDTEAM"):
        assert set(env[key].split(",")) <= set(arms)


# ---- round 2: skills/cron off, update check off, explicit approvals, empty bundled skills ----


@pytest.mark.parametrize("name", ["hermes", "hermes-guarded"])
def test_hermes_arm_config_should_disable_skills_and_cron_toolsets(arms, name):
    _, _, files = render(arms[name], CTX)
    cfg = mini_yaml(files["/tmp/hc-hermes-home/config.yaml"])
    assert {"skills", "cronjob"} <= set(cfg["agent"]["disabled_toolsets"])


@pytest.mark.parametrize("name", ["hermes", "hermes-guarded"])
def test_hermes_arm_config_should_turn_update_check_off(arms, name):
    _, _, files = render(arms[name], CTX)
    assert mini_yaml(files["/tmp/hc-hermes-home/config.yaml"])["updates"]["check"] is False


@pytest.mark.parametrize("name", ["hermes", "hermes-guarded"])
def test_hermes_arms_config_should_pin_documented_approval_defaults_explicitly(arms, name):
    # Both arms write them (--yolo overrides approvals on the permissive arm), so the
    # documented defaults are pre-registered for both.
    _, _, files = render(arms[name], CTX)
    appr = mini_yaml(files["/tmp/hc-hermes-home/config.yaml"])["approvals"]
    assert appr["mode"] == "smart"
    assert appr["single_query_mode"] == "deny"


@pytest.mark.parametrize("name", ["hermes", "hermes-guarded"])
def test_hermes_arm_env_should_point_bundled_skills_at_a_path_that_does_not_exist(arms, name):
    # With an existing (even empty) dir, skills_sync.sync_skills() writes
    # $HERMES_HOME/skills/.bundled_manifest, which probe P-H5 would flag. With a nonexistent
    # path it writes nothing. The bundle build must never create this path.
    _, env, _ = render(arms[name], CTX)
    assert env["HERMES_BUNDLED_SKILLS"] == "/opt/harness/hermes/no-bundled-skills"
