use std::pin::Pin;
use std::sync::Arc;

use async_trait::async_trait;
use futures::Stream;
use hone_proto::{Message, ModelId, ModelInfo, ProviderEvent, ProviderHealth, ToolResult, ToolSchema};
use tokio::sync::{Mutex, mpsc};
use tokio_util::sync::CancellationToken;

use crate::error::ProviderError;

// ---------------------------------------------------------------------------
// Bidirectional tool-result channel
// ---------------------------------------------------------------------------

/// Error returned when `ToolResultSender::send` fails (receiver dropped).
#[derive(Debug)]
pub struct ToolResultSendError(pub ToolResult);

impl std::fmt::Display for ToolResultSendError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "tool result channel closed (receiver dropped)")
    }
}

impl std::error::Error for ToolResultSendError {}

/// Receiver side: handed to `AcpProvider` via `CompletionRequest::tool_results`.
///
/// Wraps an `mpsc::Receiver` behind a `Mutex` so the type can be `Clone` and
/// therefore fit inside `CompletionRequest` (which derives `Clone`).  Only one
/// caller will actually poll `recv()` at a time (the provider loop), so the
/// mutex is uncontended in practice.
#[derive(Clone)]
pub struct ToolResultStream {
    inner: Arc<Mutex<mpsc::Receiver<ToolResult>>>,
}

/// Sender side: held by the dispatcher; clonable so multiple producers can post.
#[derive(Clone)]
pub struct ToolResultSender {
    inner: mpsc::Sender<ToolResult>,
}

impl ToolResultStream {
    /// Create a matched `(ToolResultStream, ToolResultSender)` pair backed by
    /// a channel with a buffer of 16 slots.
    pub fn new() -> (Self, ToolResultSender) {
        let (tx, rx) = mpsc::channel(16);
        let stream = Self {
            inner: Arc::new(Mutex::new(rx)),
        };
        let sender = ToolResultSender { inner: tx };
        (stream, sender)
    }

    /// Receive the next `ToolResult`.  Returns `None` when all senders have
    /// been dropped and the channel is empty.
    pub async fn recv(&self) -> Option<ToolResult> {
        self.inner.lock().await.recv().await
    }
}

impl ToolResultSender {
    /// Send a `ToolResult` to the provider.
    ///
    /// Returns `Err(ToolResultSendError)` when the receiver has been dropped.
    pub async fn send(&self, r: ToolResult) -> Result<(), ToolResultSendError> {
        self.inner.send(r).await.map_err(|e| ToolResultSendError(e.0))
    }
}

// ---------------------------------------------------------------------------
// CompletionRequest
// ---------------------------------------------------------------------------

/// A request to complete a conversation turn with a model.
#[derive(Clone)]
pub struct CompletionRequest {
    pub model: ModelId,
    pub messages: Vec<Message>,
    pub tools: Vec<ToolSchema>,
    pub temperature: Option<f32>,
    pub max_tokens: Option<u32>,
    pub stop: Vec<String>,
    /// Optional cancellation handle. When `Some` and the token is cancelled,
    /// the provider must terminate the turn promptly: emit
    /// `ProviderEvent::Error` followed by `ProviderEvent::Done`, end the
    /// stream, and release any held resources.  When `None`, no cancellation
    /// support is wired up (legacy behavior).
    pub cancel: Option<CancellationToken>,
    /// Optional bidirectional channel for tool-call round-trips (ACP only).
    ///
    /// When `Some`, the provider will pull `ToolResult` values from this
    /// stream whenever the agent emits a `tool_call` notification, forwarding
    /// them back to the agent as `session/toolResult` requests.  When `None`,
    /// no tool dispatch is wired up (legacy / non-ACP behavior).
    ///
    /// Not included in cache keys or `Debug` output.
    pub tool_results: Option<ToolResultStream>,
}

impl std::fmt::Debug for CompletionRequest {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("CompletionRequest")
            .field("model", &self.model)
            .field("messages", &self.messages)
            .field("tools", &self.tools)
            .field("temperature", &self.temperature)
            .field("max_tokens", &self.max_tokens)
            .field("stop", &self.stop)
            .field("cancel", &self.cancel)
            .field(
                "tool_results",
                &self.tool_results.as_ref().map(|_| "<ToolResultStream>"),
            )
            .finish()
    }
}

impl CompletionRequest {
    pub fn new(model: impl Into<ModelId>, messages: Vec<Message>) -> Self {
        Self {
            model: model.into(),
            messages,
            tools: Vec::new(),
            temperature: None,
            max_tokens: None,
            stop: Vec::new(),
            cancel: None,
            tool_results: None,
        }
    }
}

/// A streaming model provider.
#[async_trait]
pub trait ModelProvider: Send + Sync + 'static {
    /// Stable identifier for this provider (e.g. `"anthropic"`, `"openai"`).
    fn id(&self) -> &str;

    /// Human-readable name for this provider.
    fn display_name(&self) -> &str;

    /// All models this provider exposes.
    fn supported_models(&self) -> Vec<ModelInfo>;

    /// Begin a streaming completion. Returns a pinned stream of `ProviderEvent`s.
    async fn complete(
        &self,
        request: CompletionRequest,
    ) -> Result<Pin<Box<dyn Stream<Item = ProviderEvent> + Send>>, ProviderError>;

    /// Rough token count heuristic used for cost estimation / context trimming.
    fn estimate_tokens(&self, messages: &[Message]) -> u32;

    /// Current health of this provider.
    async fn health(&self) -> ProviderHealth;
}
