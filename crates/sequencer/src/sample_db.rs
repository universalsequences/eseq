use std::collections::HashSet;
use std::path::Path;

use rusqlite::{params, params_from_iter, Connection, OptionalExtension, Result};

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
}

impl SampleDb {
    pub fn open(path: &Path) -> Result<Self> {
        let conn = Connection::open(path)?;
        initialize_connection(&conn)?;
        Ok(Self { conn })
    }

    pub fn open_in_memory() -> Result<Self> {
        let conn = Connection::open_in_memory()?;
        initialize_connection(&conn)?;
        Ok(Self { conn })
    }

    pub fn initialize_schema(&self) -> Result<()> {
        initialize_connection(&self.conn)
    }

    pub fn connection(&self) -> &Connection {
        &self.conn
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
            samples.push(SampleRow {
                hash: row.hash,
                title: row.title.and_then(clean_display_title),
                favorited: row.favorited,
                tags,
            });
        }
        Ok(samples)
    }

    pub fn query_samples_for_browser(
        &self,
        include_tags: &[&str],
        text: Option<&str>,
    ) -> Result<Vec<SampleRow>> {
        let mut sql = String::from(
            "SELECT s.id, s.hash, s.title, s.favorited \
             FROM samples s WHERE 1 = 1",
        );
        let mut values = Vec::new();
        append_browser_sample_filter(&mut sql, &mut values, include_tags, text);
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
            samples.push(SampleRow {
                hash: row.hash,
                title: row.title.and_then(clean_display_title),
                favorited: row.favorited,
                tags,
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

    pub fn add_tag(&self, hash: &str, tag: &str) -> Result<()> {
        let Some(sample_id) = self.sample_id(hash)? else {
            return Ok(());
        };
        let tag = tag.trim();
        if tag.is_empty() {
            return Ok(());
        }
        self.conn
            .execute("INSERT OR IGNORE INTO tags(name) VALUES (?)", params![tag])?;
        let tag_id: i64 = self.conn.query_row(
            "SELECT id FROM tags WHERE name = ? COLLATE NOCASE",
            params![tag],
            |row| row.get(0),
        )?;
        self.conn.execute(
            "INSERT OR IGNORE INTO sample_tags(sample_id, tag_id) VALUES (?, ?)",
            params![sample_id, tag_id],
        )?;
        Ok(())
    }

    pub fn remove_tag(&self, hash: &str, tag: &str) -> Result<()> {
        let Some(sample_id) = self.sample_id(hash)? else {
            return Ok(());
        };
        self.conn.execute(
            "DELETE FROM sample_tags \
             WHERE sample_id = ? \
             AND tag_id IN (SELECT id FROM tags WHERE name = ? COLLATE NOCASE)",
            params![sample_id, tag.trim()],
        )?;
        Ok(())
    }

    pub fn set_favorite(&self, hash: &str, favorited: bool) -> Result<()> {
        self.conn.execute(
            "UPDATE samples SET favorited = ? WHERE hash = ?",
            params![if favorited { 1 } else { 0 }, hash],
        )?;
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

pub fn display_title_for_sample_path(path: &Path) -> Option<String> {
    let hash = path.file_stem()?.to_str()?.trim();
    if hash.is_empty() {
        return None;
    }

    let db_path = crate::paths::sequencer_dir().ok()?.join("samples.db");
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
            .query(&["dark"], &["instruments"], Some("kick"), true)
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
        let rows = db.query(&[], &[], None, true).expect("favorites");
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].hash, "bbb222");
        assert!(rows[0].favorited);
    }

    #[test]
    fn sample_db_query_order_is_deterministic() {
        let db = SampleDb::open_in_memory().expect("open db");
        insert_sample(&db, "ccc333", None, false);
        insert_sample(&db, "bbb222", Some("Alpha"), false);
        insert_sample(&db, "aaa111", Some("alpha"), false);

        let rows = db.query(&[], &[], None, false).expect("query");
        let hashes: Vec<_> = rows.into_iter().map(|row| row.hash).collect();
        assert_eq!(hashes, vec!["aaa111", "bbb222", "ccc333"]);
    }
}
