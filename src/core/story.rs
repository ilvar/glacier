//! Story files: markdown with YAML frontmatter, the atomic unit of the archive.
//!
//! A story is read and written as a single `.md` file. The frontmatter
//! schema below is intentionally small and stable — see the top-level
//! README for the rationale. Do not add fields without updating README.md
//! and the schema version in archive.yaml.

use std::fmt;
use std::path::{Path, PathBuf};

use crate::cap;
use crate::core::dates::{from_frontmatter, parse_fuzzy_date, FuzzyDate, Precision};
use crate::core::yaml::{self, Emitter};

pub const VISIBILITIES: [&str; 3] = ["public", "family", "executor-only"];

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Visibility {
    Public,
    /// The default for a new story: most of a life is family material,
    /// and widening it later is a deliberate act.
    #[default]
    Family,
    ExecutorOnly,
}

impl Visibility {
    pub fn as_str(self) -> &'static str {
        match self {
            Visibility::Public => "public",
            Visibility::Family => "family",
            Visibility::ExecutorOnly => "executor-only",
        }
    }

    pub fn parse(raw: &str) -> Result<Visibility, StoryError> {
        match raw {
            "public" => Ok(Visibility::Public),
            "family" => Ok(Visibility::Family),
            "executor-only" => Ok(Visibility::ExecutorOnly),
            other => Err(StoryError(format!(
                "invalid visibility {other:?}, must be one of {VISIBILITIES:?}"
            ))),
        }
    }
}

impl fmt::Display for Visibility {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(self.as_str())
    }
}

#[derive(Debug, PartialEq, Eq)]
pub struct StoryError(pub String);

impl fmt::Display for StoryError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.0)
    }
}

impl std::error::Error for StoryError {}

/// Lowercase, hyphen-separated slug used for ids and filenames.
pub fn slugify(text: &str) -> String {
    let mut slug = String::new();
    let mut pending_separator = false;
    for character in text.trim().to_lowercase().chars() {
        if character.is_ascii_alphanumeric() {
            if pending_separator && !slug.is_empty() {
                slug.push('-');
            }
            pending_separator = false;
            slug.push(character);
        } else {
            pending_separator = true;
        }
    }
    slug
}

#[derive(Debug, Clone)]
pub struct Story {
    pub id: String,
    pub title: String,
    pub body: String,
    pub date: Option<String>,
    pub date_precision: Precision,
    pub people: Vec<String>,
    pub places: Vec<String>,
    pub tags: Vec<String>,
    pub media: Vec<String>,
    pub source: Option<String>,
    pub recorded_at: Option<String>,
    /// `None` when a human wrote the tags, otherwise the model id. This
    /// line is never blurred.
    pub tags_generated_by: Option<String>,
    pub visibility: Visibility,
}

/// Everything a caller may vary when creating a story. Grouped into a
/// struct so adding a field later does not churn every call site.
#[derive(Debug, Clone, Default)]
pub struct NewStory {
    pub title: String,
    pub date: Option<String>,
    pub body: String,
    pub people: Vec<String>,
    pub places: Vec<String>,
    pub tags: Vec<String>,
    pub media: Vec<String>,
    pub source: Option<String>,
    pub recorded_at: Option<String>,
    pub visibility: Option<Visibility>,
    /// Override the derived id. Used by the interview subsystem, which
    /// names stories after the session and question they came from.
    pub id: Option<String>,
}

impl Story {
    pub fn fuzzy_date(&self) -> FuzzyDate {
        from_frontmatter(self.date.as_deref(), Some(self.date_precision.as_str()))
    }

    /// `timeline/<year>/<id>.md`, or `timeline/undated/<id>.md`.
    pub fn relative_path(&self) -> PathBuf {
        let bucket = match self.fuzzy_date().year() {
            Some(year) => year.to_string(),
            None => "undated".to_owned(),
        };
        Path::new("timeline")
            .join(bucket)
            .join(format!("{}.md", self.id))
    }

    /// The complete file contents: frontmatter, then the prose body.
    pub fn render(&self) -> String {
        let mut emitter = Emitter::new();
        emitter
            .string("id", &self.id)
            .string("title", &self.title)
            .optional_string("date", self.date.as_deref())
            .string("date_precision", self.date_precision.as_str())
            .list("people", &self.people)
            .list("places", &self.places)
            .list("tags", &self.tags)
            .list("media", &self.media)
            .optional_string("source", self.source.as_deref())
            .optional_string("recorded_at", self.recorded_at.as_deref())
            .optional_string("tags_generated_by", self.tags_generated_by.as_deref())
            .string("visibility", self.visibility.as_str());

        let body = self.body.trim_matches('\n');
        format!("---\n{}---\n\n{body}\n", emitter.finish())
    }
}

