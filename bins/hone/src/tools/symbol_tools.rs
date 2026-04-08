//! Symbol search tools backed by the SqliteIndex.
//!
//! These tools are registered locally in the main binary (not in hone-tools)
//! to avoid a circular dependency between hone-tools and hone-index.

use std::sync::Arc;

use async_trait::async_trait;
use hone_index::SqliteIndex;
use hone_proto::ToolLayer;
use hone_tools::{Tool, ToolContext, ToolError, ToolInput, ToolOutput};
use serde_json::{Value, json};

/// Tool that queries the persistent symbol index for symbols matching a name.
pub struct SymbolSearchTool {
    index: Arc<SqliteIndex>,
}

impl SymbolSearchTool {
    pub fn new(index: Arc<SqliteIndex>) -> Self {
        Self { index }
    }
}

#[async_trait]
impl Tool for SymbolSearchTool {
    fn name(&self) -> &str {
        "symbol_search"
    }

    fn description(&self) -> &str {
        "Search the project's symbol index for functions, structs, classes, etc. \
         matching a name. Returns up to `limit` results sorted by relevance (exact \
         match > prefix match > substring match). Use this BEFORE grepping when you \
         need to find a named entity."
    }

    fn schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "query": {
                    "type": "string",
                    "description": "Symbol name to search for (case-insensitive)."
                },
                "limit": {
                    "type": "integer",
                    "description": "Maximum number of results. Default: 20.",
                    "minimum": 1,
                    "maximum": 100
                }
            },
            "required": ["query"]
        })
    }

    async fn call(&self, input: ToolInput, _ctx: &ToolContext) -> Result<ToolOutput, ToolError> {
        let query = input.get_str("query").ok_or_else(|| ToolError::InvalidInput {
            reason: "missing required field 'query'".to_string(),
        })?;

        let limit = input.get_u64("limit").unwrap_or(20) as usize;

        let results = self
            .index
            .search(query, limit)
            .await
            .map_err(|e| ToolError::Other(format!("symbol search failed: {e}")))?;

        let json_results: Vec<Value> = results
            .into_iter()
            .map(|result| {
                json!({
                    "name": result.symbol.name,
                    "kind": format!("{:?}", result.symbol.kind).to_lowercase(),
                    "file": result.symbol.file.to_string_lossy(),
                    "line": result.symbol.line,
                    "col": result.symbol.col,
                    "score": result.score,
                })
            })
            .collect();

        Ok(ToolOutput::new(json!({
            "query": query,
            "count": json_results.len(),
            "results": json_results,
        })))
    }

    fn layer(&self) -> ToolLayer {
        ToolLayer::Native
    }
}

/// Tool that finds the symbol enclosing a specific file/line position.
///
/// Useful for expanding context around a known location (e.g., "what function
/// contains line 42 of auth.rs?").
pub struct SymbolAtTool {
    index: Arc<SqliteIndex>,
}

impl SymbolAtTool {
    pub fn new(index: Arc<SqliteIndex>) -> Self {
        Self { index }
    }
}

#[async_trait]
impl Tool for SymbolAtTool {
    fn name(&self) -> &str {
        "symbol_at"
    }

    fn description(&self) -> &str {
        "Find the symbol (function, struct, etc.) at a specific file/line position. \
         Returns metadata about the enclosing symbol, or null if no symbol is found \
         at that location. Use this to discover which function contains a specific line \
         of code without reading the entire file."
    }

    fn schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "file": {
                    "type": "string",
                    "description": "Path to the file (relative to working directory)."
                },
                "line": {
                    "type": "integer",
                    "description": "1-indexed line number.",
                    "minimum": 1
                },
                "col": {
                    "type": "integer",
                    "description": "1-indexed column number. Default: 1.",
                    "minimum": 1
                }
            },
            "required": ["file", "line"]
        })
    }

    async fn call(&self, input: ToolInput, ctx: &ToolContext) -> Result<ToolOutput, ToolError> {
        let file_str = input.get_str("file").ok_or_else(|| ToolError::InvalidInput {
            reason: "missing required field 'file'".to_string(),
        })?;

        let line = input.get_u64("line").ok_or_else(|| ToolError::InvalidInput {
            reason: "missing required field 'line'".to_string(),
        })? as u32;

        let col = input.get_u64("col").unwrap_or(1) as u32;

        let file_path = ctx.working_dir.join(file_str);

        let symbol = self
            .index
            .symbol_at(&file_path, line, col)
            .await
            .map_err(|e| ToolError::Other(format!("symbol_at failed: {e}")))?;

        match symbol {
            Some(sym) => Ok(ToolOutput::new(json!({
                "found": true,
                "name": sym.name,
                "kind": format!("{:?}", sym.kind).to_lowercase(),
                "file": sym.file.to_string_lossy(),
                "line": sym.line,
                "col": sym.col,
            }))),
            None => Ok(ToolOutput::new(json!({
                "found": false,
                "file": file_str,
                "line": line,
            }))),
        }
    }

    fn layer(&self) -> ToolLayer {
        ToolLayer::Native
    }
}
