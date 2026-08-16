//! The TUI's state and every transition it can make.
//!
//! Deliberately knows nothing about a terminal: no drawing, no key codes,
//! no raw mode. `mod.rs` translates keystrokes into calls on this type and
//! `render.rs` reads it back out. Keeping the split means the behaviour
//! that matters — navigation, filtering, editing, saving — is testable
//! without a TTY, and the parts that need a real terminal stay small
//! enough to check by eye.

use std::path::PathBuf;

use crate::cap;
use crate::core::media::{self, IngestOptions, Sidecar};
use crate::core::story::{self, NewStory, Story, Visibility};
use crate::core::vault::Vault;
use crate::core::{clock, interview, voice};

/// A named stand-in for `()`. strictrs requires `pub fn`s to spell out an
/// explicit return type, but its checker misreads a literal `-> ()` — the
/// return type's own parentheses look like the end of the parameter list —
/// and flags it as missing one. Spelling it as a named type satisfies both
/// strictrs and clippy's `unused_unit` lint, which forbids the literal form.
type Unit = ();

/// Which collection is being browsed.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Tab {
    Stories,
    Interviews,
    Media,
}

impl Tab {
    pub const ALL: [Tab; 3] = [Tab::Stories, Tab::Interviews, Tab::Media];

    pub fn title(self) -> &'static str {
        match self {
            Tab::Stories => "Stories",
            Tab::Interviews => "Interviews",
            Tab::Media => "Media",
        }
    }

    pub fn index(self) -> usize {
        match self {
            Tab::Stories => 0,
            Tab::Interviews => 1,
            Tab::Media => 2,
        }
    }

    fn next(self) -> Tab {
        match self {
            Tab::Stories => Tab::Interviews,
            Tab::Interviews => Tab::Media,
            Tab::Media => Tab::Stories,
        }
    }

    fn previous(self) -> Tab {
        match self {
            Tab::Stories => Tab::Media,
            Tab::Interviews => Tab::Stories,
            Tab::Media => Tab::Interviews,
        }
    }
}

/// What the keyboard is currently doing.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Mode {
    /// Moving around a list.
    Browse,
    /// Typing into the filter box.
    Filter,
    /// Filling in the new-story form.
    Add,
    /// Showing the key reference.
    Help,
}

/// A field of the new-story form.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Field {
    /// Where the cursor starts in a fresh form.
    #[default]
    Title,
    Date,
    People,
    Places,
    Tags,
    Visibility,
    Body,
}

impl Field {
    pub const ALL: [Field; 7] = [
        Field::Title,
        Field::Date,
        Field::People,
        Field::Places,
        Field::Tags,
        Field::Visibility,
        Field::Body,
    ];

    pub fn label(self) -> &'static str {
        match self {
            Field::Title => "Title",
            Field::Date => "Date",
            Field::People => "People",
            Field::Places => "Places",
            Field::Tags => "Tags",
            Field::Visibility => "Visibility",
            Field::Body => "Story",
        }
    }

    /// A hint shown beside the field, so the accepted forms are visible
    /// at the moment of typing rather than in documentation elsewhere.
    pub fn hint(self) -> &'static str {
        match self {
            Field::Title => "required",
            Field::Date => "YYYY-MM-DD, YYYY-MM, YYYY, 1970s, or blank",
            Field::People => "comma separated slugs",
            Field::Places => "comma separated slugs",
            Field::Tags => "comma separated",
            Field::Visibility => "left/right to change",
            Field::Body => "their own words; ctrl-r records",
        }
    }

    fn next(self) -> Field {
        match self {
            Field::Title => Field::Date,
            Field::Date => Field::People,
            Field::People => Field::Places,
            Field::Places => Field::Tags,
            Field::Tags => Field::Visibility,
            Field::Visibility => Field::Body,
            Field::Body => Field::Title,
        }
    }

    fn previous(self) -> Field {
        match self {
            Field::Title => Field::Body,
            Field::Date => Field::Title,
            Field::People => Field::Date,
            Field::Places => Field::People,
            Field::Tags => Field::Places,
            Field::Visibility => Field::Tags,
            Field::Body => Field::Visibility,
        }
    }
}

/// How prominently to show a status message.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Level {
    Info,
    Success,
    Warning,
    Error,
}

