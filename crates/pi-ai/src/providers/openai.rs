//! OpenAI Completions API provider (via rig-core).
//!
//! This is a thin wrapper that converts pi-ai types to rig-core's
//! `CompletionRequest` and converts the response back to pi-ai types.

use crate::types::{AssistantMessageEvent, Context, Model, StreamOptions};

/// Create a stream function for the OpenAI Completions API.
pub fn create_openai_stream(
    _model: &Model,
    _context: &Context,
    _options: Option<&StreamOptions>,
) -> Box<dyn futures::Stream<Item = AssistantMessageEvent> + Unpin + Send> {
    // TODO: Implement via rig-core openai client
    Box::new(futures::stream::empty())
}