/// Build a story, deriving its id from the date and a slug of the title.
pub fn new_story(request: NewStory) -> Result<Story, StoryError> {
    let fuzzy = parse_fuzzy_date(request.date.as_deref()).map_err(|error| StoryError(error.0))?;
    let slug = slugify(&request.title);
    if slug.is_empty() {
        return Err(StoryError(
            "title must contain at least one alphanumeric character".to_owned(),
        ));
    }
    let id = match request.id {
        Some(id) => id,
        None => match fuzzy.value.as_deref() {
            Some(value) => format!("{value}-{slug}"),
            None => slug,
        },
    };

    Ok(Story {
        id,
        title: request.title,
        body: request.body,
        date: fuzzy.value,
        date_precision: fuzzy.precision,
        people: request.people,
        places: request.places,
        tags: request.tags,
        media: request.media,
        source: request.source,
        recorded_at: request.recorded_at,
        tags_generated_by: None,
        visibility: request.visibility.unwrap_or(Visibility::Family),
    })
}

/// Split a story file into its frontmatter block and prose body.
fn split_frontmatter(text: &str) -> Result<(&str, &str), StoryError> {
    let rest = text
        .strip_prefix("---\n")
        .ok_or_else(|| StoryError("file does not contain a --- frontmatter block".to_owned()))?;
    let (frontmatter, body) = rest.split_once("\n---\n").ok_or_else(|| {
        StoryError("file does not contain a closing --- frontmatter delimiter".to_owned())
    })?;
    Ok((frontmatter, body))
}

pub fn parse_story(text: &str) -> Result<Story, StoryError> {
    let (frontmatter, body) = split_frontmatter(text)?;
    let document = yaml::parse(frontmatter).map_err(StoryError)?;

    let id = yaml::get_string(&document, "id")
        .ok_or_else(|| StoryError("missing required frontmatter field: id".to_owned()))?;
    let title = yaml::get_string(&document, "title")
        .ok_or_else(|| StoryError("missing required frontmatter field: title".to_owned()))?;

    let date = yaml::get_string(&document, "date");
    let precision = yaml::get_string(&document, "date_precision");
    let fuzzy = from_frontmatter(date.as_deref(), precision.as_deref());

    let visibility = match yaml::get_string(&document, "visibility") {
        Some(raw) => Visibility::parse(&raw)?,
        None => Visibility::Family,
    };

    Ok(Story {
        id,
        title,
        body: body.to_owned(),
        date: fuzzy.value,
        date_precision: fuzzy.precision,
        people: yaml::get_list(&document, "people"),
        places: yaml::get_list(&document, "places"),
        tags: yaml::get_list(&document, "tags"),
        media: yaml::get_list(&document, "media"),
        source: yaml::get_string(&document, "source"),
        recorded_at: yaml::get_string(&document, "recorded_at"),
        tags_generated_by: yaml::get_string(&document, "tags_generated_by"),
        visibility,
    })
}

pub fn load_story(path: &Path) -> Result<Story, StoryError> {
    let text = cap::fs::read_to_string(path)
        .map_err(|error| StoryError(format!("failed to read {}: {error}", path.display())))?;
    parse_story(&text)
}

pub fn save_story(
    archive_root: &Path,
    story: &Story,
    overwrite: bool,
) -> Result<PathBuf, StoryError> {
    let path = archive_root.join(story.relative_path());
    if cap::fs::exists(&path) && !overwrite {
        return Err(StoryError(format!(
            "story {:?} already exists at {}",
            story.id,
            path.display()
        )));
    }
    cap::fs::write(&path, &story.render())
        .map_err(|error| StoryError(format!("failed to write {}: {error}", path.display())))?;
    Ok(path)
}

/// Every story in the archive, ordered by path so results are deterministic.
pub fn iter_stories(archive_root: &Path) -> Result<Vec<Story>, StoryError> {
    let timeline = archive_root.join("timeline");
    if !cap::fs::is_dir(&timeline) {
        return Ok(Vec::new());
    }
    let files = cap::fs::walk_files(&timeline)
        .map_err(|error| StoryError(format!("failed to walk timeline: {error}")))?;

    let mut stories = Vec::new();
    for path in files {
        if path.extension().and_then(|extension| extension.to_str()) == Some("md") {
            stories.push(load_story(&path)?);
        }
    }
    Ok(stories)
}

#[cfg(test)]
mod tests {
    use super::{
        iter_stories, load_story, new_story, parse_story, save_story, slugify, NewStory, Visibility,
    };
    use crate::core::dates::Precision;
    use std::path::Path;

