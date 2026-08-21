use std::collections::HashSet;
use std::fs::{self, File, OpenOptions};
use std::io::{BufRead, BufReader, BufWriter, Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};

use fs2::FileExt;
use rusqlite::{params, params_from_iter, Connection, OptionalExtension, Result};
use serde::{Deserialize, Serialize};

pub const SCHEMA_SQL: &str = r#"
PRAGMA foreign_keys = ON;

CREATE TABLE IF NOT EXISTS samples (
    id        INTEGER PRIMARY KEY,
    hash      TEXT NOT NULL UNIQUE,
    title     TEXT,
    favorited INTEGER NOT NULL DEFAULT 0,
    added_at  INTEGER NOT NULL DEFAULT (unixepoch())
);

CREATE TABLE IF NOT EXISTS tags (
    id   INTEGER PRIMARY KEY,
    name TEXT NOT NULL UNIQUE COLLATE NOCASE
);

CREATE TABLE IF NOT EXISTS sample_tags (
    sample_id INTEGER NOT NULL REFERENCES samples(id) ON DELETE CASCADE,
    tag_id    INTEGER NOT NULL REFERENCES tags(id) ON DELETE CASCADE,
    PRIMARY KEY (sample_id, tag_id)
);

CREATE INDEX IF NOT EXISTS idx_sample_tags_tag ON sample_tags(tag_id);

CREATE TABLE IF NOT EXISTS sample_tag_origins (
    sample_id INTEGER NOT NULL REFERENCES samples(id) ON DELETE CASCADE,
    tag_id    INTEGER NOT NULL REFERENCES tags(id) ON DELETE CASCADE,
    origin    TEXT NOT NULL,
    PRIMARY KEY (sample_id, tag_id, origin)
);

CREATE TABLE IF NOT EXISTS sample_titles (
    sample_id INTEGER NOT NULL REFERENCES samples(id) ON DELETE CASCADE,
    origin    TEXT NOT NULL,
    title     TEXT NOT NULL,
    PRIMARY KEY (sample_id, origin)
);

CREATE TABLE IF NOT EXISTS sample_provenance (
    sample_id INTEGER NOT NULL REFERENCES samples(id) ON DELETE CASCADE,
    origin    TEXT NOT NULL,
    PRIMARY KEY (sample_id, origin)
);
CREATE INDEX IF NOT EXISTS idx_sample_provenance_origin ON sample_provenance(origin);

CREATE TABLE IF NOT EXISTS schema_migrations (
    version INTEGER PRIMARY KEY
);

CREATE TABLE IF NOT EXISTS user_fact_replay_state (
    singleton   INTEGER PRIMARY KEY CHECK (singleton = 1),
    byte_offset INTEGER NOT NULL
);

CREATE TABLE IF NOT EXISTS package_sources (
    origin    TEXT NOT NULL,
    wire_id   TEXT NOT NULL,
    source_id INTEGER NOT NULL REFERENCES sources(id) ON DELETE CASCADE,
    PRIMARY KEY (origin, wire_id)
);

CREATE TABLE IF NOT EXISTS sources (
    id            INTEGER PRIMARY KEY,
    kind          TEXT NOT NULL,
    title         TEXT,
    release_title TEXT,
    notes         TEXT,
    created_at    INTEGER NOT NULL DEFAULT (unixepoch())
);

CREATE TABLE IF NOT EXISTS source_contributors (
    id        INTEGER PRIMARY KEY,
    source_id INTEGER NOT NULL REFERENCES sources(id) ON DELETE CASCADE,
    role      TEXT NOT NULL,
    name      TEXT NOT NULL,
    UNIQUE(source_id, role, name)
);

CREATE TABLE IF NOT EXISTS source_refs (
    id        INTEGER PRIMARY KEY,
    source_id INTEGER NOT NULL REFERENCES sources(id) ON DELETE CASCADE,
    provider  TEXT NOT NULL,
    ref_kind  TEXT NOT NULL,
    ref_value TEXT NOT NULL,
    url       TEXT,
    UNIQUE(provider, ref_kind, ref_value)
);

CREATE INDEX IF NOT EXISTS idx_source_refs_source ON source_refs(source_id);

CREATE TABLE IF NOT EXISTS source_assets (
    id        INTEGER PRIMARY KEY,
    source_id INTEGER NOT NULL REFERENCES sources(id) ON DELETE CASCADE,
    kind      TEXT NOT NULL,
    hash      TEXT NOT NULL,
    path      TEXT NOT NULL,
    mime_type TEXT,
    width     INTEGER,
    height    INTEGER,
    UNIQUE(source_id, kind, hash)
);

CREATE TABLE IF NOT EXISTS sample_origins (
    id               INTEGER PRIMARY KEY,
    dedupe_key       TEXT UNIQUE,
    sample_id        INTEGER NOT NULL REFERENCES samples(id) ON DELETE CASCADE,
    source_id        INTEGER REFERENCES sources(id) ON DELETE SET NULL,
    parent_sample_id INTEGER REFERENCES samples(id) ON DELETE SET NULL,
    method           TEXT NOT NULL,
    source_start_ms  INTEGER,
    source_end_ms    INTEGER,
    captured_at      INTEGER,
    notes            TEXT
);

CREATE INDEX IF NOT EXISTS idx_sample_origins_sample ON sample_origins(sample_id);
CREATE INDEX IF NOT EXISTS idx_sample_origins_source ON sample_origins(source_id);

CREATE TABLE IF NOT EXISTS source_metadata_guesses (
    id         INTEGER PRIMARY KEY,
    source_id  INTEGER NOT NULL REFERENCES sources(id) ON DELETE CASCADE,
    field      TEXT NOT NULL,
    value      TEXT NOT NULL,
    method     TEXT NOT NULL,
    confidence REAL NOT NULL,
    accepted   INTEGER NOT NULL DEFAULT 0,
    UNIQUE(source_id, field, value, method)
);
"#;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SampleRow {
    pub hash: String,
    pub title: Option<String>,
    pub favorited: bool,
    pub tags: Vec<String>,
    pub origins: Vec<String>,
    pub available: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TagFacet {
    pub name: String,
    pub count: usize,
    pub selected: bool,
}

#[derive(Debug)]
pub struct SampleDb {
    conn: Connection,
    journal_path: Option<PathBuf>,
    store_path: Option<PathBuf>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
enum UserFact {
    Sample {
        hash: String,
        title: Option<String>,
        tags: Vec<String>,
        favorited: bool,
    },
    AddTag {
        hash: String,
        tag: String,
    },
    RemoveTag {
        hash: String,
        tag: String,
    },
    SetFavorite {
        hash: String,
        favorited: bool,
    },
    SetTitle {
        hash: String,
        title: Option<String>,
    },
}

impl SampleDb {
    pub fn open(path: &Path) -> Result<Self> {
        let existed = path.is_file();
        let conn = Connection::open(path)?;
        initialize_connection(&conn)?;
        migrate_legacy_rows(&conn)?;
        let parent = path.parent().unwrap_or_else(|| Path::new("."));
        let journal_path = parent.join("samples.jsonl");
        let store_path = parent.join("samples");
        let mut db = Self {
            conn,
            journal_path: Some(journal_path.clone()),
            store_path: Some(store_path.clone()),
        };
        if existed {
            if !journal_path.is_file() {
                db.write_initial_user_snapshot()?;
            }
        } else {
            db.rebuild_store_rows(&store_path)?;
        }
        db.replay_user_facts(&journal_path)?;
        Ok(db)
    }

