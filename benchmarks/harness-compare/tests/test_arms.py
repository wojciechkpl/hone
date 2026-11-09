"""Spec section 7: hcbench.arms."""

import dataclasses
from pathlib import Path

import pytest

from hcbench.arms import Arm, load_arms, render

ARMS_TOML = '''
[arms.pi]
kind = "pi"
argv = ["pi", "--mode", "json", "--model={{model}}", "-p", "{{prompt}}"]

[arms.pi.env]
OPENAI_BASE_URL = "{{base_url}}"
PLAIN = "literal"

[arms.pi.files]
"{{workdir}}/.pi/settings.json" = """{"model": "{{model}}", "nested": {"a": [1, 2]}}"""

[arms.hone-base]
kind = "hone"
argv = ["hone", "run", "{{prompt}}"]

[arms.selftest]
kind = "selftest"
argv = ["true"]
'''


def write(tmp_path: Path, text: str) -> Path:
    p = tmp_path / "arms.toml"
    p.write_text(text)
    return p


def test_load_arms_should_parse_all_arms_with_fields(tmp_path):
    arms = load_arms(write(tmp_path, ARMS_TOML))
    assert set(arms) == {"pi", "hone-base", "selftest"}
    pi = arms["pi"]
    assert isinstance(pi, Arm)
    assert pi.name == "pi"
    assert pi.kind == "pi"
    assert pi.argv == ("pi", "--mode", "json", "--model={{model}}", "-p", "{{prompt}}")
    assert dict(pi.env) == {"OPENAI_BASE_URL": "{{base_url}}", "PLAIN": "literal"}
    assert list(pi.files) == ["{{workdir}}/.pi/settings.json"]
    assert arms["hone-base"].kind == "hone"
    assert arms["selftest"].kind == "selftest"


def test_load_arms_when_env_and_files_omitted_should_default_to_empty(tmp_path):
    arms = load_arms(write(tmp_path, '[arms.a]\nkind = "pi"\nargv = ["pi"]\n'))
    assert dict(arms["a"].env) == {}
    assert dict(arms["a"].files) == {}


def test_arm_should_be_frozen(tmp_path):
    arm = load_arms(write(tmp_path, '[arms.a]\nkind = "pi"\nargv = ["pi"]\n'))["a"]
    with pytest.raises(dataclasses.FrozenInstanceError):
        arm.name = "other"  # type: ignore[misc]


@pytest.mark.parametrize(
    "toml_text",
    [
        '[arms.a]\nkind = "claude"\nargv = ["x"]\n',  # unknown kind
        '[arms.a]\nkind = "pi"\n',  # missing argv
        '[arms."Bad Name"]\nkind = "pi"\nargv = ["x"]\n',  # invalid name
        '[arms.UPPER]\nkind = "pi"\nargv = ["x"]\n',  # invalid name (uppercase)
    ],
)
def test_load_arms_when_invalid_should_raise_value_error(tmp_path, toml_text):
    with pytest.raises(ValueError):
        load_arms(write(tmp_path, toml_text))


CTX = {
    "model": "qwen3-coder",
    "prompt": "fix the bug",
    "base_url": "http://127.0.0.1:9000/run/x/v1",
    "workdir": "/work",
}


def test_render_should_substitute_argv_env_file_paths_and_contents(tmp_path):
    arm = load_arms(write(tmp_path, ARMS_TOML))["pi"]
    argv, env, files = render(arm, CTX)
    assert argv == ["pi", "--mode", "json", "--model=qwen3-coder", "-p", "fix the bug"]
    assert env == {"OPENAI_BASE_URL": "http://127.0.0.1:9000/run/x/v1", "PLAIN": "literal"}
    assert list(files) == ["/work/.pi/settings.json"]


def test_render_should_leave_single_brace_json_untouched(tmp_path):
    arm = load_arms(write(tmp_path, ARMS_TOML))["pi"]
    _, _, files = render(arm, CTX)
    assert files["/work/.pi/settings.json"] == '{"model": "qwen3-coder", "nested": {"a": [1, 2]}}'


def test_render_when_prompt_has_spaces_newlines_quotes_should_stay_one_argv_element():
    arm = Arm("a", "pi", ("pi", "-p", "{{prompt}}"), {}, {})
    prompt = 'line one\nline "two" it\'s $HOME `x`; rm -rf /'
    argv, _, _ = render(arm, {"prompt": prompt})
    assert argv == ["pi", "-p", prompt]
    assert isinstance(argv, list)


def test_render_when_value_looks_like_placeholder_should_not_be_resubstituted():
    arm = Arm("a", "pi", ("{{prompt}}",), {}, {})
    argv, _, _ = render(arm, {"prompt": "{{model}}", "model": "m"})
    assert argv == ["{{model}}"]


def test_render_when_unknown_placeholder_should_raise_key_error():
    arm = Arm("a", "pi", ("pi", "{{nope}}"), {}, {})
    with pytest.raises(KeyError):
        render(arm, {"prompt": "x"})


@pytest.mark.parametrize("where", ["env", "file_path", "file_content"])
def test_render_when_unknown_placeholder_anywhere_should_raise_key_error(where):
    env = {"K": "{{nope}}"} if where == "env" else {}
    files = {"{{nope}}": "x"} if where == "file_path" else {}
    if where == "file_content":
        files = {"/p": "{{nope}}"}
    arm = Arm("a", "pi", ("pi",), env, files)
    with pytest.raises(KeyError):
        render(arm, {})


def test_render_should_ignore_unused_context_keys():
    arm = Arm("a", "pi", ("pi",), {}, {})
    assert render(arm, {"model": "m"}) == (["pi"], {}, {})
