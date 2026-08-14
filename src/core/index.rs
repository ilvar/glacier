//! A rebuildable SQLite FTS5 index over story files.
//!
//! This is never the source of truth — it can be deleted at any time and
//! rebuilt from the Markdown files under `timeline/` with
//! `legacy timeline build`. It exists so the replica's retrieval step does
//! not have to re-parse every file on every query.

use std::fmt;
use std::path::Path;

use rusqlite::Connection;

use crate::cap;
use crate::core::story::{iter_stories, Story, Visibility};

const SCHEMA: &str = "
CREATE VIRTUAL TABLE IF NOT EXISTS stories USING fts5(
    id UNINDEXED,
    title,
    body,
    tags,
    people,
    places,
    date UNINDEXED,
    year UNINDEXED,
    visibility UNINDEXED
);
";

#[derive(Debug)]
pub struct IndexError(pub String);

impl fmt::Display for IndexError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.0)
    }
}

impl std::error::Error for IndexError {}

fn to_index_error(error: rusqlite::Error) -> IndexError {
    IndexError(format!("search index error: {error}"))
}

/// One search result. `snippet` marks the matched terms with brackets.
#[derive(Debug, Clone)]
pub struct Hit {
    pub id: String,
    pub title: String,
    pub date: Option<String>,
    pub visibility: String,
    pub snippet: String,
}

/// Rebuild the index from scratch. Returns how many stories were indexed.
pub fn rebuild_index(archive_root: &Path, db_path: &Path) -> Result<usize, IndexError> {
    if let Some(parent) = db_path.parent() {
        cap::fs::create_dir_all(parent)
            .map_err(|error| IndexError(format!("failed to create index directory: {error}")))?;
    }
    if cap::fs::exists(db_path) {
        cap::fs::remove_file(db_path)
            .map_err(|error| IndexError(format!("failed to remove stale index: {error}")))?;
    }

    let stories = iter_stories(archive_root).map_err(|error| IndexError(error.0))?;

    let connection = Connection::open(db_path).map_err(to_index_error)?;
    connection.execute_batch(SCHEMA).map_err(to_index_error)?;
    connection
        .execute_batch("BEGIN TRANSACTION")
        .map_err(to_index_error)?;
    for story in &stories {
        insert(&connection, story)?;
    }
    connection.execute_batch("COMMIT").map_err(to_index_error)?;
    Ok(stories.len())
}

fn insert(connection: &Connection, story: &Story) -> Result<(), IndexError> {
    connection
        .execute(
            "INSERT INTO stories (id, title, body, tags, people, places, date, year, visibility)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)",
            rusqlite::params![
                story.id,
                story.title,
                story.body,
                story.tags.join(" "),
                story.people.join(" "),
                story.places.join(" "),
                story.date.clone().unwrap_or_default(),
                story.fuzzy_date().year().unwrap_or(0),
                story.visibility.as_str(),
            ],
        )
        .map_err(to_index_error)?;
    Ok(())
}

/// FTS5 treats characters like `?`, `(`, `"`, and `*` as query syntax, so
/// a plain user question ending in a question mark is a syntax error
/// rather than a search. Strip everything that is not a word character or
/// whitespace before matching. Bare AND/OR/NOT keywords survive, which is
/// what `llm::retrieval_query` relies on.
pub fn sanitize_query(query: &str) -> String {
    let cleaned: String = query
        .chars()
        .map(|character| {
            if character.is_alphanumeric() || character == '_' || character.is_whitespace() {
                character
            } else {
                ' '
            }
        })
        .collect();
    let collapsed = cleaned.split_whitespace().collect::<Vec<&str>>().join(" ");
    if collapsed.is_empty() {
        query.to_owned()
    } else {
        collapsed
    }
}

/// Full-text search. `visibilities` restricts results to those tiers, which
/// is how the replica is prevented from ever seeing executor-only material.
pub fn search(
    db_path: &Path,
    query: &str,
    visibilities: Option<&[Visibility]>,
    limit: u32,
) -> Result<Vec<Hit>, IndexError> {
    if !cap::fs::exists(db_path) {
        return Err(IndexError(format!(
            "no index at {}; run `legacy timeline build` first",
            db_path.display()
        )));
    }
    let connection = Connection::open(db_path).map_err(to_index_error)?;

    let mut sql = String::from(
        "SELECT id, title, date, visibility, snippet(stories, 2, '[', ']', '...', 12) AS snippet \
         FROM stories WHERE stories MATCH ?1",
    );
    let mut parameters: Vec<String> = vec![sanitize_query(query)];

    if let Some(visibilities) = visibilities {
        let placeholders: Vec<String> = visibilities
            .iter()
            .enumerate()
            .map(|(offset, _)| format!("?{}", offset + 2))
            .collect();
        sql.push_str(&format!(" AND visibility IN ({})", placeholders.join(",")));
        for visibility in visibilities {
            parameters.push(visibility.as_str().to_owned());
        }
    }
    sql.push_str(&format!(" ORDER BY rank LIMIT {limit}"));

    let mut statement = connection.prepare(&sql).map_err(to_index_error)?;
    let bound: Vec<&dyn rusqlite::ToSql> = parameters
        .iter()
        .map(|value| -> &dyn rusqlite::ToSql { value })
        .collect();

    let rows = statement
        .query_map(bound.as_slice(), |row| {
            Ok(Hit {
                id: row.get(0)?,
                title: row.get(1)?,
                date: row.get::<usize, String>(2).ok().filter(|d| !d.is_empty()),
                visibility: row.get(3)?,
                snippet: row.get(4)?,
            })
        })
        .map_err(to_index_error)?;

    let mut hits = Vec::new();
    for hit in rows {
        hits.push(hit.map_err(to_index_error)?);
    }
    Ok(hits)
}