    pub fn open_in_memory() -> Result<Self> {
        let conn = Connection::open_in_memory()?;
        initialize_connection(&conn)?;
        migrate_legacy_rows(&conn)?;
        Ok(Self {
            conn,
            journal_path: None,
            store_path: None,
        })
    }

    pub fn initialize_schema(&self) -> Result<()> {
        initialize_connection(&self.conn)
    }

    pub fn connection(&self) -> &Connection {
        &self.conn
    }

    pub fn list_origins(&self) -> Result<Vec<TagFacet>> {
        let mut stmt = self.conn.prepare(
            "SELECT origin, COUNT(*) FROM sample_provenance GROUP BY origin ORDER BY origin COLLATE NOCASE",
        )?;
        let origins = stmt
            .query_map([], |row| {
                Ok(TagFacet {
                    name: row.get(0)?,
                    count: row.get::<_, i64>(1)?.max(0) as usize,
                    selected: false,
                })
            })?
            .collect();
        origins
    }

    pub fn list_tags(&self) -> Result<Vec<String>> {
        let mut stmt = self
            .conn
            .prepare("SELECT name FROM tags ORDER BY name COLLATE NOCASE")?;
        let rows = stmt.query_map([], |row| row.get(0))?.collect();
        rows
    }

    pub fn query(
        &self,
        include_tags: &[&str],
        exclude_tags: &[&str],
        text: Option<&str>,
        favorites_only: bool,
        origins: &[&str],
    ) -> Result<Vec<SampleRow>> {
        let mut sql = String::from(
            "SELECT s.id, s.hash, s.title, s.favorited \
             FROM samples s WHERE 1 = 1",
        );
        let mut values: Vec<String> = Vec::new();

        for tag in include_tags
            .iter()
            .map(|tag| tag.trim())
            .filter(|tag| !tag.is_empty())
        {
            sql.push_str(
                " AND EXISTS (\
                 SELECT 1 FROM sample_tags st \
                 JOIN tags t ON t.id = st.tag_id \
                 WHERE st.sample_id = s.id AND t.name = ? COLLATE NOCASE)",
            );
            values.push(tag.to_string());
        }

        for tag in exclude_tags
            .iter()
            .map(|tag| tag.trim())
            .filter(|tag| !tag.is_empty())
        {
            sql.push_str(
                " AND NOT EXISTS (\
                 SELECT 1 FROM sample_tags st \
                 JOIN tags t ON t.id = st.tag_id \
                 WHERE st.sample_id = s.id AND t.name = ? COLLATE NOCASE)",
            );
            values.push(tag.to_string());
        }

        if let Some(text) = text.map(str::trim).filter(|text| !text.is_empty()) {
            let pattern = sqlite_like_contains_pattern(text);
            sql.push_str(
                " AND (COALESCE(s.title, '') COLLATE NOCASE LIKE ? ESCAPE '\\' \
                 OR s.hash COLLATE NOCASE LIKE ? ESCAPE '\\')",
            );
            values.push(pattern.clone());
            values.push(pattern);
        }

        if favorites_only {
            sql.push_str(" AND s.favorited != 0");
        }
        append_origin_filter(&mut sql, &mut values, origins);

        sql.push_str(" ORDER BY LOWER(COALESCE(NULLIF(s.title, ''), s.hash)), s.hash");

        let mut stmt = self.conn.prepare(&sql)?;
        let rows = stmt.query_map(params_from_iter(values.iter()), |row| {
            Ok(QueriedSample {
                id: row.get(0)?,
                hash: row.get(1)?,
                title: row.get(2)?,
                favorited: row.get::<_, i64>(3)? != 0,
            })
        })?;

        let mut samples = Vec::new();
        for row in rows {
            let row = row?;
            let tags = self.tags_for_id(row.id)?;
            let available = self.sample_available(&row.hash);
            samples.push(SampleRow {
                hash: row.hash,
                title: row.title.and_then(clean_display_title),
                favorited: row.favorited,
                tags,
                origins: self.origins_for_id(row.id)?,
                available,
            });
        }
        Ok(samples)
    }

    pub fn query_samples_for_browser(
        &self,
        include_tags: &[&str],
        text: Option<&str>,
    ) -> Result<Vec<SampleRow>> {
        self.query_samples_for_browser_with_limit(include_tags, text, &[], None)
    }

    pub fn query_samples_for_browser_limited(
        &self,
        include_tags: &[&str],
        text: Option<&str>,
        max_samples: usize,
    ) -> Result<Vec<SampleRow>> {
        self.query_samples_for_browser_with_limit(include_tags, text, &[], Some(max_samples))
    }

    pub fn query_samples_for_browser_with_origins(
        &self,
        include_tags: &[&str],
        origins: &[&str],
        text: Option<&str>,
        max_samples: usize,
    ) -> Result<Vec<SampleRow>> {
        self.query_samples_for_browser_with_limit(include_tags, text, origins, Some(max_samples))
    }

    fn query_samples_for_browser_with_limit(
        &self,
        include_tags: &[&str],
        text: Option<&str>,
        origins: &[&str],
        max_samples: Option<usize>,
    ) -> Result<Vec<SampleRow>> {
        let mut sql = String::from(
            "SELECT s.id, s.hash, s.title, s.favorited \
             FROM samples s WHERE 1 = 1",
        );
        let mut values = Vec::new();
        append_browser_sample_filter(&mut sql, &mut values, include_tags, text);
        append_origin_filter(&mut sql, &mut values, origins);
        sql.push_str(" ORDER BY LOWER(COALESCE(NULLIF(s.title, ''), s.hash)), s.hash");
        if let Some(max_samples) = max_samples {
            sql.push_str(" LIMIT ");
            sql.push_str(&max_samples.to_string());
        }

        let mut stmt = self.conn.prepare(&sql)?;
        let rows = stmt.query_map(params_from_iter(values.iter()), |row| {
            Ok(QueriedSample {
                id: row.get(0)?,
                hash: row.get(1)?,
                title: row.get(2)?,
                favorited: row.get::<_, i64>(3)? != 0,
            })
        })?;

        let mut samples = Vec::new();
        for row in rows {
            let row = row?;
            let tags = self.tags_for_id(row.id)?;
            let available = self.sample_available(&row.hash);
            samples.push(SampleRow {
                hash: row.hash,
                title: row.title.and_then(clean_display_title),
                favorited: row.favorited,
                tags,
                origins: self.origins_for_id(row.id)?,
                available,
            });
        }
        Ok(samples)
    }

