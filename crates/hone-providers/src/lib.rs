pub mod adapters;
pub mod error;
pub mod fallback;
pub mod mock;
pub mod pricing;
pub mod registry;
pub mod router;
pub mod traits;

pub use adapters::claude_cli::ClaudeCliProvider;
pub use adapters::gemini_cli::GeminiCliProvider;
pub use error::ProviderError;
pub use fallback::FallbackChain;
pub use mock::MockProvider;
pub use pricing::PricingTable;
pub use registry::ProviderRegistry;
pub use router::{ModelRouter, RoutingRule};
pub use traits::{CompletionRequest, ModelProvider};
