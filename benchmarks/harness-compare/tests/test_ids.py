"""Spec section 1: hcbench.ids."""

import pytest

from hcbench.ids import make_run_id, parse_run_id


def test_make_run_id_when_valid_should_use_documented_format():
    assert make_run_id("c", "pi", 0, "rt01") == "c.pi.s0.rt01"


def test_make_run_id_when_task_id_has_dots_underscores_dashes_should_accept():
    run_id = make_run_id("a", "hone-v2_x", 12, "django__django-1.2")
    assert run_id == "a.hone-v2_x.s12.django__django-1.2"


@pytest.mark.parametrize(
    ("suite", "arm", "seed", "task_id"),
    [
        ("", "pi", 0, "t"),
        ("C", "pi", 0, "t"),
        ("_c", "pi", 0, "t"),
        ("-c", "pi", 0, "t"),
        ("c.d", "pi", 0, "t"),
        ("c", "", 0, "t"),
        ("c", "Pi", 0, "t"),
        ("c", "p i", 0, "t"),
        ("c", "pi", -1, "t"),
        ("c", "pi", 0, ""),
        ("c", "pi", 0, "a/b"),
        ("c", "pi", 0, "a b"),
        ("c", "pi", 0, "a:b"),
    ],
)
def test_make_run_id_when_component_invalid_should_raise_value_error(suite, arm, seed, task_id):
    with pytest.raises(ValueError):
        make_run_id(suite, arm, seed, task_id)


@pytest.mark.parametrize(
    "args",
    [
        ("a", "pi", 0, "t"),
        ("swe", "hone-1", 7, "django__django-11099"),
        ("c", "x", 3, "task.with.dots"),
    ],
)
def test_parse_run_id_when_roundtrip_should_return_original_parts(args):
    assert parse_run_id(make_run_id(*args)) == args


def test_parse_run_id_when_task_id_contains_dots_should_split_on_first_three_only():
    assert parse_run_id("c.pi.s2.a.b.c") == ("c", "pi", 2, "a.b.c")


def test_parse_run_id_should_return_seed_as_int():
    _, _, seed, _ = parse_run_id("c.pi.s10.t")
    assert isinstance(seed, int)
    assert seed == 10


@pytest.mark.parametrize(
    "bad",
    ["", "c", "c.pi", "c.pi.s0", "c.pi.0.t", "c.pi.sx.t", "c.pi.s.t", "c.pi.1x.t"],
)
def test_parse_run_id_when_malformed_should_raise_value_error(bad):
    with pytest.raises(ValueError):
        parse_run_id(bad)
