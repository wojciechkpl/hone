//! Dataset builder for local fine-tuning.
//!
//! Convert agent session outputs into a JSONL training dataset suitable
//! for LoRA fine-tuning toolchains (axolotl, unsloth, llama.cpp finetune).

mod builder;
mod example;
mod filter;
mod jsonl;

pub use builder::{BuilderError, DatasetBuilder};
pub use example::{ExampleMetadata, TrainingExample};
pub use filter::{dedup_by_prompt, filter_by_score, shuffle_with_seed};
pub use jsonl::{read_jsonl, write_jsonl, JsonlError};
