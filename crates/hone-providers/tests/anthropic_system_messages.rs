//! AS1-AS4 of docs/plans/anthropic-system-messages-spec.md: every system message
//! of a request reaches the Anthropic `system` field, in order, with at most
//! 4 cache breakpoints per request.
#![allow(clippy::expect_used, clippy::unwrap_used, clippy::panic)]

use hone_proto::{Message, ModelId, ToolSchema};
use hone_providers::adapters::anthropic::build_request_body;
use hone_providers::traits::CompletionRequest;
use serde_json::Value;

fn request(messages: Vec<Message>, tools: Vec<ToolSchema>) -> CompletionRequest {
    CompletionRequest {
        model: ModelId::new("anthropic:claude-sonnet-4-5"),
        messages,
        tools,
        temperature: None,
        max_tokens: None,
        stop: Vec::new(),
        cancel: None,
        tool_results: None,
    }
}

fn tool(name: &str) -> ToolSchema {
    ToolSchema {
        name: name.to_owned(),
        description: "d".to_owned(),
        parameters: serde_json::json!({"type": "object", "properties": {}}),
    }
}

fn system_texts(body: &Value) -> Vec<String> {
    body["system"]
        .as_array()
        .map(|a| {
            a.iter()
                .map(|b| b["text"].as_str().unwrap_or_default().to_owned())
                .collect()
        })
        .unwrap_or_default()
}

fn message_roles(body: &Value) -> Vec<String> {
    body["messages"]
        .as_array()
        .unwrap()
        .iter()
        .map(|m| m["role"].as_str().unwrap().to_owned())
        .collect()
}

fn count_cache_control(v: &Value) -> usize {
    match v {
        Value::Object(m) => {
            usize::from(m.contains_key("cache_control"))
                + m.values().map(count_cache_control).sum::<usize>()
        }
        Value::Array(a) => a.iter().map(count_cache_control).sum(),
        _ => 0,
    }
}

fn has_cache_control(block: &Value) -> bool {
    block.get("cache_control").is_some_and(|c| c["type"] == "ephemeral")
}

// ---- AS1 ------------------------------------------------------------------

#[test]
fn as1_no_system_message_means_no_system_field() {
    let body = build_request_body(&request(vec![Message::user("hi")], vec![]));
    assert!(body.get("system").is_none(), "unexpected system: {body}");
}

#[test]
fn as1_single_system_message_is_one_block() {
    let body = build_request_body(&request(
        vec![Message::system("base"), Message::user("hi")],
        vec![],
    ));
    assert_eq!(system_texts(&body), vec!["base"]);
    assert_eq!(message_roles(&body), vec!["user"]);
}

#[test]
fn as1_three_leading_system_messages_all_sent_in_order() {
    let body = build_request_body(&request(
        vec![
            Message::system("base prompt"),
            Message::system("# Relevant Memories\n- a fact"),
            Message::system("# Skills\n- a skill"),
            Message::user("hi"),
        ],
        vec![],
    ));
    assert_eq!(
        system_texts(&body),
        vec![
            "base prompt",
            "# Relevant Memories\n- a fact",
            "# Skills\n- a skill"
        ]
    );
    for b in body["system"].as_array().unwrap() {
        assert_eq!(b["type"], "text");
    }
    assert_eq!(message_roles(&body), vec!["user"]);
    assert!(!body["messages"].to_string().contains("Relevant Memories"));
}

// ---- AS2 ------------------------------------------------------------------

#[test]
fn as2_single_system_block_keeps_cache_control() {
    let body = build_request_body(&request(
        vec![Message::system("base"), Message::user("hi")],
        vec![],
    ));
    assert!(has_cache_control(&body["system"][0]));
}

#[test]
fn as2_first_and_last_system_blocks_have_cache_control_middle_does_not() {
    let body = build_request_body(&request(
        vec![
            Message::system("one"),
            Message::system("two"),
            Message::system("three"),
            Message::user("hi"),
        ],
        vec![],
    ));
    let blocks = body["system"].as_array().expect("system array");
    assert_eq!(blocks.len(), 3);
    assert!(has_cache_control(&blocks[0]), "first block");
    assert!(!has_cache_control(&blocks[1]), "middle block");
    assert!(has_cache_control(&blocks[2]), "last block");
}

#[test]
fn as2_two_system_blocks_both_have_cache_control() {
    let body = build_request_body(&request(
        vec![Message::system("one"), Message::system("two"), Message::user("hi")],
        vec![],
    ));
    let blocks = body["system"].as_array().expect("system array");
    assert_eq!(blocks.len(), 2);
    assert!(has_cache_control(&blocks[0]));
    assert!(has_cache_control(&blocks[1]));
}

#[test]
fn as2_breakpoints_use_all_four_slots_but_never_exceed_four() {
    // tools (1) + first system (1) + last system (1) + conversation (1) = 4
    let body = build_request_body(&request(
        vec![
            Message::system("one"),
            Message::system("two"),
            Message::system("three"),
            Message::user("q1"),
            Message::assistant("a1"),
            Message::user("q2"),
            Message::assistant("a2"),
            Message::user("q3"),
        ],
        vec![tool("read_file"), tool("write_file")],
    ));
    assert_eq!(count_cache_control(&body), 4, "body: {body}");
}

#[test]
fn as2_total_breakpoints_never_exceed_four_with_many_system_messages() {
    let mut msgs: Vec<Message> = (0..6).map(|i| Message::system(format!("sys {i}"))).collect();
    msgs.extend([
        Message::user("q1"),
        Message::assistant("a1"),
        Message::user("q2"),
    ]);
    let body = build_request_body(&request(msgs, vec![tool("t1")]));
    assert!(count_cache_control(&body) <= 4, "body: {body}");
    assert_eq!(body["system"].as_array().unwrap().len(), 6);
}

// ---- AS3 ------------------------------------------------------------------

#[test]
fn as3_mid_history_system_message_is_delivered_in_request_order() {
    let body = build_request_body(&request(
        vec![
            Message::system("base"),
            Message::system("memories"),
            Message::user("q1"),
            Message::assistant("a1"),
            Message::system("[session history: 4 messages compressed]"),
            Message::user("q2"),
        ],
        vec![],
    ));
    assert_eq!(
        system_texts(&body),
        vec![
            "base",
            "memories",
            "[session history: 4 messages compressed]"
        ]
    );
    assert_eq!(message_roles(&body), vec!["user", "assistant", "user"]);
    assert!(!body["messages"].to_string().contains("session history"));
}

// ---- AS4 ------------------------------------------------------------------

#[test]
fn as4_empty_and_whitespace_system_messages_are_skipped() {
    let body = build_request_body(&request(
        vec![
            Message::system("base"),
            Message::system(""),
            Message::system("  \n\t "),
            Message::system("memories"),
            Message::user("hi"),
        ],
        vec![],
    ));
    assert_eq!(system_texts(&body), vec!["base", "memories"]);
    let blocks = body["system"].as_array().unwrap();
    assert!(has_cache_control(&blocks[0]));
    assert!(has_cache_control(&blocks[1]));
}

#[test]
fn as4_only_empty_system_messages_means_no_system_field() {
    let body = build_request_body(&request(
        vec![Message::system("   "), Message::user("hi")],
        vec![],
    ));
    assert!(body.get("system").is_none(), "unexpected system: {body}");
}
