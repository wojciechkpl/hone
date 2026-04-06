use hone_core::{ComplexityClassifier, ComplexityTier};

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[test]
fn test_simple_classification() {
    let tier = ComplexityClassifier::classify("fix typo in main.rs");
    assert_eq!(tier, ComplexityTier::Simple);
}

#[test]
fn test_complex_classification() {
    let tier = ComplexityClassifier::classify(
        "refactor the authentication system across multiple modules",
    );
    assert_eq!(tier, ComplexityTier::Complex);
}

#[test]
fn test_medium_classification() {
    let tier = ComplexityClassifier::classify("add error handling to the parser");
    assert_eq!(tier, ComplexityTier::Medium);
}

/// A request under 100 characters with no complex keywords should be Simple.
#[test]
fn test_short_request_is_simple() {
    // 50-char request — well under the 100-char threshold.
    let request = "rename the `process` function to `handle`";
    assert!(request.chars().count() < 100, "request should be short");
    let tier = ComplexityClassifier::classify(request);
    assert_eq!(tier, ComplexityTier::Simple);
}

/// A long request (> 300 chars) that mentions multiple files should be Complex.
#[test]
fn test_long_request_is_complex() {
    let request = concat!(
        "Please update the user authentication flow. ",
        "The changes should span auth.rs, middleware.rs, routes.rs, database.rs and config.toml. ",
        "We need to switch from session-based tokens to JWTs, update all related tests, ",
        "ensure backward compatibility with the existing API consumers, ",
        "and document all the public interfaces that are affected by this change."
    );
    assert!(
        request.chars().count() > 300,
        "request should be longer than 300 chars (got {})",
        request.chars().count()
    );
    let tier = ComplexityClassifier::classify(request);
    assert_eq!(tier, ComplexityTier::Complex);
}

/// "architecture" is an explicit complex keyword regardless of length.
#[test]
fn test_architecture_keyword_is_complex() {
    let tier = ComplexityClassifier::classify("review the architecture of the system");
    assert_eq!(tier, ComplexityTier::Complex);
}

/// A request that mentions 3+ file paths should be classified as Complex.
#[test]
fn test_three_file_references_is_complex() {
    let tier = ComplexityClassifier::classify(
        "update the logic in parser.rs, lexer.rs, and ast.rs to handle unicode",
    );
    assert_eq!(tier, ComplexityTier::Complex);
}
