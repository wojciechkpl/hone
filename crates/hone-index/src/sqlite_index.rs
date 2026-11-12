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

/// At most this many tokens of a query are used.
const MAX_QUERY_TOKENS: usize = 16;

/// Rows read per token: enough for ranking to pick the best few, bounded.
const MAX_ROWS_PER_TOKEN: i64 = 2000;

const STOP_WORDS: &[&str] = &[
    "the", "and", "for", "are", "but", "not", "you", "all", "any", "can", "had", "her", "was",
    "one", "our", "out", "has", "have", "him", "his", "how", "its", "may", "new", "now", "old",
    "see", "two", "way", "who", "did", "get", "let", "put", "say", "she", "too", "use", "with",
    "that", "this", "from", "they", "will", "would", "there", "their", "what", "when", "where",
    "which", "while", "about", "into", "than", "then", "them", "these", "those", "your", "been",
    "were", "does", "should", "could", "just", "also", "some", "such", "only", "over", "very",
    "make", "please",
];

/// Lower-case word tokens (`[A-Za-z0-9_]`) of 3 or more characters, without
/// English stop words, first [`MAX_QUERY_TOKENS`] distinct ones.
fn query_tokens(query: &str) -> Vec<String> {
    let mut tokens: Vec<String> = Vec::new();
    for word in query.split(|c: char| !(c.is_ascii_alphanumeric() || c == '_')) {
        if word.len() < 3 {
            continue;
        }
        let word = word.to_ascii_lowercase();
        if STOP_WORDS.contains(&word.as_str()) || tokens.contains(&word) {
            continue;
        }
        tokens.push(word);
        if tokens.len() == MAX_QUERY_TOKENS {
            break;
        }
    }
    tokens
}

/// Escape `\`, `%` and `_` for a `LIKE ... ESCAPE '\'` pattern.
fn escape_like(token: &str) -> String {
    let mut out = String::with_capacity(token.len());
    for c in token.chars() {
        if matches!(c, '\\' | '%' | '_') {
            out.push('\\');
        }
        out.push(c);
    }
    out
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

    /// Search symbols by name, for a plain-English prompt or an identifier.
    ///
    /// The query is split into word tokens (see [`query_tokens`]); a symbol
    /// matches if its name contains any token (case-insensitive). Results are
    /// ranked by the number of distinct tokens the name contains, then exact >
    /// prefix > substring match, then shorter name, and cut to `limit`. A query
    /// with no usable token returns nothing.
    pub async fn search(&self, query: &str, limit: usize) -> Result<Vec<SearchResult>, IndexError> {
        let tokens = query_tokens(query);
        if tokens.is_empty() || limit == 0 {
            return Ok(Vec::new());
        }

        // One bound `LIKE` per token. Per token the shortest names come first
        // (they rank best), and the fetch is capped so a very common token
        // cannot read the whole table.
        let mut seen = std::collections::HashSet::new();
        let mut ranked: Vec<(usize, SearchResult)> = Vec::new();
        for token in &tokens {
            let pattern = format!("%{}%", escape_like(token));
            let rows = sqlx::query(
                "SELECT rowid AS rid, name, kind, file_path, line, col
                 FROM index_symbols
                 WHERE name LIKE ? ESCAPE '\\' COLLATE NOCASE
                 ORDER BY length(name), name
                 LIMIT ?",
            )
            .bind(&pattern)
            .bind(MAX_ROWS_PER_TOKEN)
            .fetch_all(&self.pool)
            .await
            .map_err(|e| std::io::Error::other(e.to_string()))?;

            for row in &rows {
                let Ok(rid) = row.try_get::<i64, _>("rid") else {
                    continue;
                };
                if !seen.insert(rid) {
                    continue;
                }
                let (Ok(name), Ok(kind_str), Ok(file_path), Ok(line), Ok(col)) = (
                    row.try_get::<String, _>("name"),
                    row.try_get::<String, _>("kind"),
                    row.try_get::<String, _>("file_path"),
                    row.try_get::<i64, _>("line"),
                    row.try_get::<i64, _>("col"),
                ) else {
                    continue;
                };
                let Some(kind) = str_to_kind(&kind_str) else {
                    continue;
                };
                let name_lower = name.to_lowercase();
                let matched = tokens
                    .iter()
                    .filter(|t| name_lower.contains(t.as_str()))
                    .count();
                let score = tokens
                    .iter()
                    .filter(|t| name_lower.contains(t.as_str()))
                    .map(|t| {
                        if name_lower == **t {
                            SCORE_EXACT
                        } else if name_lower.starts_with(t.as_str()) {
                            SCORE_PREFIX
                        } else {
                            SCORE_SUBSTRING
                        }
                    })
                    .fold(SCORE_SUBSTRING, f64::max);
                ranked.push((
                    matched,
                    SearchResult {
                        symbol: Symbol {
                            name,
                            kind,
                            file: std::path::PathBuf::from(file_path),
                            line: line as u32,
                            col: col as u32,
                        },
                        score,
                    },
                ));
            }
        }

        ranked.sort_by(|(ca, a), (cb, b)| {
            cb.cmp(ca)
                .then_with(|| {
                    b.score
                        .partial_cmp(&a.score)
                        .unwrap_or(std::cmp::Ordering::Equal)
                })
                .then_with(|| a.symbol.name.len().cmp(&b.symbol.name.len()))
                .then_with(|| a.symbol.name.cmp(&b.symbol.name))
        });
        ranked.truncate(limit);
        Ok(ranked.into_iter().map(|(_, r)| r).collect())
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