    pub fn adjacent_tags(
        &self,
        include_tags: &[&str],
        text: Option<&str>,
        max_tags: usize,
    ) -> Result<Vec<TagFacet>> {
        self.adjacent_tags_with_origins(include_tags, &[], text, max_tags)
    }

    pub fn adjacent_tags_with_origins(
        &self,
        include_tags: &[&str],
        origins: &[&str],
        text: Option<&str>,
        max_tags: usize,
    ) -> Result<Vec<TagFacet>> {
        let selected = normalized_tag_set(include_tags);
        let mut sql = String::from(
            "SELECT t.name, COUNT(*) AS sample_count \
             FROM tags t \
             JOIN sample_tags st ON st.tag_id = t.id \
             JOIN samples s ON s.id = st.sample_id \
             WHERE 1 = 1",
        );
        let mut values = Vec::new();
        append_browser_sample_filter(&mut sql, &mut values, include_tags, text);
        append_origin_filter(&mut sql, &mut values, origins);
        sql.push_str(
            " GROUP BY t.id \
             ORDER BY sample_count DESC, LOWER(t.name), t.name",
        );

        let mut stmt = self.conn.prepare(&sql)?;
        let mut facets = Vec::new();
        let rows = stmt.query_map(params_from_iter(values.iter()), |row| {
            let name: String = row.get(0)?;
            let count: i64 = row.get(1)?;
            Ok(TagFacet {
                selected: selected.contains(&name.to_lowercase()),
                name,
                count: count.max(0) as usize,
            })
        })?;
        for row in rows {
            facets.push(row?);
        }

        for tag in include_tags
            .iter()
            .map(|tag| tag.trim())
            .filter(|tag| !tag.is_empty())
        {
            if !facets
                .iter()
                .any(|facet| facet.name.eq_ignore_ascii_case(tag))
            {
                facets.push(TagFacet {
                    name: tag.to_string(),
                    count: 0,
                    selected: true,
                });
            }
        }
        if max_tags > 0 && facets.len() > max_tags {
            let mut selected_facets = Vec::new();
            let mut other_facets = Vec::new();
            for facet in facets {
                if facet.selected {
                    selected_facets.push(facet);
                } else {
                    other_facets.push(facet);
                }
            }
            let remaining = max_tags.saturating_sub(selected_facets.len());
            selected_facets.extend(other_facets.into_iter().take(remaining));
            facets = selected_facets;
        }
        Ok(facets)
    }

    pub fn tags_for(&self, hash: &str) -> Result<Vec<String>> {
        let Some(sample_id) = self.sample_id(hash)? else {
            return Ok(Vec::new());
        };
        self.tags_for_id(sample_id)
    }

    pub fn title_for_hash(&self, hash: &str) -> Result<Option<String>> {
        let hash = hash.trim();
        if hash.is_empty() {
            return Ok(None);
        }

        let title: Option<String> = self
            .conn
            .query_row(
                "SELECT title FROM samples WHERE hash = ?",
                params![hash],
                |row| row.get::<_, Option<String>>(0),
            )
            .optional()?
            .flatten();
        Ok(title.and_then(clean_display_title))
    }

    pub fn contains_sample(&self, hash: &str) -> Result<bool> {
        Ok(self.sample_id(hash.trim())?.is_some())
    }

    pub fn insert_sample_with_tags(
        &mut self,
        hash: &str,
        title: Option<&str>,
        tags: &[String],
    ) -> Result<bool> {
        let hash = hash.trim();
        if hash.is_empty() {
            return Ok(false);
        }
        if self.contains_sample(hash)? {
            return Ok(false);
        }
        let title = title.map(clean_display_title_str).unwrap_or(None);
        let tags = normalize_fact_tags(tags);
        self.append_user_fact(&UserFact::Sample {
            hash: hash.to_string(),
            title: title.clone(),
            tags: tags.clone(),
            favorited: false,
        })?;
        let tx = self.conn.transaction()?;
        tx.execute(
            "INSERT INTO samples(hash, title) VALUES (?, ?)",
            params![hash, title],
        )?;
        let sample_id = tx.last_insert_rowid();
        tx.execute(
            "INSERT INTO sample_provenance(sample_id, origin) VALUES (?, 'user')",
            params![sample_id],
        )?;
        if let Some(title) = &title {
            tx.execute(
                "INSERT INTO sample_titles(sample_id, origin, title) VALUES (?, 'user', ?)",
                params![sample_id, title],
            )?;
        }
        for tag in &tags {
            insert_tag_contribution(&tx, sample_id, tag, "user")?;
        }
        tx.commit()?;
        Ok(true)
    }

    pub fn add_tag(&self, hash: &str, tag: &str) -> Result<()> {
        let Some(sample_id) = self.sample_id(hash)? else {
            return Ok(());
        };
        let tag = tag.trim();
        if tag.is_empty() {
            return Ok(());
        }
        self.append_user_fact(&UserFact::AddTag {
            hash: hash.to_string(),
            tag: tag.to_string(),
        })?;
        insert_tag_contribution(&self.conn, sample_id, tag, "user")?;
        Ok(())
    }

    pub fn remove_tag(&self, hash: &str, tag: &str) -> Result<()> {
        let Some(sample_id) = self.sample_id(hash)? else {
            return Ok(());
        };
        let tag = tag.trim();
        if tag.is_empty() {
            return Ok(());
        }
        self.append_user_fact(&UserFact::RemoveTag {
            hash: hash.to_string(),
            tag: tag.to_string(),
        })?;
        self.conn.execute(
            "DELETE FROM sample_tag_origins WHERE sample_id = ? AND origin = 'user' \
             AND tag_id IN (SELECT id FROM tags WHERE name = ? COLLATE NOCASE)",
            params![sample_id, tag],
        )?;
        self.conn.execute(
            "DELETE FROM sample_tags WHERE sample_id = ? \
             AND tag_id IN (SELECT id FROM tags WHERE name = ? COLLATE NOCASE) \
             AND NOT EXISTS (SELECT 1 FROM sample_tag_origins sto \
                             WHERE sto.sample_id = sample_tags.sample_id \
                             AND sto.tag_id = sample_tags.tag_id)",
            params![sample_id, tag],
        )?;
        Ok(())
    }

    pub fn set_favorite(&self, hash: &str, favorited: bool) -> Result<()> {
        if self.sample_id(hash)?.is_none() {
            return Ok(());
        }
        self.append_user_fact(&UserFact::SetFavorite {
            hash: hash.to_string(),
            favorited,
        })?;
        self.conn.execute(
            "UPDATE samples SET favorited = ? WHERE hash = ?",
            params![if favorited { 1 } else { 0 }, hash],
        )?;
        Ok(())
    }

