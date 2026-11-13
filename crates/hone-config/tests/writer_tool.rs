#![allow(clippy::expect_used, clippy::unwrap_used, clippy::panic)]
//! W1 of `docs/plans/memory-write-tool-spec.md`: the writer names `tool` and
//! `triggers+tool` are valid for `[memory] writer`, and a project file can only turn the
//! writer off (so it cannot add the tool). API: PR D's `hone_config::strategy`.

use std::path::Path;

use hone_config::resolve::ConfigLayer;
use hone_config::strategy::{Flags, Origin, StrategySelection};

const USER: &str = "/home/u/.config/hone/config.toml";
const PROJECT: &str = "/repo/.hone/config.toml";

fn layer(path: &str, raw: &str) -> ConfigLayer {
    ConfigLayer::from_toml(path, raw).unwrap_or_else(|e| panic!("fixture should parse: {e}\n{raw}"))
}

fn select(user: Option<&ConfigLayer>, project: Option<&ConfigLayer>) -> StrategySelection {
    StrategySelection::from_sources(user, project, &|_| None, &Flags::default()).unwrap()
}

#[test]
fn test_w1_a_user_file_may_choose_tool_and_triggers_plus_tool() {
    for name in ["tool", "triggers+tool"] {
        let u = layer(USER, &format!("[memory]\nwriter = \"{name}\"\n"));
        let s = select(Some(&u), None);
        assert_eq!(s.writer.value, name);
        assert_eq!(s.writer.origin, Origin::File(USER.into()));
    }
}

#[test]
fn test_w1_a_project_file_cannot_add_the_tool() {
    for name in ["tool", "triggers+tool"] {
        let p = layer(PROJECT, &format!("[memory]\nwriter = \"{name}\"\n"));

        let s = select(None, Some(&p));

        assert_eq!(s.writer.value, "triggers", "{name}: the default stays");
        assert_eq!(s.writer.origin, Origin::Default);
        let w: Vec<&str> = s
            .warnings
            .iter()
            .filter(|w| w.path == Path::new(PROJECT))
            .map(|w| w.message.as_str())
            .collect();
        assert_eq!(w.len(), 1, "{name}: one warning naming the project file: {:?}", s.warnings);
        assert!(w[0].contains("writer"), "{}", w[0]);
    }
}

#[test]
fn test_w1_a_project_file_cannot_widen_a_users_off_but_may_still_turn_the_tool_off() {
    let u = layer(USER, "[memory]\nwriter = \"tool\"\n");
    let p = layer(PROJECT, "[memory]\nwriter = \"off\"\n");
    let s = select(Some(&u), Some(&p));
    assert_eq!((s.writer.value.as_str(), s.writer.origin), ("off", Origin::File(PROJECT.into())));

    let u = layer(USER, "[memory]\nwriter = \"off\"\n");
    let p = layer(PROJECT, "[memory]\nwriter = \"tool\"\n");
    let s = select(Some(&u), Some(&p));
    assert_eq!(s.writer.value, "off");
}

#[test]
fn test_w1_an_unknown_writer_should_list_tool_and_triggers_plus_tool_among_the_valid_names() {
    let u = layer(USER, "[memory]\nwriter = \"llm-extract\"\n");

    let e = StrategySelection::from_sources(Some(&u), None, &|_| None, &Flags::default())
        .err()
        .expect("unknown writer")
        .to_string();

    for needle in ["llm-extract", "triggers", "tool", "triggers+tool", "off"] {
        assert!(e.contains(needle), "{needle:?} in {e}");
    }
}
