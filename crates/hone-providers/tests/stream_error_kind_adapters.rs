#![allow(clippy::expect_used, clippy::unwrap_used, clippy::panic)]
//! E2-E4 (`docs/plans/provider-error-kind-spec.md`): the HTTP adapters classify the `Error`
//! events they emit. Transport failures: stall -> `IdleTimeout`, cut / clean end without the
//! completion marker -> `Interrupted`. In-band errors: `Provider { status }` from the provider's
//! code (rules and name table of E3/E4). Messages are unchanged.

mod common;

use std::time::Duration;

use common::{ADAPTERS, Adapter, End, Step, head, piece, run, text};
use hone_proto::{ProviderEvent, StreamErrorKind};
use serde_json::{Value, json};

const IDLE: Duration = Duration::from_secs(1);

fn error_events(events: &[ProviderEvent]) -> Vec<(String, StreamErrorKind)> {
    events
        .iter()
        .filter_map(|e| match e {
            ProviderEvent::Error { message, kind } => Some((message.clone(), *kind)),
            _ => None,
        })
        .collect()
}

fn only_kind(a: Adapter, what: &str, events: &[ProviderEvent]) -> StreamErrorKind {
    let errs = error_events(events);
    assert_eq!(errs.len(), 1, "{a:?} {what}: {events:?}");
    errs[0].1
}

// -- E2: transport failures --------------------------------------------------------------------

#[tokio::test]
async fn test_e2_idle_stall_is_idle_timeout() {
    for a in ADAPTERS {
        let events = run(
            a,
            vec![Step::Delay(Duration::from_secs(3))],
            End::Normal,
            Some(IDLE),
        )
        .await;
        assert_eq!(
            only_kind(a, "stall", &events),
            StreamErrorKind::IdleTimeout,
            "{a:?}"
        );
    }
}

#[tokio::test]
async fn test_e2_stall_after_output_is_idle_timeout_too() {
    for a in ADAPTERS {
        let steps = vec![
            piece(head(a)),
            piece(text(a, "x")),
            Step::Delay(Duration::from_secs(3)),
        ];
        let events = run(a, steps, End::Normal, Some(IDLE)).await;
        assert_eq!(
            only_kind(a, "stall after output", &events),
            StreamErrorKind::IdleTimeout,
            "{a:?}"
        );
    }
}

#[tokio::test]
async fn test_e2_cut_socket_is_interrupted() {
    for a in ADAPTERS {
        let steps = vec![piece(head(a)), piece(text(a, "x"))];
        let events = run(a, steps, End::CutOff, None).await;
        assert_eq!(
            only_kind(a, "cut", &events),
            StreamErrorKind::Interrupted,
            "{a:?}"
        );
    }
}

#[tokio::test]
async fn test_e2_clean_end_without_the_completion_marker_is_interrupted() {
    for a in ADAPTERS {
        let steps = vec![piece(head(a)), piece(text(a, "x"))];
        let events = run(a, steps, End::Normal, None).await;
        assert_eq!(
            only_kind(a, "no marker", &events),
            StreamErrorKind::Interrupted,
            "{a:?}"
        );
    }
}

// -- E3 / E4: in-band errors ---------------------------------------------------------------------------

fn data(v: Value) -> String {
    format!("data: {v}\n\n")
}

/// An OpenAI-compatible stream consisting of one in-band error object.
async fn openai_error(error: Value) -> Vec<(String, StreamErrorKind)> {
    let body = data(json!({ "error": error }));
    error_events(&run(Adapter::OpenAi, vec![piece(body)], End::Normal, None).await)
}

fn provider(status: Option<u16>) -> StreamErrorKind {
    StreamErrorKind::Provider { status }
}

const NAME_TABLE: [(&str, u16); 11] = [
    ("invalid_request_error", 400),
    ("authentication_error", 401),
    ("permission_error", 403),
    ("not_found_error", 404),
    ("request_too_large", 413),
    ("rate_limit_error", 429),
    ("api_error", 500),
    ("overloaded_error", 529),
    ("server_error", 500),
    ("context_length_exceeded", 400),
    ("rate_limit_exceeded", 429),
];

#[tokio::test]
async fn test_e3_openai_integer_code_in_range_is_the_status() {
    let errs =
        openai_error(json!({"message": "maximum context length exceeded", "code": 400})).await;
    assert_eq!(
        errs,
        vec![(
            "maximum context length exceeded".to_owned(),
            provider(Some(400))
        )]
    );
    for code in [100, 429, 502, 599] {
        let errs = openai_error(json!({"message": "m", "code": code})).await;
        assert_eq!(errs[0].1, provider(Some(code)), "code {code}");
    }
}

#[tokio::test]
async fn test_e3_openai_digit_string_code_in_range_is_the_status() {
    let errs = openai_error(json!({"message": "m", "code": "400"})).await;
    assert_eq!(errs[0].1, provider(Some(400)));
    let errs = openai_error(json!({"message": "m", "code": "503"})).await;
    assert_eq!(errs[0].1, provider(Some(503)));
}

#[tokio::test]
async fn test_e3_openai_codes_out_of_range_give_no_status() {
    for code in [
        json!(42),
        json!(99),
        json!(600),
        json!("42"),
        json!("0"),
        json!(-5),
    ] {
        let errs = openai_error(json!({"message": "m", "code": code})).await;
        assert_eq!(errs[0].1, provider(None), "code {code}");
    }
}