#[derive(Debug, Clone)]
pub struct Status {
    pub message: String,
    pub level: Level,
}

impl Status {
    fn info(message: &str) -> Status {
        Status {
            message: message.to_owned(),
            level: Level::Info,
        }
    }
}

/// The new-story form.
#[derive(Debug, Clone, Default)]
pub struct Draft {
    pub title: String,
    pub date: String,
    pub people: String,
    pub places: String,
    pub tags: String,
    pub visibility: Visibility,
    pub body: String,
    pub field: Field,
    /// Recordings already ingested for this draft, as archive-relative
    /// paths. They are attached to the story when it is saved.
    pub media: Vec<String>,
}

impl Draft {
    /// The text currently being edited.
    fn buffer_mut(&mut self) -> Option<&mut String> {
        match self.field {
            Field::Title => Some(&mut self.title),
            Field::Date => Some(&mut self.date),
            Field::People => Some(&mut self.people),
            Field::Places => Some(&mut self.places),
            Field::Tags => Some(&mut self.tags),
            Field::Body => Some(&mut self.body),
            // Visibility cycles with the arrow keys rather than typing.
            Field::Visibility => None,
        }
    }

    pub fn value(&self, field: Field) -> &str {
        match field {
            Field::Title => &self.title,
            Field::Date => &self.date,
            Field::People => &self.people,
            Field::Places => &self.places,
            Field::Tags => &self.tags,
            Field::Body => &self.body,
            Field::Visibility => self.visibility.as_str(),
        }
    }

    fn split_list(raw: &str) -> Vec<String> {
        raw.split(',')
            .map(str::trim)
            .filter(|part| !part.is_empty())
            .map(str::to_owned)
            .collect()
    }

    /// Turn the form into a story request, or explain what is missing.
    pub fn to_request(&self) -> Result<NewStory, String> {
        if self.title.trim().is_empty() {
            return Err("a story needs a title".to_owned());
        }
        let date = self.date.trim();
        Ok(NewStory {
            title: self.title.trim().to_owned(),
            date: if date.is_empty() {
                None
            } else {
                Some(date.to_owned())
            },
            body: self.body.clone(),
            people: Draft::split_list(&self.people),
            places: Draft::split_list(&self.places),
            tags: Draft::split_list(&self.tags),
            media: self.media.clone(),
            visibility: Some(self.visibility),
            recorded_at: Some(clock::now_timestamp()),
            ..NewStory::default()
        })
    }
}

/// A row in the interviews list.
#[derive(Debug, Clone)]
pub struct InterviewRow {
    pub session_id: String,
    pub bank: String,
    pub status: String,
    pub mode: String,
    pub answered: usize,
    pub skipped: usize,
    pub transcript: String,
}

/// A row in the media list.
#[derive(Debug, Clone)]
pub struct MediaRow {
    pub relative_path: String,
    pub sidecar: Option<Sidecar>,
}

/// A voice take in progress.
struct Take {
    recording: cap::process::Recording,
    wav_path: PathBuf,
    /// Held so the scratch directory outlives the recording.
    _scratch: tempfile::TempDir,
}

pub struct App {
    pub vault: Vault,
    pub stories: Vec<Story>,
    pub interviews: Vec<InterviewRow>,
    pub media: Vec<MediaRow>,
    pub tab: Tab,
    pub mode: Mode,
    pub filter: String,
    pub draft: Draft,
    pub status: Status,
    pub should_quit: bool,
    /// Indices into the active collection that survive the filter.
    visible: Vec<usize>,
    selected: usize,
    take: Option<Take>,
}

impl App {
    pub fn new(vault: Vault) -> Result<App, String> {
        let mut app = App {
            vault,
            stories: Vec::new(),
            interviews: Vec::new(),
            media: Vec::new(),
            tab: Tab::Stories,
            mode: Mode::Browse,
            filter: String::new(),
            draft: Draft::default(),
            status: Status::info("? for keys"),
            should_quit: false,
            visible: Vec::new(),
            selected: 0,
            take: None,
        };
        app.reload()?;
        Ok(app)
    }

