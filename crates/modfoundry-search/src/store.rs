//! The search index on disk: one SQLite file per game build.

use std::collections::{HashMap, HashSet};
use std::fmt;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

use rusqlite::types::Type;
use rusqlite::{params, params_from_iter, Connection, OpenFlags};

use crate::chunk::{Chunk, Kind};

/// Layout version of the index file; another version is refused on open.
pub const SCHEMA: u32 = 1;

const TABLES: &str = "
CREATE TABLE meta (key TEXT PRIMARY KEY, value TEXT);
CREATE TABLE sources (
    id INTEGER PRIMARY KEY,
    label TEXT,
    root TEXT,
    reference_only INTEGER
);
CREATE TABLE files (
    id INTEGER PRIMARY KEY,
    source INTEGER REFERENCES sources,
    path TEXT,
    size INTEGER,
    hash TEXT
);
CREATE TABLE chunks (
    id INTEGER PRIMARY KEY,
    file INTEGER REFERENCES files,
    kind TEXT,
    class TEXT,
    method TEXT,
    first INTEGER,
    last INTEGER,
    snippet TEXT,
    text TEXT,
    vector BLOB
);
";

/// What an index was built from.
#[derive(Debug, Clone, PartialEq)]
pub struct Meta {
    pub model: String,
    pub revision: String,
    pub build: String,
    pub created: String,
}

/// Errors from reading or writing an index file.
#[derive(Debug)]
pub enum StoreError {
    Sql(rusqlite::Error),
    Io(std::io::Error),
    Dims { expected: usize, found: usize },
    Schema { found: String },
    Meta { key: String },
    NonFinite,
}

impl fmt::Display for StoreError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Sql(e) => write!(f, "index database: {e}"),
            Self::Io(e) => write!(f, "index file: {e}"),
            Self::Dims { expected, found } => {
                write!(f, "vector has {found} values, the index holds {expected}")
            }
            Self::Schema { found } => {
                write!(f, "index schema {found}, this build reads {SCHEMA}")
            }
            Self::Meta { key } => write!(f, "index has no {key} entry; rebuild it"),
            Self::NonFinite => write!(f, "vector holds a value that is not a finite number"),
        }
    }
}

impl std::error::Error for StoreError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Sql(e) => Some(e),
            Self::Io(e) => Some(e),
            Self::Dims { .. } | Self::Schema { .. } | Self::Meta { .. } | Self::NonFinite => None,
        }
    }
}

impl From<rusqlite::Error> for StoreError {
    fn from(e: rusqlite::Error) -> Self {
        Self::Sql(e)
    }
}

impl From<std::io::Error> for StoreError {
    fn from(e: std::io::Error) -> Self {
        Self::Io(e)
    }
}

/// Builds an index in `<path>.<pid>.<n>.part` and moves it into place on `finish`.
pub struct IndexWriter {
    conn: Connection,
    part: Part,
    path: PathBuf,
    dims: Option<usize>,
}

impl IndexWriter {
    /// Starts an index at `path`; an older index there stays until `finish`.
    pub fn create(path: &Path, meta: &Meta) -> Result<Self, StoreError> {
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent)?;
        }
        let part = Part {
            path: part_path(path),
        };
        if part.path.exists() {
            fs::remove_file(&part.path)?;
        }
        let conn = Connection::open(&part.path)?;
        // The part file is thrown away on any failure, so per-commit syncing buys nothing.
        conn.execute_batch("PRAGMA synchronous = OFF; PRAGMA journal_mode = MEMORY;")?;
        conn.execute_batch(TABLES)?;
        let schema = SCHEMA.to_string();
        let rows = [
            ("schema", schema.as_str()),
            ("model", meta.model.as_str()),
            ("revision", meta.revision.as_str()),
            ("build", meta.build.as_str()),
            ("created", meta.created.as_str()),
        ];
        for (key, value) in rows {
            conn.execute(
                "INSERT INTO meta (key, value) VALUES (?1, ?2)",
                params![key, value],
            )?;
        }
        Ok(Self {
            conn,
            part,
            path: path.to_path_buf(),
            dims: None,
        })
    }

    /// Records a source folder and returns its id.
    pub fn add_source(
        &mut self,
        label: &str,
        root: &str,
        reference_only: bool,
    ) -> Result<i64, StoreError> {
        self.conn.execute(
            "INSERT INTO sources (label, root, reference_only) VALUES (?1, ?2, ?3)",
            params![label, root, reference_only],
        )?;
        Ok(self.conn.last_insert_rowid())
    }

    /// Records a file under a source and returns its id.
    pub fn add_file(
        &mut self,
        source: i64,
        path: &str,
        size: u64,
        hash: &str,
    ) -> Result<i64, StoreError> {
        // rusqlite takes u64 only with its fallible_uint feature, which is not enabled here.
        self.conn.execute(
            "INSERT INTO files (source, path, size, hash) VALUES (?1, ?2, ?3, ?4)",
            params![source, path, size as i64, hash],
        )?;
        Ok(self.conn.last_insert_rowid())
    }

    /// Stores a file's chunks with their snippets and vectors, in one transaction.
    pub fn add_chunks(
        &mut self,
        file: i64,
        chunks: &[(Chunk, String, Vec<f32>)],
    ) -> Result<(), StoreError> {
        let expected = self
            .dims
            .or_else(|| chunks.first().map(|(_, _, vector)| vector.len()));
        for (_, _, vector) in chunks {
            if let Some(expected) = expected {
                if vector.len() != expected {
                    return Err(StoreError::Dims {
                        expected,
                        found: vector.len(),
                    });
                }
            }
            if vector.iter().any(|value| !value.is_finite()) {
                return Err(StoreError::NonFinite);
            }
        }
        let tx = self.conn.transaction()?;
        {
            let mut stmt = tx.prepare(
                "INSERT INTO chunks
                 (file, kind, class, method, first, last, snippet, text, vector)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)",
            )?;
            for (chunk, snippet, vector) in chunks {
                stmt.execute(params![
                    file,
                    kind_text(&chunk.kind),
                    chunk.class,
                    chunk.method,
                    chunk.lines[0] as i64,
                    chunk.lines[1] as i64,
                    snippet,
                    chunk.text,
                    vector_bytes(vector),
                ])?;
            }
        }
        tx.commit()?;
        self.dims = expected;
        Ok(())
    }

    /// Closes the file and moves it to the index path, replacing an older index.
    pub fn finish(self) -> Result<(), StoreError> {
        self.conn.close().map_err(|(_, e)| e)?;
        fs::File::open(&self.part.path)?.sync_all()?;
        fs::rename(&self.part.path, &self.path)?;
        Ok(())
    }
}

