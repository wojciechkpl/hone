//! Spec-driven planning: generate requirements + design + tasks docs from
//! a goal, then execute the tasks.

pub mod generator;
pub mod model;
pub mod parse;
pub mod render;
pub mod store;

pub use generator::{DefaultSpecGenerator, GenerateError, SpecGenerator};
pub use model::{AcceptanceCriterion, Spec, Task, TaskStatus};
pub use parse::parse_tasks_md;
pub use render::render_tasks_md;
pub use store::{SpecStore, SpecStoreError};