    /// Re-read everything from disk. The archive is the source of truth,
    /// so the TUI never caches across an edit.
    pub fn reload(&mut self) -> Result<(), String> {
        let mut stories = story::iter_stories(&self.vault.root).map_err(|error| error.0)?;
        stories.sort_by(|left, right| {
            left.fuzzy_date()
                .sort_key()
                .cmp(&right.fuzzy_date().sort_key())
                .then_with(|| left.id.cmp(&right.id))
        });
        self.stories = stories;
        self.interviews = self.load_interviews();
        self.media = self.load_media();
        self.recompute_visible();
        Ok(())
    }

    fn load_interviews(&self) -> Vec<InterviewRow> {
        let directory = self.vault.interviews_dir();
        let Ok(entries) = cap::fs::read_dir_sorted(&directory) else {
            return Vec::new();
        };
        let mut rows = Vec::new();
        for path in entries {
            if path.extension().and_then(|value| value.to_str()) != Some("md") {
                continue;
            }
            let Some(stem) = path.file_stem().and_then(|value| value.to_str()) else {
                continue;
            };
            let Ok(session) = interview::load_session(&self.vault.root, stem) else {
                continue;
            };
            rows.push(InterviewRow {
                session_id: session.session_id.clone(),
                bank: session.bank.clone(),
                status: session.status.as_str().to_owned(),
                mode: session.mode.as_str().to_owned(),
                answered: session.answered.len(),
                skipped: session.skipped.len(),
                transcript: session.transcript.clone(),
            });
        }
        rows
    }

    fn load_media(&self) -> Vec<MediaRow> {
        let directory = self.vault.media_dir();
        let Ok(files) = cap::fs::walk_files(&directory) else {
            return Vec::new();
        };
        let mut rows = Vec::new();
        for path in files {
            if path.extension().and_then(|value| value.to_str()) == Some("yaml") {
                continue;
            }
            let Ok(relative) = path.strip_prefix(&self.vault.root) else {
                continue;
            };
            let sidecar = media::load_sidecar(&media::sidecar_path_for(&path)).ok();
            rows.push(MediaRow {
                relative_path: relative
                    .components()
                    .filter_map(|part| part.as_os_str().to_str())
                    .collect::<Vec<&str>>()
                    .join("/"),
                sidecar,
            });
        }
        rows
    }

    /// How many rows the active tab holds, before filtering.
    fn row_count(&self) -> usize {
        match self.tab {
            Tab::Stories => self.stories.len(),
            Tab::Interviews => self.interviews.len(),
            Tab::Media => self.media.len(),
        }
    }

    /// The searchable text of one row of the active tab.
    fn haystack(&self, index: usize) -> String {
        match self.tab {
            Tab::Stories => match self.stories.get(index) {
                Some(story) => format!(
                    "{} {} {} {} {} {} {}",
                    story.id,
                    story.title,
                    story.body,
                    story.tags.join(" "),
                    story.people.join(" "),
                    story.places.join(" "),
                    story.date.clone().unwrap_or_default()
                ),
                None => String::new(),
            },
            Tab::Interviews => match self.interviews.get(index) {
                Some(row) => format!(
                    "{} {} {} {}",
                    row.session_id, row.bank, row.status, row.transcript
                ),
                None => String::new(),
            },
            Tab::Media => match self.media.get(index) {
                Some(row) => {
                    let extra = row.sidecar.as_ref().map_or_else(String::new, |sidecar| {
                        format!(
                            "{} {} {}",
                            sidecar.original_filename,
                            sidecar.date.clone().unwrap_or_default(),
                            sidecar.caption.clone().unwrap_or_default()
                        )
                    });
                    format!("{} {extra}", row.relative_path)
                }
                None => String::new(),
            },
        }
    }

    fn recompute_visible(&mut self) {
        let needle = self.filter.trim().to_lowercase();
        self.visible = (0..self.row_count())
            .filter(|index| {
                needle.is_empty() || self.haystack(*index).to_lowercase().contains(&needle)
            })
            .collect();
        if self.selected >= self.visible.len() {
            self.selected = self.visible.len().saturating_sub(1);
        }
    }

    /// Row indices currently shown, in order.
    pub fn visible(&self) -> &[usize] {
        &self.visible
    }

    /// Position within the visible list.
    pub fn selected_position(&self) -> usize {
        self.selected
    }

    /// Index into the underlying collection, or `None` when nothing matches.
    pub fn selected_index(&self) -> Option<usize> {
        self.visible.get(self.selected).copied()
    }

