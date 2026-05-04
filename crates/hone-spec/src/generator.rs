use std::sync::Arc;

use async_trait::async_trait;
use futures::StreamExt;
use hone_proto::{Message, ModelId};
use hone_providers::ModelProvider;
use hone_providers::traits::CompletionRequest;

use crate::model::Spec;
use crate::parse::parse_tasks_md;

#[async_trait]
pub trait SpecGenerator: Send + Sync {
    /// Generate a Spec from a free-form goal string.
    async fn generate(&self, goal: &str) -> Result<Spec, GenerateError>;
}

/// Default generator that drives a ModelProvider with three structured
/// prompts (one per document). Single-pass, sequential.
pub struct DefaultSpecGenerator {
    provider: Arc<dyn ModelProvider>,
    model: ModelId,
}

impl DefaultSpecGenerator {
    pub fn new(provider: Arc<dyn ModelProvider>, model: ModelId) -> Self {
        Self { provider, model }
    }

    /// Slugify a goal string using the same algorithm as plan_mode::slugify:
    /// lower, non-alphanum -> -, collapse runs, trim, max 40, fallback "spec".
    pub fn slugify_goal(goal: &str) -> String {
        let lower = goal.to_lowercase();
        // Replace non-alphanumeric chars with hyphens
        let mut slug = String::with_capacity(lower.len());
        let mut prev_hyphen = false;
        for ch in lower.chars() {
            if ch.is_ascii_alphanumeric() {
                slug.push(ch);
                prev_hyphen = false;
            } else if !prev_hyphen {
                slug.push('-');
                prev_hyphen = true;
            }
        }
        // Trim leading/trailing hyphens
        let slug = slug.trim_matches('-');
        // Truncate to 40 chars, avoid cutting mid-hyphen-run
        let slug = if slug.len() > 40 {
            let truncated = &slug[..40];
            truncated.trim_end_matches('-')
        } else {
            slug
        };
        if slug.is_empty() {
            "spec".to_owned()
        } else {
            slug.to_owned()
        }
    }

    async fn call_provider(&self, prompt: &str) -> Result<String, GenerateError> {
        let messages = vec![Message::user(prompt)];
        let request = CompletionRequest::new(self.model.clone(), messages);
        let mut stream = self
            .provider
            .complete(request)
            .await
            .map_err(|e| GenerateError::Provider(e.to_string()))?;
        let mut text = String::new();
        while let Some(event) = stream.next().await {
            if let hone_proto::ProviderEvent::Token { text: chunk } = event {
                text.push_str(&chunk);
            }
        }
        Ok(text)
    }
}

#[async_trait]
impl SpecGenerator for DefaultSpecGenerator {
    async fn generate(&self, goal: &str) -> Result<Spec, GenerateError> {
        if goal.trim().is_empty() {
            return Err(GenerateError::EmptyGoal);
        }

        let slug = Self::slugify_goal(goal);

        // Call 1: requirements
        let requirements_md = self
            .call_provider(&format!(
                "Generate a requirements.md document for: {}. \
                 Include user stories and acceptance criteria. Output ONLY the markdown.",
                goal
            ))
            .await?;
        if requirements_md.trim().is_empty() {
            return Err(GenerateError::EmptySection {
                section: "requirements".to_owned(),
            });
        }

        // Call 2: design
        let design_md = self
            .call_provider(&format!(
                "Given these requirements: {}. Generate a design.md document covering \
                 architecture and components. Output ONLY the markdown.",
                requirements_md
            ))
            .await?;
        if design_md.trim().is_empty() {
            return Err(GenerateError::EmptySection {
                section: "design".to_owned(),
            });
        }

        // Call 3: tasks
        let tasks_md = self
            .call_provider(&format!(
                "Given these requirements and design, generate a tasks.md document with \
                 discrete work items in the format `## T001 — Title` etc. Output ONLY the markdown.\n\
                 Requirements: {}\nDesign: {}",
                requirements_md, design_md
            ))
            .await?;
        if tasks_md.trim().is_empty() {
            return Err(GenerateError::EmptySection {
                section: "tasks".to_owned(),
            });
        }

        let tasks = parse_tasks_md(&tasks_md);

        Ok(Spec {
            slug,
            goal: goal.to_owned(),
            requirements_md,
            design_md,
            tasks,
        })
    }
}

#[derive(Debug, thiserror::Error)]
pub enum GenerateError {
    #[error("provider error: {0}")]
    Provider(String),
    #[error("empty goal")]
    EmptyGoal,
    #[error("model returned empty {section} document")]
    EmptySection { section: String },
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
#[allow(
    clippy::expect_used,
    clippy::unwrap_used,
    clippy::panic,
    clippy::todo
)]
mod tests {
    use super::*;

    use std::pin::Pin;
    use std::sync::{Arc, Mutex};

    use async_trait::async_trait;
    use futures::{Stream, stream};
    use hone_proto::{Message, ModelId, ModelInfo, ProviderEvent, ProviderHealth};
    use hone_providers::{
        error::ProviderError,
        traits::{CompletionRequest, ModelProvider},
    };

    // ------------------------------------------------------------------------
    // RecordingMockProvider
    //
    // An inline provider that records every CompletionRequest it receives and
    // returns a canned text response. Used to assert the generator calls the
    // provider exactly three times (requirements, design, tasks).
    // ------------------------------------------------------------------------

