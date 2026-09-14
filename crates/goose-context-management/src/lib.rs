//! Conversation compaction: summarizing a message history down to a single
//! message so a conversation can continue past a model's context window.
//!
//! Three layers, smallest first:
//!
//! * [`summarize`] - given a model and messages, produce one summary message.
//! * [`compact`] - the trait-based API ([`CompactionInput`] /
//!   [`CompactionOutput`]) letting a caller read from and write back to its own
//!   conversation representation. Rust only.
//!
//! Cross-language (Python/Kotlin) access is exposed by `goose-sdk`, which
//! wraps this crate in its uniffi bindings.

pub mod format;
pub mod model;
pub mod provider;
pub mod structured;
pub mod summarize;
pub mod templates;

use anyhow::Result;
use goose_provider_types::conversation::message::Message;
use goose_provider_types::conversation::token_usage::ProviderUsage;

pub use format::format_message_for_compacting;
pub use model::{CompactionModel, ProviderModel, TokenEstimator};
pub use provider::CompactingProvider;
pub use structured::{FileActivity, StructuredSummary};
pub use summarize::{summarize, Summary};
pub use templates::Templates;

pub const DEFAULT_COMPACTION_THRESHOLD: f64 = 0.8;

/// Everything compaction reads from the caller's conversation.
pub trait CompactionInput {
    fn messages(&self) -> Vec<Message>;

    fn templates(&self) -> Templates {
        Templates::default()
    }
}

/// Where compaction writes its result back into the caller's conversation.
pub trait CompactionOutput {
    fn set_summary(&mut self, summary: Message);
    fn set_usage(&mut self, usage: ProviderUsage);
}

pub async fn compact<I, O>(
    model: &dyn CompactionModel,
    estimator: Option<&dyn TokenEstimator>,
    input: &I,
    output: &mut O,
) -> Result<()>
where
    I: CompactionInput + ?Sized,
    O: CompactionOutput + ?Sized,
{
    let templates = input.templates();
    let summary = summarize(model, estimator, &templates, &input.messages()).await?;
    output.set_summary(summary.message);
    output.set_usage(summary.usage);
    Ok(())
}

impl CompactionInput for Vec<Message> {
    fn messages(&self) -> Vec<Message> {
        self.clone()
    }
}
