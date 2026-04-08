use std::collections::HashMap;
use std::sync::Arc;

use hone_proto::ToolSchema;

use crate::traits::Tool;

/// An immutable, cheaply-cloneable registry of native tools.
///
/// Construct one via [`ToolRegistryBuilder`]; the finished registry is
/// read-only and can be freely cloned and shared across tasks.
#[derive(Clone, Default)]
pub struct ToolRegistry {
    tools: Arc<HashMap<String, Arc<dyn Tool>>>,
}

impl ToolRegistry {
    /// Create an empty registry.
    pub fn new() -> Self {
        Self::default()
    }

    /// Start building a registry with the fluent builder API.
    pub fn builder() -> ToolRegistryBuilder {
        ToolRegistryBuilder::default()
    }

    /// Look up a tool by name.
    pub fn get(&self, name: &str) -> Option<&Arc<dyn Tool>> {
        self.tools.get(name)
    }

    /// Return the JSON Schema descriptors for all registered tools.
    pub fn schemas(&self) -> Vec<ToolSchema> {
        self.tools
            .values()
            .map(|t| ToolSchema {
                name: t.name().to_string(),
                description: t.description().to_string(),
                parameters: t.schema(),
            })
            .collect()
    }

    /// Return the names of all registered tools.
    pub fn names(&self) -> Vec<&str> {
        self.tools.keys().map(String::as_str).collect()
    }

    /// Number of registered tools.
    pub fn len(&self) -> usize {
        self.tools.len()
    }

    /// Returns `true` if no tools are registered.
    pub fn is_empty(&self) -> bool {
        self.tools.is_empty()
    }
}

// ---------------------------------------------------------------------------
// Builder
// ---------------------------------------------------------------------------

/// Fluent builder for constructing a [`ToolRegistry`].
///
/// Register all tools up-front, then call [`build`](ToolRegistryBuilder::build)
/// to obtain an immutable, `Clone`-able registry.
#[derive(Default)]
pub struct ToolRegistryBuilder {
    tools: HashMap<String, Arc<dyn Tool>>,
}

impl ToolRegistryBuilder {
    /// Register a tool. An existing entry with the same name is replaced.
    pub fn register(mut self, tool: Box<dyn Tool>) -> Self {
        let name = tool.name().to_string();
        self.tools.insert(name, Arc::from(tool));
        self
    }

    /// Finalise the builder and return the immutable [`ToolRegistry`].
    pub fn build(self) -> ToolRegistry {
        ToolRegistry {
            tools: Arc::new(self.tools),
        }
    }
}