    pub fn selected_story(&self) -> Option<&Story> {
        match self.tab {
            Tab::Stories => self
                .selected_index()
                .and_then(|index| self.stories.get(index)),
            Tab::Interviews | Tab::Media => None,
        }
    }

    pub fn selected_interview(&self) -> Option<&InterviewRow> {
        match self.tab {
            Tab::Interviews => self
                .selected_index()
                .and_then(|index| self.interviews.get(index)),
            Tab::Stories | Tab::Media => None,
        }
    }

    pub fn selected_media(&self) -> Option<&MediaRow> {
        match self.tab {
            Tab::Media => self
                .selected_index()
                .and_then(|index| self.media.get(index)),
            Tab::Stories | Tab::Interviews => None,
        }
    }

    // ------------------------------------------------------- navigation --

    pub fn select_next(&mut self) -> Unit {
        if self.visible.is_empty() {
            return;
        }
        self.selected = (self.selected + 1) % self.visible.len();
    }

    pub fn select_previous(&mut self) -> Unit {
        if self.visible.is_empty() {
            return;
        }
        self.selected = match self.selected.checked_sub(1) {
            Some(previous) => previous,
            None => self.visible.len().saturating_sub(1),
        };
    }

    pub fn select_first(&mut self) -> Unit {
        self.selected = 0;
    }

    pub fn select_last(&mut self) -> Unit {
        self.selected = self.visible.len().saturating_sub(1);
    }

    /// Move by a screenful, used by PageUp/PageDown.
    pub fn select_page(&mut self, rows: usize, forward: bool) -> Unit {
        if self.visible.is_empty() {
            return;
        }
        let last = self.visible.len().saturating_sub(1);
        self.selected = if forward {
            self.selected.saturating_add(rows).min(last)
        } else {
            self.selected.saturating_sub(rows)
        };
    }

    pub fn next_tab(&mut self) -> Unit {
        self.tab = self.tab.next();
        self.selected = 0;
        self.recompute_visible();
    }

    pub fn previous_tab(&mut self) -> Unit {
        self.tab = self.tab.previous();
        self.selected = 0;
        self.recompute_visible();
    }

    // ----------------------------------------------------------- filter --

    pub fn begin_filter(&mut self) -> Unit {
        self.mode = Mode::Filter;
    }

    pub fn push_filter(&mut self, character: char) -> Unit {
        self.filter.push(character);
        self.selected = 0;
        self.recompute_visible();
    }

    pub fn pop_filter(&mut self) -> Unit {
        self.filter.pop();
        self.selected = 0;
        self.recompute_visible();
    }

    pub fn clear_filter(&mut self) -> Unit {
        self.filter.clear();
        self.selected = 0;
        self.recompute_visible();
    }

    pub fn end_filter(&mut self) -> Unit {
        self.mode = Mode::Browse;
    }

    // ------------------------------------------------------------- help --

    pub fn toggle_help(&mut self) -> Unit {
        self.mode = match self.mode {
            Mode::Help => Mode::Browse,
            Mode::Browse => Mode::Help,
            Mode::Filter | Mode::Add => self.mode,
        };
    }

    // -------------------------------------------------------------- add --

    pub fn begin_add(&mut self) -> Unit {
        self.draft = Draft::default();
        self.mode = Mode::Add;
        self.status = Status::info("tab moves between fields, ctrl-s saves, esc cancels");
    }

    /// Leave the form. Returns whether anything was actually discarded, so
    /// the caller can tell the difference between an empty form and lost
    /// typing.
    pub fn cancel_add(&mut self) -> bool {
        let had_content = !self.draft.title.trim().is_empty()
            || !self.draft.body.trim().is_empty()
            || !self.draft.media.is_empty();
        self.draft = Draft::default();
        self.mode = Mode::Browse;
        had_content
    }

    pub fn next_field(&mut self) -> Unit {
        self.draft.field = self.draft.field.next();
    }

    pub fn previous_field(&mut self) -> Unit {
        self.draft.field = self.draft.field.previous();
    }

    pub fn cycle_visibility(&mut self, forward: bool) -> Unit {
        self.draft.visibility = match (self.draft.visibility, forward) {
            (Visibility::Public, true) => Visibility::Family,
            (Visibility::Family, true) => Visibility::ExecutorOnly,
            (Visibility::ExecutorOnly, true) => Visibility::Public,
            (Visibility::Public, false) => Visibility::ExecutorOnly,
            (Visibility::Family, false) => Visibility::Public,
            (Visibility::ExecutorOnly, false) => Visibility::Family,
        };
    }