    pub fn set_title(&self, hash: &str, title: Option<&str>) -> Result<()> {
        let Some(sample_id) = self.sample_id(hash)? else {
            return Ok(());
        };
        let title = title.and_then(clean_display_title_str);
        self.append_user_fact(&UserFact::SetTitle {
            hash: hash.to_string(),
            title: title.clone(),
        })?;
        self.conn.execute(
            "DELETE FROM sample_titles WHERE sample_id = ? AND origin = 'user'",
            params![sample_id],
        )?;
        if let Some(title) = title {
            self.conn.execute(
                "INSERT INTO sample_titles(sample_id, origin, title) VALUES (?, 'user', ?)",
                params![sample_id, title],
            )?;
        }
        refresh_sample_title(&self.conn, sample_id)?;
        Ok(())
    }

    pub fn contribute_sample(
        &mut self,
        hash: &str,
        title: Option<&str>,
        tags: &[String],
        origin: &str,
    ) -> Result<()> {
        let hash = hash.trim();
        let origin = origin.trim();
        if hash.is_empty() || origin.is_empty() {
            return Ok(());
        }
        let tx = self.conn.transaction()?;
        tx.execute(
            "INSERT OR IGNORE INTO samples(hash) VALUES (?)",
            params![hash],
        )?;
        let sample_id: i64 = tx.query_row(
            "SELECT id FROM samples WHERE hash = ?",
            params![hash],
            |row| row.get(0),
        )?;
        tx.execute(
            "INSERT OR IGNORE INTO sample_provenance(sample_id, origin) VALUES (?, ?)",
            params![sample_id, origin],
        )?;
        if let Some(title) = title.and_then(clean_display_title_str) {
            tx.execute(
                "INSERT INTO sample_titles(sample_id, origin, title) VALUES (?, ?, ?) \
                 ON CONFLICT(sample_id, origin) DO UPDATE SET title = excluded.title",
                params![sample_id, origin, title],
            )?;
        }
        for tag in normalize_fact_tags(tags) {
            insert_tag_contribution(&tx, sample_id, &tag, origin)?;
        }
        refresh_sample_title(&tx, sample_id)?;
        tx.commit()
    }

    pub fn validate_source_merge(
        &self,
        source: &crate::sample_manifest::PackageSource,
    ) -> Result<()> {
        let mut matched: HashSet<i64> = HashSet::new();
        for reference in &source.refs {
            if let Some(id) = self.conn.query_row(
                "SELECT source_id FROM source_refs WHERE provider = ? AND ref_kind = ? AND ref_value = ?",
                params![reference.provider, reference.ref_kind, reference.ref_value], |row| row.get(0),
            ).optional()? { matched.insert(id); }
        }
        if matched.len() > 1 {
            Err(rusqlite::Error::InvalidParameterName(format!(
                "source {} references multiple existing sources",
                source.id
            )))
        } else {
            Ok(())
        }
    }

    pub fn contribute_source(
        &mut self,
        origin: &str,
        source: &crate::sample_manifest::PackageSource,
    ) -> Result<i64> {
        self.validate_source_merge(source)?;
        let mut matched: HashSet<i64> = HashSet::new();
        for reference in &source.refs {
            if let Some(id) = self.conn.query_row(
                "SELECT source_id FROM source_refs WHERE provider = ? AND ref_kind = ? AND ref_value = ?",
                params![reference.provider, reference.ref_kind, reference.ref_value], |row| row.get(0),
            ).optional()? { matched.insert(id); }
        }
        let tx = self.conn.transaction()?;
        let source_id = if let Some(id) = matched.into_iter().next() {
            id
        } else if let Some(id) = tx
            .query_row(
                "SELECT source_id FROM package_sources WHERE origin = ? AND wire_id = ?",
                params![origin, source.id],
                |row| row.get(0),
            )
            .optional()?
        {
            id
        } else {
            tx.execute(
                "INSERT INTO sources(kind, title, release_title) VALUES ('package', ?, ?)",
                params![source.title, source.release_title],
            )?;
            tx.last_insert_rowid()
        };
        tx.execute(
            "INSERT INTO package_sources(origin, wire_id, source_id) VALUES (?, ?, ?) \
             ON CONFLICT(origin, wire_id) DO UPDATE SET source_id = excluded.source_id",
            params![origin, source.id, source_id],
        )?;
        tx.execute(
            "UPDATE sources SET title = COALESCE(?, title), release_title = COALESCE(?, release_title) WHERE id = ?",
            params![source.title, source.release_title, source_id],
        )?;
        for contributor in &source.contributors {
            tx.execute(
                "INSERT OR IGNORE INTO source_contributors(source_id, role, name) VALUES (?, ?, ?)",
                params![source_id, contributor.role, contributor.name],
            )?;
        }
        for reference in &source.refs {
            tx.execute(
                "INSERT OR IGNORE INTO source_refs(source_id, provider, ref_kind, ref_value, url) VALUES (?, ?, ?, ?, ?)",
                params![source_id, reference.provider, reference.ref_kind, reference.ref_value, reference.url],
            )?;
        }
        for asset in &source.assets {
            tx.execute(
                "INSERT OR IGNORE INTO source_assets(source_id, kind, hash, path, mime_type) VALUES (?, ?, ?, ?, ?)",
                params![source_id, asset.kind, asset.hash, asset.path, asset.mime_type],
            )?;
        }
        tx.commit()?;
        Ok(source_id)
    }

    pub fn associate_package_source(&self, hash: &str, origin: &str, wire_id: &str) -> Result<()> {
        let Some(sample_id) = self.sample_id(hash)? else {
            return Ok(());
        };
        let source_id: i64 = self.conn.query_row(
            "SELECT source_id FROM package_sources WHERE origin = ? AND wire_id = ?",
            params![origin, wire_id],
            |row| row.get(0),
        )?;
        let dedupe = format!("{origin}:{hash}:{wire_id}");
        self.conn.execute(
            "INSERT INTO sample_origins(dedupe_key, sample_id, source_id, method) VALUES (?, ?, ?, 'package') \
             ON CONFLICT(dedupe_key) DO UPDATE SET source_id = excluded.source_id",
            params![dedupe, sample_id, source_id],
        )?;
        Ok(())
    }

