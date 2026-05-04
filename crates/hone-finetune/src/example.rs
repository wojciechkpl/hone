use std::collections::BTreeMap;

pub type ExampleMetadata = BTreeMap<String, String>;

#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct TrainingExample {
    pub prompt: String,
    pub response: String,
    /// Quality score in [0.0, 1.0] — higher = better. None = unscored.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub score: Option<f32>,
    /// Free-form key/value metadata (model id, session id, source, etc.).
    /// Use BTreeMap for deterministic JSON serialization.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub metadata: ExampleMetadata,
}

impl TrainingExample {
    pub fn new(prompt: impl Into<String>, response: impl Into<String>) -> Self {
        Self {
            prompt: prompt.into(),
            response: response.into(),
            score: None,
            metadata: BTreeMap::new(),
        }
    }

    pub fn with_score(mut self, score: f32) -> Self {
        self.score = Some(score);
        self
    }

    pub fn with_metadata(mut self, key: impl Into<String>, value: impl Into<String>) -> Self {
        self.metadata.insert(key.into(), value.into());
        self
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn make_example() -> TrainingExample {
        TrainingExample::new("What is 2+2?", "4")
    }

    #[test]
    fn new_default_score_and_metadata_are_none_and_empty() {
        let ex = make_example();
        assert_eq!(ex.prompt, "What is 2+2?");
        assert_eq!(ex.response, "4");
        assert!(ex.score.is_none());
        assert!(ex.metadata.is_empty());
    }

    #[test]
    fn with_score_sets_score() {
        let ex = make_example().with_score(0.9);
        assert_eq!(ex.score, Some(0.9));
    }

    #[test]
    fn with_metadata_appends_key_value() {
        let ex = make_example().with_metadata("model", "llama3");
        assert_eq!(ex.metadata.get("model").map(String::as_str), Some("llama3"));
    }

    #[test]
    fn with_metadata_called_twice_keeps_both() {
        let ex = make_example()
            .with_metadata("model", "llama3")
            .with_metadata("session", "abc-123");
        assert_eq!(ex.metadata.len(), 2);
        assert_eq!(ex.metadata.get("model").map(String::as_str), Some("llama3"));
        assert_eq!(
            ex.metadata.get("session").map(String::as_str),
            Some("abc-123")
        );
    }

    #[test]
    fn serde_roundtrip_full_example() {
        let ex = make_example()
            .with_score(0.75)
            .with_metadata("source", "session-42");
        let json = serde_json::to_string(&ex).expect("serialise");
        let back: TrainingExample = serde_json::from_str(&json).expect("deserialise");
        assert_eq!(ex, back);
    }

    #[test]
    fn serde_omits_none_score() {
        let ex = make_example();
        let json = serde_json::to_string(&ex).expect("serialise");
        assert!(!json.contains("score"), "score field must be absent when None");
    }

    #[test]
    fn serde_omits_empty_metadata() {
        let ex = make_example();
        let json = serde_json::to_string(&ex).expect("serialise");
        assert!(
            !json.contains("metadata"),
            "metadata field must be absent when empty"
        );
    }

    #[test]
    fn serde_metadata_is_deterministic_btreemap() {
        // Insert keys in different orders; JSON output must be identical because
        // BTreeMap serialises in sorted key order.
        let ex_a = make_example()
            .with_metadata("z_key", "last")
            .with_metadata("a_key", "first");
        let ex_b = make_example()
            .with_metadata("a_key", "first")
            .with_metadata("z_key", "last");
        let json_a = serde_json::to_string(&ex_a).expect("serialise a");
        let json_b = serde_json::to_string(&ex_b).expect("serialise b");
        assert_eq!(json_a, json_b);
    }
}