    fn draft(title: &str, date: Option<&str>) -> NewStory {
        NewStory {
            title: title.to_owned(),
            date: date.map(str::to_owned),
            ..NewStory::default()
        }
    }

    #[test]
    fn slugify_handles_punctuation_and_spacing() {
        assert_eq!(slugify("Starting university"), "starting-university");
        assert_eq!(slugify("  Dad's  Boat!  "), "dad-s-boat");
        assert_eq!(slugify("---"), "");
    }

    #[test]
    fn id_derives_from_date_and_title() {
        let story =
            new_story(draft("Started university", Some("1994-09-15"))).expect("story should build");
        assert_eq!(story.id, "1994-09-15-started-university");
        assert_eq!(story.date.as_deref(), Some("1994-09-15"));
        assert_eq!(story.date_precision, Precision::Day);
        assert_eq!(story.visibility, Visibility::Family);
        assert_eq!(story.tags_generated_by, None);
    }

    #[test]
    fn undated_story_keeps_a_bare_slug() {
        let story = new_story(draft("Dad and the fishing trip", None)).expect("builds");
        assert_eq!(story.id, "dad-and-the-fishing-trip");
        assert_eq!(story.date, None);
        assert_eq!(story.date_precision, Precision::Unknown);
    }

    #[test]
    fn rejects_a_title_with_no_alphanumerics() {
        assert!(new_story(draft("   ", None)).is_err());
    }

    #[test]
    fn relative_path_buckets_by_year() {
        let dated = new_story(draft("Anecdote", Some("1978-03"))).expect("builds");
        assert_eq!(
            dated.relative_path(),
            Path::new("timeline/1978/1978-03-anecdote.md")
        );

        let undated = new_story(draft("Undated thing", None)).expect("builds");
        assert_eq!(
            undated.relative_path(),
            Path::new("timeline/undated/undated-thing.md")
        );
    }

    #[test]
    fn round_trips_through_disk() {
        let temp = tempfile::TempDir::new().expect("temp dir");
        let root = temp.path();
        let story = new_story(NewStory {
            title: "Starting university".to_owned(),
            date: Some("1994-09-15".to_owned()),
            body: "I packed one suitcase.".to_owned(),
            people: vec!["mary-oconnor".to_owned()],
            places: vec!["dublin".to_owned()],
            tags: vec!["education".to_owned(), "leaving-home".to_owned()],
            ..NewStory::default()
        })
        .expect("builds");

        let path = save_story(root, &story, false).expect("saves");
        let loaded = load_story(&path).expect("loads");

        assert_eq!(loaded.id, story.id);
        assert_eq!(loaded.title, story.title);
        assert_eq!(loaded.date, story.date);
        assert_eq!(loaded.date_precision, story.date_precision);
        assert_eq!(loaded.people, vec!["mary-oconnor".to_owned()]);
        assert_eq!(loaded.places, vec!["dublin".to_owned()]);
        assert_eq!(loaded.tags.len(), 2);
        assert_eq!(loaded.visibility, Visibility::Family);
        assert!(loaded.body.contains("I packed one suitcase."));

        let raw = crate::cap::fs::read_to_string(&path).expect("reads");
        assert!(raw.contains("tags_generated_by: null"));
        // The date must be quoted so no reader coerces it to a timestamp.
        assert!(raw.contains("date: '1994-09-15'"));
    }

    #[test]
    fn refuses_to_overwrite_unless_asked() {
        let temp = tempfile::TempDir::new().expect("temp dir");
        let story = new_story(draft("Once only", None)).expect("builds");
        save_story(temp.path(), &story, false).expect("first save");
        assert!(save_story(temp.path(), &story, false).is_err());
        save_story(temp.path(), &story, true).expect("overwrite is allowed");
    }

    #[test]
    fn rejects_unknown_visibility_on_read() {
        let text = "---\nid: x\ntitle: X\nvisibility: top-secret\n---\n\nBody\n";
        assert!(parse_story(text).is_err());
    }

    #[test]
    fn rejects_a_file_with_no_frontmatter() {
        assert!(parse_story("just some text, no frontmatter").is_err());
    }

    #[test]
    fn iterates_every_story() {
        let temp = tempfile::TempDir::new().expect("temp dir");
        let root = temp.path();
        for (title, date) in [
            ("One", Some("1990")),
            ("Two", Some("1995")),
            ("Three", None),
        ] {
            let story = new_story(draft(title, date)).expect("builds");
            save_story(root, &story, false).expect("saves");
        }
        let mut ids: Vec<String> = iter_stories(root)
            .expect("iterates")
            .into_iter()
            .map(|story| story.id)
            .collect();
        ids.sort();
        assert_eq!(ids, vec!["1990-one", "1995-two", "three"]);
    }
}