    pub fn type_char(&mut self, character: char) -> Unit {
        if let Some(buffer) = self.draft.buffer_mut() {
            buffer.push(character);
        }
    }

    pub fn backspace(&mut self) -> Unit {
        if let Some(buffer) = self.draft.buffer_mut() {
            buffer.pop();
        }
    }

    /// Newlines only make sense in the prose body; elsewhere Enter moves on.
    pub fn newline(&mut self) -> Unit {
        match self.draft.field {
            Field::Body => self.draft.body.push('\n'),
            Field::Title
            | Field::Date
            | Field::People
            | Field::Places
            | Field::Tags
            | Field::Visibility => self.next_field(),
        }
    }

    /// Write the drafted story to disk and return to browsing.
    pub fn save_draft(&mut self) -> Result<String, String> {
        let request = self.draft.to_request()?;
        let built = story::new_story(request).map_err(|error| error.0)?;
        let path = story::save_story(&self.vault.root, &built, false).map_err(|error| error.0)?;

        self.draft = Draft::default();
        self.mode = Mode::Browse;
        self.reload()?;

        // Land the cursor on what was just written, so the save is visible
        // rather than merely reported.
        if let Some(position) = self
            .visible
            .iter()
            .position(|index| self.stories.get(*index).is_some_and(|s| s.id == built.id))
        {
            self.tab = Tab::Stories;
            self.selected = position;
        }

        Ok(path
            .strip_prefix(&self.vault.root)
            .unwrap_or(&path)
            .display()
            .to_string())
    }

    // ------------------------------------------------------------ voice --

    pub fn is_recording(&self) -> bool {
        self.take.is_some()
    }

    /// Begin a voice take for the current draft.
    pub fn start_voice(&mut self) -> Result<(), String> {
        if self.take.is_some() {
            return Err("already recording".to_owned());
        }
        let scratch = cap::fs::temp_dir()
            .map_err(|error| format!("failed to create scratch directory: {error}"))?;
        let wav_path = scratch.path().join("take.wav");
        let recording = voice::start_recording(&wav_path).map_err(|error| error.0)?;
        self.take = Some(Take {
            recording,
            wav_path,
            _scratch: scratch,
        });
        Ok(())
    }

    /// Stop the take, archive the audio, then transcribe it.
    ///
    /// The audio is ingested into `media/` *before* transcription is
    /// attempted, so a failed or unconfigured transcription still leaves
    /// the recording safely in the archive. Losing what someone said is
    /// far worse than lacking a text copy of it.
    pub fn stop_voice(&mut self) -> Result<String, String> {
        let Some(take) = self.take.take() else {
            return Err("not recording".to_owned());
        };
        voice::finish_recording(take.recording, &take.wav_path).map_err(|error| error.0)?;

        let options = IngestOptions {
            manual_date: Some(self.draft.date.trim().to_owned()).filter(|d| !d.is_empty()),
            visibility: Some(self.draft.visibility),
            ..IngestOptions::default()
        };
        let ingested = media::ingest_file(&take.wav_path, &self.vault.media_dir(), &options)
            .map_err(|error| error.0)?;

        let relative = ingested
            .destination
            .strip_prefix(&self.vault.root)
            .unwrap_or(&ingested.destination)
            .components()
            .filter_map(|part| part.as_os_str().to_str())
            .collect::<Vec<&str>>()
            .join("/");
        self.draft.media.push(relative.clone());

        let settings = voice::VoiceSettings::from_env();
        match voice::transcribe(&settings, &ingested.destination) {
            Ok(text) => {
                if !self.draft.body.is_empty() && !self.draft.body.ends_with('\n') {
                    self.draft.body.push('\n');
                }
                self.draft.body.push_str(text.trim());
                Ok(format!(
                    "transcribed into the story; audio kept at {relative}"
                ))
            }
            Err(error) => Err(format!(
                "audio saved to {relative}, but transcription failed: {}",
                error.0
            )),
        }
    }

    pub fn set_status(&mut self, message: &str, level: Level) -> Unit {
        self.status = Status {
            message: message.to_owned(),
            level,
        };
    }

