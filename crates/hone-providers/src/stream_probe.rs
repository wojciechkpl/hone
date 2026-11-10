//! Reading a provider stream up to its first output, so a failure *before any
//! output* can be retried or handed to another provider without the caller
//! having seen anything.
//!
//! An *output event* is any [`ProviderEvent`] other than `Usage` and `Error`.
//! A *failure before output* is a stream whose first non-`Usage` event is an
//! `Error`.

use std::pin::Pin;

use futures::{Stream, StreamExt, stream};
use hone_proto::ProviderEvent;

use crate::error::ProviderError;
use crate::traits::{CompletionRequest, ModelProvider};

pub type EventStream = Pin<Box<dyn Stream<Item = ProviderEvent> + Send>>;

/// A response that failed before producing any output.
#[derive(Debug, Clone, PartialEq)]
pub struct StreamFailure {
    /// `Usage` events the attempt reported before failing (input tokens are
    /// reported at the start of some providers' streams).
    pub usage: Vec<ProviderEvent>,
    /// The provider's error message, unchanged.
    pub message: String,
    /// What kind of failure it was; decides whether another attempt is worthwhile.
    pub kind: hone_proto::StreamErrorKind,
}

impl StreamFailure {
    /// The failure as the stream the provider produced: its usage, then its `Error`.
    pub fn into_events(self) -> Vec<ProviderEvent> {
        let mut events = self.usage;
        events.push(ProviderEvent::Error {
            message: self.message,
            kind: self.kind,
        });
        events
    }
}

/// Result of waiting for the first output of a stream.
pub enum Probe {
    /// Output arrived (or the stream ended without an error): the stream,
    /// with the events read so far in front. Everything after the first
    /// output is passed on live, not buffered.
    Stream(EventStream),
    /// The first non-`Usage` event was an `Error`.
    Failed(StreamFailure),
}

/// Wait for the first event that is not `Usage`.
pub async fn probe_first_output(mut stream: EventStream) -> Probe {
    let mut head: Vec<ProviderEvent> = Vec::new();
    while let Some(event) = stream.next().await {
        match event {
            ProviderEvent::Usage(_) => head.push(event),
            ProviderEvent::Error { message, kind } => {
                return Probe::Failed(StreamFailure {
                    usage: head,
                    message,
                    kind,
                });
            }
            output => {
                head.push(output);
                return Probe::Stream(Box::pin(stream::iter(head).chain(stream)));
            }
        }
    }
    // No output and no error: pass on what there was (usage only, or nothing).
    Probe::Stream(Box::pin(stream::iter(head)))
}

/// One attempt at a provider, as seen by a retrying caller.
pub enum Attempt {
    /// A stream to hand to the caller: output arrived, the provider runs its
    /// own tools (never waited on), or the request was cancelled.
    Stream(EventStream),
    /// The stream failed before output.
    Failed(StreamFailure),
    /// The request itself failed.
    Request(ProviderError),
}

/// Call `provider` and wait for its first output.
///
/// A provider that runs its own tools may already have acted before its first
/// event, so its stream is returned as is. A failure of a cancelled request is
/// passed on as is too: cancellation is never retried.
pub async fn attempt(provider: &dyn ModelProvider, request: CompletionRequest) -> Attempt {
    let cancelled = request.cancel.clone();
    let stream = match provider.complete(request).await {
        Ok(s) => s,
        Err(e) => return Attempt::Request(e),
    };
    if provider.runs_own_tools() {
        return Attempt::Stream(stream);
    }
    match probe_first_output(stream).await {
        Probe::Stream(s) => Attempt::Stream(s),
        Probe::Failed(f) => {
            if cancelled.is_some_and(|t| t.is_cancelled()) || !f.kind.is_retryable() {
                Attempt::Stream(Box::pin(stream::iter(f.into_events())))
            } else {
                tracing::warn!(
                    provider = provider.id(),
                    message = %f.message,
                    "response failed before any output; trying again or falling back"
                );
                Attempt::Failed(f)
            }
        }
    }
}

/// Put `carried` events (usage of failed attempts) in front of `stream`.
pub(crate) fn with_carried(carried: Vec<ProviderEvent>, stream: EventStream) -> EventStream {
    if carried.is_empty() {
        stream
    } else {
        Box::pin(stream::iter(carried).chain(stream))
    }
}

/// How the last attempt of a fallback sequence failed.
pub(crate) enum LastFailure {
    Request(ProviderError),
    Stream(String, hone_proto::StreamErrorKind),
}

impl LastFailure {
    /// Report it the way that provider reported it: a request error as `Err`, a
    /// stream failure as a stream of the carried usage and the `Error`.
    pub(crate) fn report(
        self,
        mut carried: Vec<ProviderEvent>,
    ) -> Result<EventStream, ProviderError> {
        match self {
            LastFailure::Request(e) => Err(e),
            LastFailure::Stream(message, kind) => {
                carried.push(ProviderEvent::Error { message, kind });
                Ok(Box::pin(stream::iter(carried)))
            }
        }
    }
}

/// Whether a failed request should be handed to the next provider: a transient
/// error, or no answer at all (HTTP status 0: connection refused, DNS, TLS).
pub(crate) fn hands_over(e: &ProviderError) -> bool {
    e.is_transient() || matches!(e, ProviderError::Http { status: 0, .. })
}
