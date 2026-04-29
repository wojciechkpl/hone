use std::path::Path;

use sqlx::Row;

use crate::error::IndexError;
use crate::parser::SymbolExtractor;
use crate::types::{SearchResult, Symbol, SymbolKind};

/// Scoring constants — higher is better (mirrors SemanticIndex).
const SCORE_EXACT: f64 = 1.0;
const SCORE_PREFIX: f64 = 0.8;
const SCORE_SUBSTRING: f64 = 0.5;

/// SQLite-backed persistent symbol index.
pub struct SqliteIndex {
    pool: sqlx::SqlitePool,
}

impl SqliteIndex {
    /// Open (or create) a persistent SQLite database at `path`.
    pub async fn open(path: &Path) -> Result<Self, IndexError> {
        let url = format!(
            "sqlite://{}?mode=rwc",
            path.to_str()
                .ok_or_else(|| std::io::Error::other("non-UTF-8 path"))?
        );
        let pool = sqlx::SqlitePool::connect(&url)
            .await
            .map_err(|e| std::io::Error::other(e.to_string()))?;
        let idx = Self { pool };
        idx.migrate().await?;
        Ok(idx)
    }

    /// Create an in-memory SQLite database (useful for tests).
    pub async fn in_memory() -> Result<Self, IndexError> {
        let pool = sqlx::SqlitePool::connect("sqlite::memory:")
            .await
            .map_err(|e| std::io::Error::other(e.to_string()))?;
        let idx = Self { pool };
        idx.migrate().await?;
        Ok(idx)
    }

    /// Apply the schema migrations (idempotent).
    async fn migrate(&self) -> Result<(), IndexError> {
        sqlx::query(
            "CREATE TABLE IF NOT EXISTS index_files (
                path       TEXT PRIMARY KEY,
                indexed_at INTEGER NOT NULL
            )",
        )
        .execute(&self.pool)
        .await
        .map_err(|e| std::io::Error::other(e.to_string()))?;