    pub fn quit(&mut self) -> Unit {
        self.should_quit = true;
    }
}

#[cfg(test)]
mod tests {
    use super::{App, Field, Level, Mode, Tab};
    use crate::cap;
    use crate::core::story::{new_story, save_story, NewStory, Visibility};
    use crate::core::vault::Vault;

    fn seeded() -> (tempfile::TempDir, App) {
        let temp = tempfile::TempDir::new().expect("temp dir");
        let vault = Vault::init(&temp.path().join("arch"), "Jane Doe", 2, 3).expect("init");
        for (title, date, body, tags) in [
            (
                "Starting university",
                Some("1994-09-15"),
                "I packed one suitcase.",
                "education",
            ),
            (
                "Childhood home",
                Some("1970s"),
                "We lived by the sea.",
                "childhood",
            ),
            ("Undated memory", None, "No idea when this was.", ""),
        ] {
            let story = new_story(NewStory {
                title: title.to_owned(),
                date: date.map(str::to_owned),
                body: body.to_owned(),
                tags: if tags.is_empty() {
                    Vec::new()
                } else {
                    vec![tags.to_owned()]
                },
                ..NewStory::default()
            })
            .expect("story");
            save_story(&vault.root, &story, false).expect("save");
        }
        let app = App::new(vault).expect("app");
        (temp, app)
    }

    #[test]
    fn loads_stories_in_chronological_order() {
        let (_temp, app) = seeded();
        let titles: Vec<&str> = app.stories.iter().map(|s| s.title.as_str()).collect();
        assert_eq!(
            titles,
            vec!["Childhood home", "Starting university", "Undated memory"],
            "undated stories sort last, as they do in timeline.md"
        );
    }

    #[test]
    fn navigation_wraps_in_both_directions() {
        let (_temp, mut app) = seeded();
        assert_eq!(app.selected_position(), 0);
        app.select_next();
        app.select_next();
        assert_eq!(app.selected_position(), 2);
        app.select_next();
        assert_eq!(app.selected_position(), 0, "wraps past the end");
        app.select_previous();
        assert_eq!(app.selected_position(), 2, "wraps before the start");
    }

    #[test]
    fn first_last_and_paging_stay_in_bounds() {
        let (_temp, mut app) = seeded();
        app.select_last();
        assert_eq!(app.selected_position(), 2);
        app.select_page(100, true);
        assert_eq!(app.selected_position(), 2, "cannot page past the last row");
        app.select_page(100, false);
        assert_eq!(app.selected_position(), 0, "cannot page before the first");
        app.select_first();
        assert_eq!(app.selected_position(), 0);
    }

    #[test]
    fn filtering_narrows_and_clears() {
        let (_temp, mut app) = seeded();
        assert_eq!(app.visible().len(), 3);

        for character in "suitcase".chars() {
            app.push_filter(character);
        }
        assert_eq!(app.visible().len(), 1);
        assert_eq!(
            app.selected_story().map(|s| s.title.as_str()),
            Some("Starting university"),
            "the filter searches body text, not just titles"
        );

        app.clear_filter();
        assert_eq!(app.visible().len(), 3);
    }

    #[test]
    fn filtering_is_case_insensitive_and_matches_tags() {
        let (_temp, mut app) = seeded();
        for character in "EDUCATION".chars() {
            app.push_filter(character);
        }
        assert_eq!(app.visible().len(), 1);
    }

    #[test]
    fn a_filter_matching_nothing_leaves_no_selection() {
        let (_temp, mut app) = seeded();
        for character in "zzzznothing".chars() {
            app.push_filter(character);
        }
        assert!(app.visible().is_empty());
        assert_eq!(app.selected_index(), None);
        assert!(app.selected_story().is_none());
        // Navigating an empty list must not panic or select a phantom row.
        app.select_next();
        app.select_previous();
        assert_eq!(app.selected_index(), None);
    }

    #[test]
    fn backspacing_the_filter_widens_it_again() {
        let (_temp, mut app) = seeded();
        for character in "suitcaseX".chars() {
            app.push_filter(character);
        }
        assert!(app.visible().is_empty());
        app.pop_filter();
        assert_eq!(app.visible().len(), 1);
    }