struct Part {
    path: PathBuf,
}

impl Drop for Part {
    fn drop(&mut self) {
        let _ = fs::remove_file(&self.path);
    }
}

/// A finished index with its vectors loaded for scoring.
pub struct Index {
    conn: Connection,
    meta: Meta,
    ids: Vec<i64>,
    vectors: Vec<f32>,
    dims: usize,
}

impl Index {
    /// Opens a finished index read-only; refuses a file of another schema.
    pub fn open(path: &Path) -> Result<Self, StoreError> {
        let conn = Connection::open_with_flags(path, OpenFlags::SQLITE_OPEN_READ_ONLY)?;
        let mut values: HashMap<String, String> = conn
            .prepare("SELECT key, value FROM meta")?
            .query_map([], |row| {
                Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
            })?
            .collect::<Result<_, _>>()?;
        let mut take = |key: &str| -> Result<String, StoreError> {
            values.remove(key).ok_or_else(|| StoreError::Meta {
                key: key.to_string(),
            })
        };
        let found = take("schema")?;
        if found != SCHEMA.to_string() {
            return Err(StoreError::Schema { found });
        }
        let meta = Meta {
            model: take("model")?,
            revision: take("revision")?,
            build: take("build")?,
            created: take("created")?,
        };
        let (ids, vectors) = load_vectors(&conn)?;
        let dims = vectors.len().checked_div(ids.len()).unwrap_or(0);
        Ok(Self {
            conn,
            meta,
            ids,
            vectors,
            dims,
        })
    }

    /// What the index was built from.
    pub fn meta(&self) -> &Meta {
        &self.meta
    }

    /// Number of chunks in the index.
    pub fn len(&self) -> usize {
        self.ids.len()
    }

    /// Length of every vector in the index.
    pub fn dims(&self) -> usize {
        self.dims
    }

    /// Whether the index holds no chunks.
    pub fn is_empty(&self) -> bool {
        self.ids.is_empty()
    }

    /// Best `top` chunk ids by dot product, ties by lower id; refuses a query of another length.
    pub fn nearest(&self, query: &[f32], top: usize) -> Result<Vec<(i64, f32)>, StoreError> {
        if query.len() != self.dims {
            return Err(StoreError::Dims {
                expected: self.dims,
                found: query.len(),
            });
        }
        let dims = self.dims;
        let mut scored: Vec<(i64, f32)> = self
            .ids
            .iter()
            .enumerate()
            .map(|(i, &id)| (id, dot(query, &self.vectors[i * dims..(i + 1) * dims])))
            .collect();
        scored.sort_by(|a, b| b.1.total_cmp(&a.1).then(a.0.cmp(&b.0)));
        scored.truncate(top);
        Ok(scored)
    }

    /// Every chunk's id and text, in id order.
    pub fn texts(&self) -> Result<Vec<(i64, String)>, StoreError> {
        let mut stmt = self
            .conn
            .prepare("SELECT id, text FROM chunks ORDER BY id")?;
        let rows = stmt
            .query_map([], |row| Ok((row.get(0)?, row.get(1)?)))?
            .collect::<Result<_, _>>()?;
        Ok(rows)
    }

