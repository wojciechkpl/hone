"""Round 2: hone-inject vs hermes is a primary comparison (Holm family changes)."""

from hcbench.report import PAIRS


def test_pairs_should_include_hone_inject_vs_hermes_and_keep_existing():
    assert ("hone-inject", "hermes") in PAIRS
    assert ("hone-inject", "pi") in PAIRS
    assert ("hone-inject", "hone-noinject") in PAIRS