    struct RecordingMockProvider {
        /// Text returned as a single Token event for every complete() call.
        response_text: String,
        /// Grows by one per complete() call.
        call_count: Arc<Mutex<usize>>,
        /// All requests captured in order.
        recorded_requests: Arc<Mutex<Vec<CompletionRequest>>>,
    }

    impl RecordingMockProvider {
        fn new(response_text: impl Into<String>) -> Self {
            Self {
                response_text: response_text.into(),
                call_count: Arc::new(Mutex::new(0)),
                recorded_requests: Arc::new(Mutex::new(vec![])),
            }
        }

        fn call_count(&self) -> usize {
            *self.call_count.lock().unwrap()
        }
    }

    #[async_trait]
    impl ModelProvider for RecordingMockProvider {
        fn id(&self) -> &str {
            "recording-mock"
        }

        fn display_name(&self) -> &str {
            "Recording Mock Provider"
        }

        fn supported_models(&self) -> Vec<ModelInfo> {
            vec![ModelInfo {
                id: ModelId::new("mock:test-model"),
                display_name: "Test Model".to_owned(),
                context_length: 4096,
                supports_tools: false,
                supports_streaming: true,
            }]
        }

        async fn complete(
            &self,
            request: CompletionRequest,
        ) -> Result<Pin<Box<dyn Stream<Item = ProviderEvent> + Send>>, ProviderError> {
            *self.call_count.lock().unwrap() += 1;
            self.recorded_requests.lock().unwrap().push(request);
            let events = vec![
                ProviderEvent::Token {
                    text: self.response_text.clone(),
                },
                ProviderEvent::Done,
            ];
            Ok(Box::pin(stream::iter(events)))
        }

        fn estimate_tokens(&self, _messages: &[Message]) -> u32 {
            1
        }

        async fn health(&self) -> ProviderHealth {
            ProviderHealth::Healthy
        }
    }

    // ------------------------------------------------------------------------
    // slugify_goal tests
    // ------------------------------------------------------------------------

    #[test]
    fn slugify_goal_simple_phrase() {
        let slug = DefaultSpecGenerator::slugify_goal("Build user auth");
        assert_eq!(slug, "build-user-auth");
    }

    #[test]
    fn slugify_goal_empty_returns_spec_fallback() {
        let slug = DefaultSpecGenerator::slugify_goal("");
        assert_eq!(slug, "spec");
    }

    #[test]
    fn slugify_goal_truncates_at_40_chars() {
        // 50-char input — result must be ≤ 40 chars
        let long_goal = "a".repeat(50);
        let slug = DefaultSpecGenerator::slugify_goal(&long_goal);
        assert!(
            slug.len() <= 40,
            "slug '{}' is {} chars, expected ≤ 40",
            slug,
            slug.len()
        );
    }

    // ------------------------------------------------------------------------
    // DefaultSpecGenerator behaviour tests
    // ------------------------------------------------------------------------

    #[tokio::test]
    async fn default_generator_calls_provider_three_times() {
        let provider = Arc::new(RecordingMockProvider::new(
            "# Requirements\n\nSome content here.",
        ));
        let call_count_ref = Arc::clone(&provider.call_count);
        let model = ModelId::new("mock:test-model");
        let generator = DefaultSpecGenerator::new(provider, model);

        generator.generate("Build user auth").await.unwrap();

        assert_eq!(
            *call_count_ref.lock().unwrap(),
            3,
            "expected exactly 3 provider calls (requirements, design, tasks)"
        );
    }

    #[tokio::test]
    async fn default_generator_returns_spec_with_all_three_docs_populated() {
        let provider = Arc::new(RecordingMockProvider::new("Non-empty document content."));
        let model = ModelId::new("mock:test-model");
        let generator = DefaultSpecGenerator::new(provider, model);

        let spec = generator.generate("Build user auth").await.unwrap();

        assert!(!spec.requirements_md.is_empty(), "requirements_md is empty");
        assert!(!spec.design_md.is_empty(), "design_md is empty");
        // tasks come from parsing provider output — at minimum the field exists
        // (may be empty if the canned text has no T### headings, but the Spec is populated)
        assert_eq!(spec.goal, "Build user auth");
        assert!(!spec.slug.is_empty(), "slug is empty");
    }

    #[tokio::test]
    async fn default_generator_empty_goal_returns_error() {
        let provider = Arc::new(RecordingMockProvider::new("anything"));
        let model = ModelId::new("mock:test-model");
        let generator = DefaultSpecGenerator::new(provider, model);

        let result = generator.generate("").await;

        assert!(
            matches!(result, Err(GenerateError::EmptyGoal)),
            "expected EmptyGoal, got: {:?}",
            result
        );
    }

    #[tokio::test]
    async fn default_generator_empty_section_response_returns_error() {
        // The provider returns an empty string — generator must surface EmptySection
        let provider = Arc::new(RecordingMockProvider::new(""));
        let model = ModelId::new("mock:test-model");
        let generator = DefaultSpecGenerator::new(provider, model);

        let result = generator.generate("Build something").await;

        assert!(
            matches!(result, Err(GenerateError::EmptySection { .. })),
            "expected EmptySection, got: {:?}",
            result
        );
    }
}