    pub fn remove_origin(&mut self, origin: &str, sample_dir: &Path) -> Result<()> {
        let tx = self.conn.transaction()?;
        let sample_ids: Vec<i64> = {
            let mut stmt =
                tx.prepare("SELECT sample_id FROM sample_provenance WHERE origin = ?")?;
            let ids = stmt
                .query_map(params![origin], |row| row.get(0))?
                .collect::<Result<_>>()?;
            ids
        };
        let source_ids: Vec<i64> = {
            let mut stmt =
                tx.prepare("SELECT DISTINCT source_id FROM package_sources WHERE origin = ?")?;
            let ids = stmt
                .query_map(params![origin], |row| row.get(0))?
                .collect::<Result<_>>()?;
            ids
        };
        tx.execute(
            "DELETE FROM sample_origins WHERE method = 'package' AND dedupe_key LIKE ? ESCAPE '\\'",
            params![format!("{}:%", escape_like(origin))],
        )?;
        tx.execute(
            "DELETE FROM package_sources WHERE origin = ?",
            params![origin],
        )?;
        tx.execute(
            "DELETE FROM sample_provenance WHERE origin = ?",
            params![origin],
        )?;
        tx.execute(
            "DELETE FROM sample_titles WHERE origin = ?",
            params![origin],
        )?;
        tx.execute(
            "DELETE FROM sample_tag_origins WHERE origin = ?",
            params![origin],
        )?;
        let mut removed_hashes = Vec::new();
        for sample_id in sample_ids {
            tx.execute(
                "DELETE FROM sample_tags WHERE sample_id = ? AND NOT EXISTS (\
                 SELECT 1 FROM sample_tag_origins sto WHERE sto.sample_id = sample_tags.sample_id \
                 AND sto.tag_id = sample_tags.tag_id)",
                params![sample_id],
            )?;
            refresh_sample_title(&tx, sample_id)?;
            let remaining: i64 = tx.query_row(
                "SELECT COUNT(*) FROM sample_provenance WHERE sample_id = ?",
                params![sample_id],
                |row| row.get(0),
            )?;
            if remaining == 0 {
                if let Some(hash) = tx
                    .query_row(
                        "SELECT hash FROM samples WHERE id = ?",
                        params![sample_id],
                        |row| row.get::<_, String>(0),
                    )
                    .optional()?
                {
                    removed_hashes.push(hash);
                }
                tx.execute("DELETE FROM samples WHERE id = ?", params![sample_id])?;
            }
        }
        for source_id in source_ids {
            tx.execute(
                "DELETE FROM sources WHERE id = ? \
                 AND NOT EXISTS (SELECT 1 FROM package_sources WHERE source_id = ?) \
                 AND NOT EXISTS (SELECT 1 FROM sample_origins WHERE source_id = ?)",
                params![source_id, source_id, source_id],
            )?;
        }
        tx.commit()?;
        for hash in removed_hashes {
            let _ = fs::remove_file(sample_dir.join(format!("{hash}.wav")));
        }
        Ok(())
    }

    fn append_user_fact(&self, fact: &UserFact) -> Result<()> {
        let Some(path) = &self.journal_path else {
            return Ok(());
        };
        let parent = path.parent().unwrap_or_else(|| Path::new("."));
        fs::create_dir_all(parent).map_err(sql_io_error)?;
        let mut encoded = serde_json::to_vec(fact).map_err(sql_json_error)?;
        encoded.push(b'\n');
        let mut file = OpenOptions::new()
            .create(true)
            .append(true)
            .open(path)
            .map_err(sql_io_error)?;
        file.lock_exclusive().map_err(sql_io_error)?;
        let result = file.write_all(&encoded).and_then(|_| file.sync_data());
        let unlock_result = file.unlock();
        result.map_err(sql_io_error)?;
        unlock_result.map_err(sql_io_error)
    }

    fn write_initial_user_snapshot(&self) -> Result<()> {
        let Some(path) = &self.journal_path else {
            return Ok(());
        };
        let tmp = path.with_extension("jsonl.tmp");
        let file = File::create(&tmp).map_err(sql_io_error)?;
        let mut writer = BufWriter::new(file);
        let mut stmt = self.conn.prepare(
            "SELECT s.id, s.hash, ut.title, s.favorited FROM samples s \
             JOIN sample_provenance sp ON sp.sample_id = s.id AND sp.origin = 'user' \
             LEFT JOIN sample_titles ut ON ut.sample_id = s.id AND ut.origin = 'user' \
             ORDER BY s.hash",
        )?;
        let rows = stmt.query_map([], |row| {
            Ok((
                row.get::<_, i64>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, Option<String>>(2)?,
                row.get::<_, i64>(3)? != 0,
            ))
        })?;
        for row in rows {
            let (id, hash, title, favorited) = row?;
            let fact = UserFact::Sample {
                hash,
                title,
                tags: self.tags_for_origin_id(id, "user")?,
                favorited,
            };
            serde_json::to_writer(&mut writer, &fact).map_err(sql_json_error)?;
            writer.write_all(b"\n").map_err(sql_io_error)?;
        }
        writer.flush().map_err(sql_io_error)?;
        writer.get_ref().sync_all().map_err(sql_io_error)?;
        fs::rename(tmp, path).map_err(sql_io_error)?;
        let byte_offset = fs::metadata(path).map_err(sql_io_error)?.len();
        self.conn.execute(
            "INSERT INTO user_fact_replay_state(singleton, byte_offset) VALUES (1, ?) \
             ON CONFLICT(singleton) DO UPDATE SET byte_offset = excluded.byte_offset",
            params![byte_offset],
        )?;
        Ok(())
    }

    fn rebuild_store_rows(&mut self, store_path: &Path) -> Result<()> {
        if let Ok(entries) = fs::read_dir(store_path) {
            for entry in entries.flatten() {
                let path = entry.path();
                if path
                    .extension()
                    .and_then(|ext| ext.to_str())
                    .is_some_and(|ext| ext.eq_ignore_ascii_case("wav"))
                {
                    if let Some(hash) = path
                        .file_stem()
                        .and_then(|stem| stem.to_str())
                        .filter(|hash| !hash.is_empty())
                    {
                        self.conn.execute(
                            "INSERT OR IGNORE INTO samples(hash) VALUES (?)",
                            params![hash],
                        )?;
                    }
                }
            }
        }
        Ok(())
    }

    fn replay_user_facts(&mut self, journal_path: &Path) -> Result<()> {
        let Ok(mut file) = File::open(journal_path) else {
            return Ok(());
        };
        let file_len = file.metadata().map_err(sql_io_error)?.len();
        let mut offset: u64 = self
            .conn
            .query_row(
                "SELECT byte_offset FROM user_fact_replay_state WHERE singleton = 1",
                [],
                |row| row.get(0),
            )
            .optional()?
            .unwrap_or(0);
        if offset > file_len {
            offset = 0;
        }
        file.seek(SeekFrom::Start(offset)).map_err(sql_io_error)?;
        let mut reader = BufReader::new(file);
        let mut line = String::new();
        let mut line_number = 0usize;
        loop {
            line.clear();
            if reader.read_line(&mut line).map_err(sql_io_error)? == 0 {
                break;
            }
            line_number += 1;
            if line.trim().is_empty() {
                continue;
            }
            let fact: UserFact = serde_json::from_str(&line).map_err(|error| {
                rusqlite::Error::InvalidParameterName(format!(
                    "{} after byte {offset}, line {line_number}: {error}",
                    journal_path.display()
                ))
            })?;
            self.apply_user_fact(fact)?;
        }
        let replayed_to = reader.stream_position().map_err(sql_io_error)?;
        self.conn.execute(
            "INSERT INTO user_fact_replay_state(singleton, byte_offset) VALUES (1, ?) \
             ON CONFLICT(singleton) DO UPDATE SET byte_offset = excluded.byte_offset",
            params![replayed_to],
        )?;
        Ok(())
    }

