use std::pin::Pin;
use std::task::{Context as TaskContext, Poll};

use futures::Stream;
use tokio::sync::mpsc;

use crate::types::AssistantMessageEvent;

/// A stream of assistant message events that can be awaited for the final result.
///
/// This wraps a `futures::Stream<Item = AssistantMessageEvent>` and provides
/// a `result()` method that collects all events and returns the final
/// `AssistantMessage`.
pub struct AssistantMessageEventStream {
    inner: Pin<Box<dyn Stream<Item = AssistantMessageEvent> + Send>>,
}

impl AssistantMessageEventStream {
    /// Create a new event stream from a futures Stream.
    pub fn new<S>(stream: S) -> Self
    where
        S: Stream<Item = AssistantMessageEvent> + Send + 'static,
    {
        Self {
            inner: Box::pin(stream),
        }
    }

    /// Create an event stream from a channel receiver.
    pub fn from_receiver(rx: mpsc::UnboundedReceiver<AssistantMessageEvent>) -> Self {
        Self::new(tokio_stream::wrappers::UnboundedReceiverStream::new(rx))
    }

    /// Collect all events and return the final assistant message.
    ///
    /// This consumes the stream and returns the message from the final
    /// `Done` or `Error` event.
    pub async fn result(mut self) -> Result<crate::types::AssistantMessage, String> {
        use futures::StreamExt;

        let mut final_message: Option<crate::types::AssistantMessage> = None;

        while let Some(event) = self.inner.next().await {
            match event {
                AssistantMessageEvent::Done { message, .. } => {
                    final_message = Some(message);
                }
                AssistantMessageEvent::Error { error, .. } => {
                    final_message = Some(error);
                }
                _ => {}
            }
        }

        final_message.ok_or_else(|| "Stream ended without a final event".to_string())
    }
}

impl Stream for AssistantMessageEventStream {
    type Item = AssistantMessageEvent;

    fn poll_next(mut self: Pin<&mut Self>, cx: &mut TaskContext<'_>) -> Poll<Option<Self::Item>> {
        Pin::new(&mut self.inner).poll_next(cx)
    }
}