        sqlx::query(
            "CREATE TABLE IF NOT EXISTS index_symbols (
                id        INTEGER PRIMARY KEY AUTOINCREMENT,
                file_path TEXT    NOT NULL REFERENCES index_files(path) ON DELETE CASCADE,
                name      TEXT    NOT NULL,
                kind      TEXT    NOT NULL,
                line      INTEGER NOT NULL,
                col       INTEGER NOT NULL
            )",
        )
        .execute(&self.pool)
        .await
        .map_err(|e| std::io::Error::other(e.to_string()))?;

        sqlx::query("CREATE INDEX IF NOT EXISTS idx_symbols_file ON index_symbols(file_path)")
            .execute(&self.pool)
            .await
            .map_err(|e| std::io::Error::other(e.to_string()))?;

        sqlx::query(
            "CREATE INDEX IF NOT EXISTS idx_symbols_name ON index_symbols(name COLLATE NOCASE)",
        )
        .execute(&self.pool)
        .await
        .map_err(|e| std::io::Error::other(e.to_string()))?;

        // Enable cascade deletes via foreign key enforcement.
        sqlx::query("PRAGMA foreign_keys = ON")
            .execute(&self.pool)
            .await
            .map_err(|e| std::io::Error::other(e.to_string()))?;

        Ok(())
    }

    /// Index a file: extract symbols and store them, replacing any previous
    /// data for this path.
    pub async fn index_file(&self, path: &Path, content: &str) -> Result<(), IndexError> {
        let path_str = path
            .to_str()
            .ok_or_else(|| std::io::Error::other("non-UTF-8 path"))?;

        let mut extractor = SymbolExtractor::new();
        let symbols = extractor.extract(path, content);

        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs() as i64;

        // Enable foreign key enforcement for this connection.  SQLite requires
        // this per-connection; with a pool we set it before each write txn.
        let mut tx = self
            .pool
            .begin()
            .await
            .map_err(|e| std::io::Error::other(e.to_string()))?;

        sqlx::query("PRAGMA foreign_keys = ON")
            .execute(&mut *tx)
            .await
            .map_err(|e| std::io::Error::other(e.to_string()))?;

        // Remove old symbols (cascade) and re-insert the file record.
        sqlx::query("DELETE FROM index_files WHERE path = ?")
            .bind(path_str)
            .execute(&mut *tx)
            .await
            .map_err(|e| std::io::Error::other(e.to_string()))?;

        sqlx::query(
            "INSERT INTO index_files (path, indexed_at) VALUES (?, ?)
             ON CONFLICT(path) DO UPDATE SET indexed_at = excluded.indexed_at",
        )
        .bind(path_str)
        .bind(now)
        .execute(&mut *tx)
        .await
        .map_err(|e| std::io::Error::other(e.to_string()))?;

        for sym in &symbols {
            let kind_str = kind_to_str(sym.kind);
            sqlx::query(
                "INSERT INTO index_symbols (file_path, name, kind, line, col)
                 VALUES (?, ?, ?, ?, ?)",
            )
            .bind(path_str)
            .bind(&sym.name)
            .bind(kind_str)
            .bind(sym.line as i64)
            .bind(sym.col as i64)
            .execute(&mut *tx)
            .await
            .map_err(|e| std::io::Error::other(e.to_string()))?;
        }

        tx.commit()
            .await
            .map_err(|e| std::io::Error::other(e.to_string()))?;

        Ok(())
    }

    /// Search symbols by name (case-insensitive substring match).
    ///
    /// Results are scored: exact match > prefix match > substring match.
    pub async fn search(&self, query: &str, limit: usize) -> Result<Vec<SearchResult>, IndexError> {
        let pattern = format!("%{}%", query);

        let rows = sqlx::query(
            "SELECT name, kind, file_path, line, col
             FROM index_symbols
             WHERE name LIKE ? COLLATE NOCASE
             LIMIT ?",
        )
        .bind(&pattern)
        .bind(limit as i64)
        .fetch_all(&self.pool)
        .await
        .map_err(|e| std::io::Error::other(e.to_string()))?;

        let query_lower = query.to_lowercase();
        let mut results: Vec<SearchResult> = rows
            .iter()
            .filter_map(|row| {
                let name: String = row.try_get("name").ok()?;
                let kind_str: String = row.try_get("kind").ok()?;
                let file_path: String = row.try_get("file_path").ok()?;
                let line: i64 = row.try_get("line").ok()?;
                let col: i64 = row.try_get("col").ok()?;

                let kind = str_to_kind(&kind_str)?;
                let name_lower = name.to_lowercase();

                let score = if name_lower == query_lower {
                    SCORE_EXACT
                } else if name_lower.starts_with(&query_lower) {
                    SCORE_PREFIX
                } else {
                    SCORE_SUBSTRING
                };

                Some(SearchResult {
                    symbol: Symbol {
                        name,
                        kind,
                        file: std::path::PathBuf::from(file_path),
                        line: line as u32,
                        col: col as u32,
                    },
                    score,
                })
            })
            .collect();

        results.sort_by(|a, b| {
            b.score
                .partial_cmp(&a.score)
                .unwrap_or(std::cmp::Ordering::Equal)
                .then_with(|| a.symbol.name.cmp(&b.symbol.name))
        });

        Ok(results)
    }

    /// Find the symbol at (or just before) the given file position.
    ///
    /// Returns the last symbol in source order whose declaration line is at or
    /// before `line` — the same heuristic used by `SemanticIndex::symbol_at`.
    pub async fn symbol_at(
        &self,
        file: &Path,
        line: u32,
        _col: u32,
    ) -> Result<Option<Symbol>, IndexError> {
        let file_str = file
            .to_str()
            .ok_or_else(|| std::io::Error::other("non-UTF-8 path"))?;

        let row = sqlx::query(
            "SELECT name, kind, file_path, line, col
             FROM index_symbols
             WHERE file_path = ? AND line <= ?
             ORDER BY line DESC
             LIMIT 1",
        )
        .bind(file_str)
        .bind(line as i64)
        .fetch_optional(&self.pool)
        .await
        .map_err(|e| std::io::Error::other(e.to_string()))?;

        let Some(row) = row else {
            return Ok(None);
        };

        let name: String = row
            .try_get("name")
            .map_err(|e| std::io::Error::other(e.to_string()))?;
        let kind_str: String = row
            .try_get("kind")
            .map_err(|e| std::io::Error::other(e.to_string()))?;
        let file_path: String = row
            .try_get("file_path")
            .map_err(|e| std::io::Error::other(e.to_string()))?;
        let sym_line: i64 = row
            .try_get("line")
            .map_err(|e| std::io::Error::other(e.to_string()))?;
        let sym_col: i64 = row
            .try_get("col")
            .map_err(|e| std::io::Error::other(e.to_string()))?;

        let kind = str_to_kind(&kind_str)
            .ok_or_else(|| std::io::Error::other(format!("unknown symbol kind: {kind_str}")))?;

        Ok(Some(Symbol {
            name,
            kind,
            file: std::path::PathBuf::from(file_path),
            line: sym_line as u32,
            col: sym_col as u32,
        }))
    }

    /// Remove all symbols for a file (for re-indexing).
    pub async fn invalidate(&self, path: &Path) -> Result<(), IndexError> {
        let path_str = path
            .to_str()
            .ok_or_else(|| std::io::Error::other("non-UTF-8 path"))?;

        // We need foreign_keys = ON so the CASCADE fires.
        let mut conn = self
            .pool
            .acquire()
            .await
            .map_err(|e| std::io::Error::other(e.to_string()))?;

        sqlx::query("PRAGMA foreign_keys = ON")
            .execute(&mut *conn)
            .await
            .map_err(|e| std::io::Error::other(e.to_string()))?;

        sqlx::query("DELETE FROM index_files WHERE path = ?")
            .bind(path_str)
            .execute(&mut *conn)
            .await
            .map_err(|e| std::io::Error::other(e.to_string()))?;

        Ok(())
    }

    /// Number of indexed files.
    pub async fn file_count(&self) -> Result<usize, IndexError> {
        let row = sqlx::query("SELECT COUNT(*) AS cnt FROM index_files")
            .fetch_one(&self.pool)
            .await
            .map_err(|e| std::io::Error::other(e.to_string()))?;

        let cnt: i64 = row
            .try_get("cnt")
            .map_err(|e| std::io::Error::other(e.to_string()))?;

        Ok(cnt as usize)
    }

    /// Total number of symbols.
    pub async fn symbol_count(&self) -> Result<usize, IndexError> {
        let row = sqlx::query("SELECT COUNT(*) AS cnt FROM index_symbols")
            .fetch_one(&self.pool)
            .await
            .map_err(|e| std::io::Error::other(e.to_string()))?;

        let cnt: i64 = row
            .try_get("cnt")
            .map_err(|e| std::io::Error::other(e.to_string()))?;

        Ok(cnt as usize)
    }
}

// ---------------------------------------------------------------------------
// SymbolKind <-> TEXT helpers
// ---------------------------------------------------------------------------

fn kind_to_str(kind: SymbolKind) -> &'static str {
    match kind {
        SymbolKind::Function => "Function",
        SymbolKind::Struct => "Struct",
        SymbolKind::Enum => "Enum",
        SymbolKind::Trait => "Trait",
        SymbolKind::Impl => "Impl",
        SymbolKind::Class => "Class",
        SymbolKind::Module => "Module",
        SymbolKind::Constant => "Constant",
        SymbolKind::TypeAlias => "TypeAlias",
    }
}

fn str_to_kind(s: &str) -> Option<SymbolKind> {
    match s {
        "Function" => Some(SymbolKind::Function),
        "Struct" => Some(SymbolKind::Struct),
        "Enum" => Some(SymbolKind::Enum),
        "Trait" => Some(SymbolKind::Trait),
        "Impl" => Some(SymbolKind::Impl),
        "Class" => Some(SymbolKind::Class),
        "Module" => Some(SymbolKind::Module),
        "Constant" => Some(SymbolKind::Constant),
        "TypeAlias" => Some(SymbolKind::TypeAlias),
        _ => None,
    }
}