    fn apply_user_fact(&mut self, fact: UserFact) -> Result<()> {
        match fact {
            UserFact::Sample {
                hash,
                title,
                tags,
                favorited,
            } => {
                self.conn.execute(
                    "INSERT OR IGNORE INTO samples(hash) VALUES (?)",
                    params![hash],
                )?;
                let id = self.sample_id(&hash)?.expect("sample inserted");
                self.conn.execute(
                    "INSERT OR IGNORE INTO sample_provenance(sample_id, origin) VALUES (?, 'user')",
                    params![id],
                )?;
                self.conn.execute(
                    "DELETE FROM sample_titles WHERE sample_id = ? AND origin = 'user'",
                    params![id],
                )?;
                self.conn.execute(
                    "DELETE FROM sample_tag_origins WHERE sample_id = ? AND origin = 'user'",
                    params![id],
                )?;
                self.conn.execute(
                    "DELETE FROM sample_tags WHERE sample_id = ? AND NOT EXISTS (\
                     SELECT 1 FROM sample_tag_origins sto WHERE sto.sample_id = sample_tags.sample_id \
                     AND sto.tag_id = sample_tags.tag_id)",
                    params![id],
                )?;
                if let Some(title) = title.and_then(|title| clean_display_title(title)) {
                    self.conn.execute(
                        "INSERT INTO sample_titles(sample_id, origin, title) VALUES (?, 'user', ?)",
                        params![id, title],
                    )?;
                }
                for tag in normalize_fact_tags(&tags) {
                    insert_tag_contribution(&self.conn, id, &tag, "user")?;
                }
                self.conn.execute(
                    "UPDATE samples SET favorited = ? WHERE id = ?",
                    params![favorited as i64, id],
                )?;
                refresh_sample_title(&self.conn, id)?;
            }
            UserFact::AddTag { hash, tag } => {
                if let Some(id) = self.sample_id(&hash)? {
                    insert_tag_contribution(&self.conn, id, &tag, "user")?;
                }
            }
            UserFact::RemoveTag { hash, tag } => {
                if let Some(id) = self.sample_id(&hash)? {
                    self.conn.execute("DELETE FROM sample_tag_origins WHERE sample_id = ? AND origin = 'user' AND tag_id IN (SELECT id FROM tags WHERE name = ? COLLATE NOCASE)", params![id, tag])?;
                    self.conn.execute("DELETE FROM sample_tags WHERE sample_id = ? AND tag_id IN (SELECT id FROM tags WHERE name = ? COLLATE NOCASE) AND NOT EXISTS (SELECT 1 FROM sample_tag_origins sto WHERE sto.sample_id = sample_tags.sample_id AND sto.tag_id = sample_tags.tag_id)", params![id, tag])?;
                }
            }
            UserFact::SetFavorite { hash, favorited } => {
                self.conn.execute(
                    "UPDATE samples SET favorited = ? WHERE hash = ?",
                    params![favorited as i64, hash],
                )?;
            }
            UserFact::SetTitle { hash, title } => {
                if let Some(id) = self.sample_id(&hash)? {
                    self.conn.execute(
                        "DELETE FROM sample_titles WHERE sample_id = ? AND origin = 'user'",
                        params![id],
                    )?;
                    if let Some(title) = title.and_then(clean_display_title) {
                        self.conn.execute("INSERT INTO sample_titles(sample_id, origin, title) VALUES (?, 'user', ?)", params![id, title])?;
                    }
                    refresh_sample_title(&self.conn, id)?;
                }
            }
        }
        Ok(())
    }

    fn sample_id(&self, hash: &str) -> Result<Option<i64>> {
        self.conn
            .query_row(
                "SELECT id FROM samples WHERE hash = ?",
                params![hash],
                |row| row.get(0),
            )
            .optional()
    }

    fn sample_available(&self, hash: &str) -> bool {
        self.store_path
            .as_ref()
            .is_some_and(|store| store.join(format!("{hash}.wav")).is_file())
    }

    fn origins_for_id(&self, sample_id: i64) -> Result<Vec<String>> {
        let mut stmt = self.conn.prepare(
            "SELECT origin FROM sample_provenance WHERE sample_id = ? ORDER BY origin COLLATE NOCASE",
        )?;
        let origins = stmt
            .query_map(params![sample_id], |row| row.get(0))?
            .collect();
        origins
    }

    fn tags_for_origin_id(&self, sample_id: i64, origin: &str) -> Result<Vec<String>> {
        let mut stmt = self.conn.prepare(
            "SELECT t.name FROM tags t \
             JOIN sample_tag_origins sto ON sto.tag_id = t.id \
             WHERE sto.sample_id = ? AND sto.origin = ? ORDER BY t.name COLLATE NOCASE",
        )?;
        let tags = stmt
            .query_map(params![sample_id, origin], |row| row.get(0))?
            .collect();
        tags
    }