    #[test]
    fn tabs_cycle_and_reset_the_selection() {
        let (_temp, mut app) = seeded();
        app.select_last();
        app.next_tab();
        assert_eq!(app.tab, Tab::Interviews);
        assert_eq!(app.selected_position(), 0);
        app.next_tab();
        assert_eq!(app.tab, Tab::Media);
        app.next_tab();
        assert_eq!(app.tab, Tab::Stories, "wraps back around");
        app.previous_tab();
        assert_eq!(app.tab, Tab::Media);
    }

    #[test]
    fn selection_accessors_respect_the_active_tab() {
        let (_temp, mut app) = seeded();
        assert!(app.selected_story().is_some());
        assert!(app.selected_interview().is_none());
        app.next_tab();
        assert!(app.selected_story().is_none(), "not a story tab any more");
    }

    #[test]
    fn the_form_writes_a_story_that_reads_back() {
        let (_temp, mut app) = seeded();
        app.begin_add();
        assert_eq!(app.mode, Mode::Add);

        for character in "A new story".chars() {
            app.type_char(character);
        }
        app.next_field();
        for character in "2001-05-04".chars() {
            app.type_char(character);
        }
        app.draft.field = Field::Tags;
        for character in "one, two".chars() {
            app.type_char(character);
        }
        app.draft.field = Field::Body;
        for character in "Body text.".chars() {
            app.type_char(character);
        }

        let written = app.save_draft().expect("saves");
        assert_eq!(written, "timeline/2001/2001-05-04-a-new-story.md");
        assert_eq!(app.mode, Mode::Browse);

        let saved = app
            .stories
            .iter()
            .find(|s| s.id == "2001-05-04-a-new-story")
            .expect("story is loaded after save");
        assert_eq!(saved.tags, vec!["one".to_owned(), "two".to_owned()]);
        assert_eq!(saved.body.trim(), "Body text.");
        assert_eq!(saved.visibility, Visibility::Family);
        assert!(saved.recorded_at.is_some());
    }

    #[test]
    fn saving_moves_the_cursor_to_the_new_story() {
        let (_temp, mut app) = seeded();
        app.begin_add();
        for character in "Zzz last".chars() {
            app.type_char(character);
        }
        app.draft.field = Field::Date;
        for character in "2020".chars() {
            app.type_char(character);
        }
        app.save_draft().expect("saves");
        assert_eq!(
            app.selected_story().map(|s| s.id.as_str()),
            Some("2020-zzz-last"),
            "the cursor lands on what was just written"
        );
    }

    #[test]
    fn a_titleless_draft_is_refused_without_touching_disk() {
        let (_temp, mut app) = seeded();
        let before = app.stories.len();
        app.begin_add();
        app.draft.field = Field::Body;
        for character in "orphan text".chars() {
            app.type_char(character);
        }
        assert!(app.save_draft().is_err());
        assert_eq!(app.stories.len(), before);
        assert_eq!(app.mode, Mode::Add, "the form stays open to be fixed");
    }

    #[test]
    fn an_unparseable_date_is_refused_rather_than_guessed() {
        let (_temp, mut app) = seeded();
        app.begin_add();
        for character in "Some title".chars() {
            app.type_char(character);
        }
        app.draft.field = Field::Date;
        for character in "sometime in the 90s".chars() {
            app.type_char(character);
        }
        let error = app.save_draft().expect_err("should refuse");
        assert!(error.contains("Unrecognized date"));
    }

    #[test]
    fn saving_a_duplicate_id_is_refused() {
        let (_temp, mut app) = seeded();
        app.begin_add();
        for character in "Childhood home".chars() {
            app.type_char(character);
        }
        app.draft.field = Field::Date;
        for character in "1970s".chars() {
            app.type_char(character);
        }
        let error = app.save_draft().expect_err("should refuse");
        assert!(error.contains("already exists"), "got: {error}");
    }

    #[test]
    fn cancelling_reports_whether_anything_was_lost() {
        let (_temp, mut app) = seeded();
        app.begin_add();
        assert!(!app.cancel_add(), "an untouched form discards nothing");

        app.begin_add();
        for character in "typed".chars() {
            app.type_char(character);
        }
        assert!(app.cancel_add(), "typing means there is something to lose");
        assert_eq!(app.mode, Mode::Browse);
    }