    /// Chunk ids of the given kinds and source labels, labels ignoring ASCII case; an empty list matches any.
    pub fn ids_where(
        &self,
        kinds: &[Kind],
        sources: &[String],
    ) -> Result<HashSet<i64>, StoreError> {
        let mut clauses = Vec::new();
        let mut values: Vec<String> = Vec::new();
        if !kinds.is_empty() {
            clauses.push(format!("c.kind IN ({})", placeholders(kinds.len())));
            values.extend(kinds.iter().map(|kind| kind_text(kind).to_string()));
        }
        if !sources.is_empty() {
            clauses.push(format!(
                "s.label COLLATE NOCASE IN ({})",
                placeholders(sources.len())
            ));
            values.extend(sources.iter().cloned());
        }
        let mut sql = String::from(
            "SELECT c.id FROM chunks c JOIN files f ON f.id = c.file JOIN sources s ON s.id = f.source",
        );
        if !clauses.is_empty() {
            sql.push_str(" WHERE ");
            sql.push_str(&clauses.join(" AND "));
        }
        let mut stmt = self.conn.prepare(&sql)?;
        let ids = stmt
            .query_map(params_from_iter(values), |row| row.get::<_, i64>(0))?
            .collect::<Result<HashSet<i64>, _>>()?;
        Ok(ids)
    }

    /// The stored chunk with this id, joined with its file and source.
    pub fn hit(&self, chunk: i64) -> Result<StoredHit, StoreError> {
        let hit = self.conn.query_row(
            "SELECT c.kind, c.class, c.method, c.first, c.last, c.snippet, c.text,
                    f.path, f.size, f.hash, s.label, s.root, s.reference_only
             FROM chunks c
             JOIN files f ON f.id = c.file
             JOIN sources s ON s.id = f.source
             WHERE c.id = ?1",
            params![chunk],
            |row| {
                let text: String = row.get(0)?;
                let kind = kind_from_text(&text).ok_or_else(|| {
                    rusqlite::Error::FromSqlConversionFailure(
                        0,
                        Type::Text,
                        format!("unknown chunk kind {text}").into(),
                    )
                })?;
                Ok(StoredHit {
                    kind,
                    class: row.get(1)?,
                    method: row.get(2)?,
                    lines: [
                        row.get::<_, i64>(3)? as usize,
                        row.get::<_, i64>(4)? as usize,
                    ],
                    snippet: row.get(5)?,
                    text: row.get(6)?,
                    path: row.get(7)?,
                    size: row.get::<_, i64>(8)? as u64,
                    hash: row.get(9)?,
                    source: row.get(10)?,
                    root: row.get(11)?,
                    reference_only: row.get(12)?,
                })
            },
        )?;
        Ok(hit)
    }
}

/// A chunk as stored, with the file and source it came from.
#[derive(Debug, PartialEq)]
pub struct StoredHit {
    pub kind: Kind,
    pub class: Option<String>,
    pub method: Option<String>,
    pub lines: [usize; 2],
    pub snippet: String,
    pub text: String,
    pub path: String,
    pub size: u64,
    pub hash: String,
    pub source: String,
    pub root: String,
    pub reference_only: bool,
}

static NEXT_PART: AtomicU64 = AtomicU64::new(0);

pub(crate) fn part_path(path: &Path) -> PathBuf {
    let mut part = path.as_os_str().to_owned();
    let n = NEXT_PART.fetch_add(1, Ordering::Relaxed);
    part.push(format!(".{}.{n}.part", std::process::id()));
    part.into()
}

fn load_vectors(conn: &Connection) -> Result<(Vec<i64>, Vec<f32>), StoreError> {
    let mut stmt = conn.prepare("SELECT id, vector FROM chunks ORDER BY id")?;
    let rows = stmt.query_map([], |row| {
        Ok((row.get::<_, i64>(0)?, row.get::<_, Vec<u8>>(1)?))
    })?;
    let mut ids = Vec::new();
    let mut vectors = Vec::new();
    let mut width = None;
    for row in rows {
        let (id, bytes) = row?;
        let expected = *width.get_or_insert(bytes.len());
        let (floats, rest) = bytes.as_chunks::<4>();
        if bytes.len() != expected || !rest.is_empty() {
            return Err(StoreError::Dims {
                expected: expected / 4,
                found: floats.len(),
            });
        }
        ids.push(id);
        vectors.extend(floats.iter().map(|b| f32::from_le_bytes(*b)));
    }
    Ok((ids, vectors))
}

fn dot(a: &[f32], b: &[f32]) -> f32 {
    a.iter().zip(b).map(|(x, y)| x * y).sum()
}

fn vector_bytes(vector: &[f32]) -> Vec<u8> {
    vector.iter().flat_map(|v| v.to_le_bytes()).collect()
}

fn placeholders(count: usize) -> String {
    vec!["?"; count].join(", ")
}

fn kind_text(kind: &Kind) -> &'static str {
    match kind {
        Kind::Script => "script",
        Kind::Config => "config",
        Kind::Layout => "layout",
        Kind::Docs => "docs",
    }
}

fn kind_from_text(text: &str) -> Option<Kind> {
    match text {
        "script" => Some(Kind::Script),
        "config" => Some(Kind::Config),
        "layout" => Some(Kind::Layout),
        "docs" => Some(Kind::Docs),
        _ => None,
    }
}