    fn tags_for_id(&self, sample_id: i64) -> Result<Vec<String>> {
        let mut stmt = self.conn.prepare(
            "SELECT t.name \
             FROM tags t \
             JOIN sample_tags st ON st.tag_id = t.id \
             WHERE st.sample_id = ? \
             ORDER BY t.name COLLATE NOCASE",
        )?;
        let rows = stmt
            .query_map(params![sample_id], |row| row.get(0))?
            .collect();
        rows
    }
}

#[derive(Debug)]
struct QueriedSample {
    id: i64,
    hash: String,
    title: Option<String>,
    favorited: bool,
}

pub fn initialize_connection(conn: &Connection) -> Result<()> {
    conn.pragma_update(None, "foreign_keys", "ON")?;
    conn.execute_batch(SCHEMA_SQL)?;
    Ok(())
}

fn migrate_legacy_rows(conn: &Connection) -> Result<()> {
    let migrated: bool = conn.query_row(
        "SELECT EXISTS(SELECT 1 FROM schema_migrations WHERE version = 1)",
        [],
        |row| row.get(0),
    )?;
    if migrated {
        return Ok(());
    }
    let tx = conn.unchecked_transaction()?;
    tx.execute(
        "INSERT OR IGNORE INTO sample_provenance(sample_id, origin) SELECT id, 'user' FROM samples",
        [],
    )?;
    tx.execute("INSERT OR IGNORE INTO sample_titles(sample_id, origin, title) SELECT id, 'user', title FROM samples WHERE title IS NOT NULL AND TRIM(title) != ''", [])?;
    tx.execute("INSERT OR IGNORE INTO sample_tag_origins(sample_id, tag_id, origin) SELECT sample_id, tag_id, 'user' FROM sample_tags", [])?;
    tx.execute("INSERT INTO schema_migrations(version) VALUES (1)", [])?;
    tx.commit()
}

fn insert_tag_contribution(
    conn: &Connection,
    sample_id: i64,
    tag: &str,
    origin: &str,
) -> Result<()> {
    let tag = tag.trim();
    if tag.is_empty() {
        return Ok(());
    }
    conn.execute("INSERT OR IGNORE INTO tags(name) VALUES (?)", params![tag])?;
    let tag_id: i64 = conn.query_row(
        "SELECT id FROM tags WHERE name = ? COLLATE NOCASE",
        params![tag],
        |row| row.get(0),
    )?;
    conn.execute(
        "INSERT OR IGNORE INTO sample_tag_origins(sample_id, tag_id, origin) VALUES (?, ?, ?)",
        params![sample_id, tag_id, origin],
    )?;
    conn.execute(
        "INSERT OR IGNORE INTO sample_tags(sample_id, tag_id) VALUES (?, ?)",
        params![sample_id, tag_id],
    )?;
    Ok(())
}

fn refresh_sample_title(conn: &Connection, sample_id: i64) -> Result<()> {
    let title: Option<String> = conn.query_row(
        "SELECT title FROM sample_titles WHERE sample_id = ? \
         ORDER BY CASE origin WHEN 'user' THEN 0 WHEN 'factory' THEN 1 ELSE 2 END, origin COLLATE NOCASE LIMIT 1",
        params![sample_id], |row| row.get(0),
    ).optional()?;
    conn.execute(
        "UPDATE samples SET title = ? WHERE id = ?",
        params![title, sample_id],
    )?;
    Ok(())
}

fn normalize_fact_tags(tags: &[String]) -> Vec<String> {
    let mut seen = HashSet::new();
    tags.iter()
        .map(|tag| tag.trim())
        .filter(|tag| !tag.is_empty())
        .filter(|tag| seen.insert(tag.to_lowercase()))
        .map(str::to_string)
        .collect()
}

fn escape_like(value: &str) -> String {
    value
        .replace('\\', "\\\\")
        .replace('%', "\\%")
        .replace('_', "\\_")
}

fn append_origin_filter(sql: &mut String, values: &mut Vec<String>, origins: &[&str]) {
    for origin in origins
        .iter()
        .map(|origin| origin.trim())
        .filter(|origin| !origin.is_empty())
    {
        sql.push_str(" AND EXISTS (SELECT 1 FROM sample_provenance sp_filter WHERE sp_filter.sample_id = s.id AND sp_filter.origin = ?)");
        values.push(origin.to_string());
    }
}

fn sql_io_error(error: std::io::Error) -> rusqlite::Error {
    rusqlite::Error::ToSqlConversionFailure(Box::new(error))
}

fn sql_json_error(error: serde_json::Error) -> rusqlite::Error {
    rusqlite::Error::ToSqlConversionFailure(Box::new(error))
}

pub fn display_title_for_sample_path(path: &Path) -> Option<String> {
    let hash = path.file_stem()?.to_str()?.trim();
    if hash.is_empty() {
        return None;
    }

    let db_path = crate::app_paths::app_paths().sample_db_path();
    if !db_path.is_file() {
        return None;
    }

    let db = SampleDb::open(&db_path).ok()?;
    db.title_for_hash(hash).ok().flatten()
}

fn sqlite_like_contains_pattern(text: &str) -> String {
    let mut pattern = String::with_capacity(text.len() + 2);
    pattern.push('%');
    for ch in text.chars() {
        match ch {
            '%' | '_' | '\\' => {
                pattern.push('\\');
                pattern.push(ch);
            }
            _ => pattern.push(ch),
        }
    }
    pattern.push('%');
    pattern
}

fn append_browser_sample_filter(
    sql: &mut String,
    values: &mut Vec<String>,
    include_tags: &[&str],
    text: Option<&str>,
) {
    for tag in include_tags
        .iter()
        .map(|tag| tag.trim())
        .filter(|tag| !tag.is_empty())
    {
        sql.push_str(
            " AND EXISTS (\
             SELECT 1 FROM sample_tags st_filter \
             JOIN tags t_filter ON t_filter.id = st_filter.tag_id \
             WHERE st_filter.sample_id = s.id AND t_filter.name = ? COLLATE NOCASE)",
        );
        values.push(tag.to_string());
    }

    if let Some(text) = text.map(str::trim).filter(|text| !text.is_empty()) {
        let pattern = sqlite_like_contains_pattern(text);
        sql.push_str(
            " AND (COALESCE(s.title, '') COLLATE NOCASE LIKE ? ESCAPE '\\' \
             OR s.hash COLLATE NOCASE LIKE ? ESCAPE '\\' \
             OR EXISTS (\
                 SELECT 1 FROM sample_tags st_text \
                 JOIN tags t_text ON t_text.id = st_text.tag_id \
                 WHERE st_text.sample_id = s.id \
                 AND t_text.name COLLATE NOCASE LIKE ? ESCAPE '\\'))",
        );
        values.push(pattern.clone());
        values.push(pattern.clone());
        values.push(pattern);
    }
}

fn normalized_tag_set(tags: &[&str]) -> HashSet<String> {
    tags.iter()
        .map(|tag| tag.trim().to_lowercase())
        .filter(|tag| !tag.is_empty())
        .collect()
}

fn clean_display_title(title: String) -> Option<String> {
    clean_display_title_str(&title)
}

fn clean_display_title_str(title: &str) -> Option<String> {
    let without_controls: String = title.chars().filter(|ch| !ch.is_control()).collect();
    let title = without_controls.trim();
    (!title.is_empty()).then(|| title.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn insert_sample(db: &SampleDb, hash: &str, title: Option<&str>, favorited: bool) {
        db.connection()
            .execute(
                "INSERT INTO samples(hash, title, favorited) VALUES (?, ?, ?)",
                params![hash, title, if favorited { 1 } else { 0 }],
            )
            .expect("insert sample");
    }

    #[test]
    fn sample_db_schema_initializes_with_foreign_keys() {
        let db = SampleDb::open_in_memory().expect("open db");
        let foreign_keys: i64 = db
            .connection()
            .query_row("PRAGMA foreign_keys", [], |row| row.get(0))
            .expect("foreign key pragma");
        assert_eq!(foreign_keys, 1);

        let table_count: i64 = db
            .connection()
            .query_row(
                "SELECT COUNT(*) FROM sqlite_master WHERE type = 'table' AND name = 'source_refs'",
                [],
                |row| row.get(0),
            )
            .expect("source refs table");
        assert_eq!(table_count, 1);
    }

    #[test]
    fn sample_db_query_filters_tags_text_and_favorites() {
        let db = SampleDb::open_in_memory().expect("open db");
        insert_sample(&db, "aaa111", Some("Dark 909 Kick"), true);
        insert_sample(&db, "bbb222", Some("Bright Snare"), false);
        insert_sample(&db, "ccc333", Some("Dark Pad"), true);
        db.add_tag("aaa111", "drums").expect("tag");
        db.add_tag("aaa111", "dark").expect("tag");
        db.add_tag("bbb222", "drums").expect("tag");
        db.add_tag("ccc333", "instruments").expect("tag");
        db.add_tag("ccc333", "dark").expect("tag");

        let rows = db
            .query(&["dark"], &["instruments"], Some("kick"), true, &[])
            .expect("query");
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].hash, "aaa111");
        assert_eq!(rows[0].tags, vec!["dark".to_string(), "drums".to_string()]);
    }