#[tokio::test]
async fn test_e3_openrouter_named_code_maps_through_the_table() {
    let errs = openai_error(
        json!({"message": "Provider disconnected unexpectedly", "code": "server_error"}),
    )
    .await;
    assert_eq!(
        errs,
        vec![(
            "Provider disconnected unexpectedly".to_owned(),
            provider(Some(500))
        )]
    );
}

#[tokio::test]
async fn test_e4_every_name_table_entry_works_as_an_openai_code_and_as_an_error_type() {
    for (name, status) in NAME_TABLE {
        let as_code = openai_error(json!({"message": "m", "code": name})).await;
        assert_eq!(as_code[0].1, provider(Some(status)), "code {name}");
        let as_type = openai_error(json!({"message": "m", "type": name})).await;
        assert_eq!(as_type[0].1, provider(Some(status)), "type {name}");
    }
}

#[tokio::test]
async fn test_e3_openai_rule_order_integer_code_then_name_code_then_type() {
    // integer code beats the type
    let errs = openai_error(json!({"message": "m", "code": 400, "type": "server_error"})).await;
    assert_eq!(errs[0].1, provider(Some(400)));
    // a digit-string code beats the type
    let errs = openai_error(json!({"message": "m", "code": "401", "type": "server_error"})).await;
    assert_eq!(errs[0].1, provider(Some(401)));
    // a named code beats the type
    let errs =
        openai_error(json!({"message": "m", "code": "api_error", "type": "invalid_request_error"}))
            .await;
    assert_eq!(errs[0].1, provider(Some(500)));
    // an unknown code falls through to a known type
    let errs =
        openai_error(json!({"message": "m", "code": "mystery", "type": "rate_limit_error"})).await;
    assert_eq!(errs[0].1, provider(Some(429)));
}

#[tokio::test]
async fn test_e3_unknown_and_case_different_names_give_no_status() {
    for name in [
        "mystery_error",
        "Server_Error",
        "SERVER_ERROR",
        "overloaded",
        "",
    ] {
        let errs = openai_error(json!({"message": "m", "code": name})).await;
        assert_eq!(errs[0].1, provider(None), "code {name:?}");
        let errs = openai_error(json!({"message": "m", "type": name})).await;
        assert_eq!(errs[0].1, provider(None), "type {name:?}");
    }
}

#[tokio::test]
async fn test_e3_openai_plain_string_error_and_finish_reason_error_give_no_status() {
    let body = data(json!({"error": "boom"}));
    let events = run(Adapter::OpenAi, vec![piece(body)], End::Normal, None).await;
    assert_eq!(
        error_events(&events),
        vec![("boom".to_owned(), provider(None))]
    );

    let body = data(json!({"choices":[{"index":0,"delta":{},"finish_reason":"error"}]}));
    let events = run(Adapter::OpenAi, vec![piece(body)], End::Normal, None).await;
    let errs = error_events(&events);
    assert_eq!(errs.len(), 1, "{events:?}");
    assert_eq!(errs[0].1, provider(None));
}

#[tokio::test]
async fn test_e3_openrouter_shape_with_finish_reason_error_and_a_code() {
    let body = data(json!({
        "choices": [{"index": 0, "delta": {"content": ""}, "finish_reason": "error"}],
        "error": {"message": "Provider disconnected unexpectedly", "code": 502}
    }));
    let events = run(
        Adapter::OpenAi,
        vec![piece(body), piece("data: [DONE]\n\n")],
        End::Normal,
        None,
    )
    .await;
    assert_eq!(
        error_events(&events),
        vec![(
            "Provider disconnected unexpectedly".to_owned(),
            provider(Some(502))
        )]
    );
}

async fn anthropic_error(error: Value) -> Vec<(String, StreamErrorKind)> {
    let body = format!(
        "{}event: error\ndata: {}\n\n",
        head(Adapter::Anthropic),
        json!({"type": "error", "error": error})
    );
    error_events(&run(Adapter::Anthropic, vec![piece(body)], End::Normal, None).await)
}

#[tokio::test]
async fn test_e3_e4_anthropic_error_type_goes_through_the_name_table() {
    let errs = anthropic_error(json!({"type": "overloaded_error", "message": "Overloaded"})).await;
    assert_eq!(errs, vec![("Overloaded".to_owned(), provider(Some(529)))]);
    let errs = anthropic_error(json!({"type": "invalid_request_error", "message": "bad"})).await;
    assert_eq!(errs, vec![("bad".to_owned(), provider(Some(400)))]);
    for (name, status) in NAME_TABLE {
        let errs = anthropic_error(json!({"type": name, "message": "m"})).await;
        assert_eq!(errs[0].1, provider(Some(status)), "type {name}");
    }
}

#[tokio::test]
async fn test_e3_anthropic_unknown_or_missing_type_gives_no_status() {
    for error in [
        json!({"type": "mystery_error", "message": "m"}),
        json!({"message": "m"}),
        json!({"type": "Overloaded_Error", "message": "m"}),
    ] {
        let errs = anthropic_error(error.clone()).await;
        assert_eq!(errs[0].1, provider(None), "{error}");
    }
}

#[tokio::test]
async fn test_e3_ollama_error_line_gives_no_status() {
    let body = format!("{}\n", json!({"error": "model 'x' not found"}));
    let events = run(Adapter::Ollama, vec![piece(body)], End::Normal, None).await;
    assert_eq!(
        error_events(&events),
        vec![("model 'x' not found".to_owned(), provider(None))]
    );
}