    #[test]
    fn enter_adds_a_newline_only_in_the_body() {
        let (_temp, mut app) = seeded();
        app.begin_add();
        app.newline();
        assert_eq!(
            app.draft.field,
            Field::Date,
            "Enter advances in a one-line field"
        );

        app.draft.field = Field::Body;
        app.newline();
        assert_eq!(app.draft.body, "\n");
        assert_eq!(app.draft.field, Field::Body, "and stays put in the body");
    }

    #[test]
    fn visibility_cycles_both_ways_and_never_types() {
        let (_temp, mut app) = seeded();
        app.begin_add();
        app.draft.field = Field::Visibility;

        app.type_char('x');
        assert_eq!(
            app.draft.visibility,
            Visibility::Family,
            "typing does nothing"
        );

        app.cycle_visibility(true);
        assert_eq!(app.draft.visibility, Visibility::ExecutorOnly);
        app.cycle_visibility(true);
        assert_eq!(app.draft.visibility, Visibility::Public);
        app.cycle_visibility(false);
        assert_eq!(app.draft.visibility, Visibility::ExecutorOnly);
    }

    #[test]
    fn fields_cycle_in_a_stable_order() {
        let (_temp, mut app) = seeded();
        app.begin_add();
        let mut seen = vec![app.draft.field];
        for _step in 0..Field::ALL.len() {
            app.next_field();
            seen.push(app.draft.field);
        }
        assert_eq!(
            seen.first(),
            seen.last(),
            "a full cycle returns to the start"
        );
        assert_eq!(app.draft.field, Field::Title);
        // Stepping back from the first field wraps to the last.
        app.previous_field();
        assert_eq!(app.draft.field, Field::Body);
    }

    #[test]
    fn backspace_edits_the_focused_field_only() {
        let (_temp, mut app) = seeded();
        app.begin_add();
        for character in "abc".chars() {
            app.type_char(character);
        }
        app.backspace();
        assert_eq!(app.draft.title, "ab");
        app.next_field();
        app.backspace();
        assert_eq!(
            app.draft.title, "ab",
            "an empty field cannot eat the previous one"
        );
    }

    #[test]
    fn help_toggles_only_from_browsing() {
        let (_temp, mut app) = seeded();
        app.toggle_help();
        assert_eq!(app.mode, Mode::Help);
        app.toggle_help();
        assert_eq!(app.mode, Mode::Browse);

        app.begin_add();
        app.toggle_help();
        assert_eq!(app.mode, Mode::Add, "help must not interrupt a draft");
    }

    #[test]
    fn reload_picks_up_a_story_written_behind_the_ui() {
        let (_temp, mut app) = seeded();
        let before = app.stories.len();
        let story = new_story(NewStory {
            title: "Written elsewhere".to_owned(),
            date: Some("1999".to_owned()),
            ..NewStory::default()
        })
        .expect("story");
        save_story(&app.vault.root, &story, false).expect("save");

        app.reload().expect("reload");
        assert_eq!(app.stories.len(), before + 1);
    }

    #[test]
    fn media_and_interview_tabs_read_the_archive() {
        let (_temp, mut app) = seeded();

        cap::fs::write_bytes(
            &app.vault.media_dir().join("1994").join("photo.jpg"),
            b"jpeg",
        )
        .expect("write");
        let mut session = crate::core::interview::new_session(
            &app.vault.root,
            "childhood",
            crate::core::interview::SessionMode::Text,
        );
        crate::core::interview::save_session(&app.vault.root, &mut session).expect("save");

        app.reload().expect("reload");
        assert_eq!(app.media.len(), 1);
        assert_eq!(app.interviews.len(), 1);

        app.next_tab();
        assert!(app.selected_interview().is_some());
        app.next_tab();
        assert_eq!(
            app.selected_media().map(|row| row.relative_path.as_str()),
            Some("media/1994/photo.jpg")
        );
    }

    #[test]
    fn stopping_a_recording_that_never_started_is_an_error_not_a_panic() {
        let (_temp, mut app) = seeded();
        assert!(!app.is_recording());
        assert!(app.stop_voice().is_err());
    }

    #[test]
    fn status_messages_carry_a_level() {
        let (_temp, mut app) = seeded();
        app.set_status("something went wrong", Level::Error);
        assert_eq!(app.status.level, Level::Error);
        assert_eq!(app.status.message, "something went wrong");
    }
}