    #[test]
    fn sample_db_browser_query_matches_tags_and_returns_adjacent_facets() {
        let db = SampleDb::open_in_memory().expect("open db");
        insert_sample(&db, "aaa111", Some("Deep Kick"), false);
        insert_sample(&db, "bbb222", Some("Round Kick"), false);
        insert_sample(&db, "ccc333", Some("Bright Snare"), false);
        db.add_tag("aaa111", "kick").expect("tag");
        db.add_tag("aaa111", "808").expect("tag");
        db.add_tag("bbb222", "kick").expect("tag");
        db.add_tag("bbb222", "909").expect("tag");
        db.add_tag("ccc333", "snare").expect("tag");
        db.add_tag("ccc333", "909").expect("tag");

        let rows = db
            .query_samples_for_browser(&["kick"], Some("808"))
            .expect("browser samples");
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].hash, "aaa111");

        let facets = db
            .adjacent_tags(&["kick"], None, 16)
            .expect("adjacent tags");
        let names: Vec<_> = facets.iter().map(|facet| facet.name.as_str()).collect();
        assert!(names.contains(&"kick"));
        assert!(names.contains(&"808"));
        assert!(names.contains(&"909"));
        assert!(facets
            .iter()
            .any(|facet| facet.name == "kick" && facet.selected && facet.count == 2));
    }

    #[test]
    fn sample_db_browser_text_search_matches_tag_names() {
        let db = SampleDb::open_in_memory().expect("open db");
        insert_sample(&db, "aaa111", Some("Plain One"), false);
        insert_sample(&db, "bbb222", Some("Plain Two"), false);
        db.add_tag("aaa111", "vocals").expect("tag");
        db.add_tag("bbb222", "kick").expect("tag");

        let rows = db
            .query_samples_for_browser(&[], Some("vocal"))
            .expect("browser samples");
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].hash, "aaa111");
    }

    #[test]
    fn sample_db_tag_mutations_are_idempotent_and_case_insensitive() {
        let db = SampleDb::open_in_memory().expect("open db");
        insert_sample(&db, "aaa111", Some("Kick"), false);

        db.add_tag("aaa111", "Drums").expect("add tag");
        db.add_tag("aaa111", "drums").expect("add duplicate tag");
        assert_eq!(
            db.list_tags().expect("list tags"),
            vec!["Drums".to_string()]
        );
        assert_eq!(db.tags_for("aaa111").expect("sample tags").len(), 1);

        db.remove_tag("aaa111", "DRUMS").expect("remove tag");
        assert!(db.tags_for("aaa111").expect("sample tags").is_empty());
    }

    #[test]
    fn sample_db_title_lookup_trims_empty_titles() {
        let db = SampleDb::open_in_memory().expect("open db");
        insert_sample(&db, "aaa111", Some("\u{0006}  Kick  \u{0007}"), false);
        insert_sample(&db, "bbb222", Some("   "), false);
        insert_sample(&db, "ccc333", None, false);

        assert_eq!(
            db.title_for_hash("aaa111").expect("title"),
            Some("Kick".to_string())
        );
        assert_eq!(db.title_for_hash("bbb222").expect("empty title"), None);
        assert_eq!(db.title_for_hash("ccc333").expect("null title"), None);
        assert_eq!(db.title_for_hash("missing").expect("missing title"), None);
    }

    #[test]
    fn sample_db_set_favorite_updates_only_target_sample() {
        let db = SampleDb::open_in_memory().expect("open db");
        insert_sample(&db, "aaa111", Some("Kick"), false);
        insert_sample(&db, "bbb222", Some("Snare"), false);

        db.set_favorite("bbb222", true).expect("favorite");
        let rows = db.query(&[], &[], None, true, &[]).expect("favorites");
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].hash, "bbb222");
        assert!(rows[0].favorited);
    }

    #[test]
    fn deleting_index_rebuilds_user_curation_from_store_and_journal() {
        let root = std::env::temp_dir().join(format!(
            "eseq-sample-db-rebuild-{}",
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        fs::create_dir_all(root.join("samples")).unwrap();
        fs::write(root.join("samples/abc123.wav"), b"stored transcode bytes").unwrap();
        let db_path = root.join("samples.db");
        {
            let db = SampleDb::open(&db_path).unwrap();
            db.append_user_fact(&UserFact::Sample {
                hash: "abc123".to_string(),
                title: None,
                tags: Vec::new(),
                favorited: false,
            })
            .unwrap();
        }
        {
            let db = SampleDb::open(&db_path).unwrap();
            db.set_title("abc123", Some("My Kick")).unwrap();
            db.add_tag("abc123", "Drums").unwrap();
            db.set_favorite("abc123", true).unwrap();
        }
        fs::remove_file(&db_path).unwrap();
        let rebuilt = SampleDb::open(&db_path).unwrap();
        let rows = rebuilt.query(&[], &[], None, false, &[]).unwrap();
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].title.as_deref(), Some("My Kick"));
        assert_eq!(rows[0].tags, vec!["Drums"]);
        assert!(rows[0].favorited);
        assert_eq!(rows[0].origins, vec!["user"]);
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn opening_existing_index_replays_facts_appended_before_a_crash() {
        let root = std::env::temp_dir().join(format!(
            "eseq-sample-db-pending-journal-{}",
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        fs::create_dir_all(root.join("samples")).unwrap();
        fs::write(root.join("samples/abc123.wav"), b"stored transcode bytes").unwrap();
        let db_path = root.join("samples.db");
        {
            let db = SampleDb::open(&db_path).unwrap();
            db.append_user_fact(&UserFact::Sample {
                hash: "abc123".to_string(),
                title: None,
                tags: Vec::new(),
                favorited: false,
            })
            .unwrap();
            db.append_user_fact(&UserFact::AddTag {
                hash: "abc123".to_string(),
                tag: "Recovered".to_string(),
            })
            .unwrap();
            assert!(db.tags_for("abc123").unwrap().is_empty());
        }
        let reopened = SampleDb::open(&db_path).unwrap();
        assert_eq!(reopened.tags_for("abc123").unwrap(), vec!["Recovered"]);
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn sample_db_query_order_is_deterministic() {
        let db = SampleDb::open_in_memory().expect("open db");
        insert_sample(&db, "ccc333", None, false);
        insert_sample(&db, "bbb222", Some("Alpha"), false);
        insert_sample(&db, "aaa111", Some("alpha"), false);

        let rows = db.query(&[], &[], None, false, &[]).expect("query");
        let hashes: Vec<_> = rows.into_iter().map(|row| row.hash).collect();
        assert_eq!(hashes, vec!["aaa111", "bbb222", "ccc333"]);
    }
}
