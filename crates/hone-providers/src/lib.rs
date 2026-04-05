pub mod adapters;
pub mod error;
pub mod fallback;
pub mod mock;
pub mod registry;
pub mod router;
pub mod traits;

pub use error::ProviderError;
pub use fallback::FallbackChain;
pub use mock::MockProvider;
pub use registry::ProviderRegistry;
pub use router::{ModelRouter, RoutingRule};
pub use traits::{CompletionRequest, ModelProvider};