#[cfg(test)]
mod tests {
    use super::{rebuild_index, sanitize_query, search};
    use crate::core::story::{new_story, save_story, NewStory, Visibility};
    use crate::core::vault::Vault;

    fn seeded() -> (tempfile::TempDir, Vault) {
        let temp = tempfile::TempDir::new().expect("temp dir");
        let vault = Vault::init(&temp.path().join("arch"), "Jane Doe", 2, 3).expect("init");
        for (title, date, body, visibility) in [
            (
                "Starting university",
                Some("1994-09-15"),
                "I packed one suitcase.",
                Visibility::Family,
            ),
            (
                "Childhood home",
                Some("1970s"),
                "We lived by the sea.",
                Visibility::Public,
            ),
            (
                "Executor note",
                None,
                "The safe combination is written down.",
                Visibility::ExecutorOnly,
            ),
        ] {
            let story = new_story(NewStory {
                title: title.to_owned(),
                date: date.map(str::to_owned),
                body: body.to_owned(),
                visibility: Some(visibility),
                ..NewStory::default()
            })
            .expect("story");
            save_story(&vault.root, &story, false).expect("save");
        }
        (temp, vault)
    }

    #[test]
    fn indexes_every_story() {
        let (_temp, vault) = seeded();
        let count = rebuild_index(&vault.root, &vault.index_db_path()).expect("rebuild");
        assert_eq!(count, 3);
    }

    #[test]
    fn finds_a_story_by_body_text() {
        let (_temp, vault) = seeded();
        rebuild_index(&vault.root, &vault.index_db_path()).expect("rebuild");
        let hits = search(&vault.index_db_path(), "suitcase", None, 10).expect("search");
        assert!(hits
            .iter()
            .any(|hit| hit.id == "1994-09-15-starting-university"));
    }

    #[test]
    fn rebuilding_is_idempotent() {
        let (_temp, vault) = seeded();
        rebuild_index(&vault.root, &vault.index_db_path()).expect("first");
        let count = rebuild_index(&vault.root, &vault.index_db_path()).expect("second");
        assert_eq!(count, 3);
        let hits = search(&vault.index_db_path(), "suitcase", None, 10).expect("search");
        assert_eq!(hits.len(), 1, "no duplicate rows after a rebuild");
    }

    #[test]
    fn tolerates_punctuation_in_a_query() {
        // FTS5 would treat a bare `?` as syntax and raise; a user question
        // must search rather than fail.
        let (_temp, vault) = seeded();
        rebuild_index(&vault.root, &vault.index_db_path()).expect("rebuild");
        let hits = search(&vault.index_db_path(), "suitcase?", None, 10).expect("search");
        assert_eq!(hits.len(), 1);

        search(&vault.index_db_path(), "\"unbalanced", None, 10).expect("no syntax error");
        search(&vault.index_db_path(), "a AND (b", None, 10).expect("no syntax error");
    }

    #[test]
    fn visibility_filter_hides_executor_only_material() {
        let (_temp, vault) = seeded();
        rebuild_index(&vault.root, &vault.index_db_path()).expect("rebuild");

        let unrestricted =
            search(&vault.index_db_path(), "safe OR combination", None, 10).expect("search");
        assert_eq!(unrestricted.len(), 1, "the story is present in the index");

        let restricted = search(
            &vault.index_db_path(),
            "safe OR combination",
            Some(&[Visibility::Public, Visibility::Family]),
            10,
        )
        .expect("search");
        assert!(
            restricted.is_empty(),
            "executor-only material must never be returned to the replica"
        );
    }

    #[test]
    fn missing_index_is_an_actionable_error() {
        let temp = tempfile::TempDir::new().expect("temp dir");
        let error = search(&temp.path().join("absent.sqlite3"), "anything", None, 10)
            .expect_err("should fail");
        assert!(error.0.contains("timeline build"));
    }

    #[test]
    fn sanitizer_keeps_words_and_boolean_keywords() {
        assert_eq!(
            sanitize_query("What did you do after school?"),
            "What did you do after school"
        );
        assert_eq!(sanitize_query("school OR work"), "school OR work");
        assert_eq!(sanitize_query("  spaced   out  "), "spaced out");
        // An all-punctuation query has nothing to strip down to, so it is
        // passed through rather than becoming an empty match-everything.
        assert_eq!(sanitize_query("???"), "???");
    }
}
