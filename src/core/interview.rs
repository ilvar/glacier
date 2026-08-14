//! Interview subsystem: question banks and resumable session state.
//!
//! Every answer becomes a story file the moment it is given — see
//! `record_answer`, which writes the story before it updates the session —
//! so a crash mid-session never loses an answer that was already spoken.
//! The question banks work entirely offline; an LLM may only ever *suggest*
//! a follow-up, and that path is optional.

use std::fmt;
use std::path::{Path, PathBuf};

use crate::cap;
use crate::core::clock;
use crate::core::story::{self, NewStory, Story, Visibility};
use crate::core::yaml::{self, Emitter};

/// The banks compiled into the binary. Users can point at their own YAML
/// with `--bank-dir`; these are the defaults so a fresh install can start
/// an interview with no setup.
const BUILTIN_BANKS: [(&str, &str); 10] = [
    (
        "childhood",
        include_str!("../templates/interviews/childhood.yaml"),
    ),
    (
        "family-and-origins",
        include_str!("../templates/interviews/family-and-origins.yaml"),
    ),
    (
        "school",
        include_str!("../templates/interviews/school.yaml"),
    ),
    ("work", include_str!("../templates/interviews/work.yaml")),
    (
        "love-and-partnership",
        include_str!("../templates/interviews/love-and-partnership.yaml"),
    ),
    (
        "parenthood",
        include_str!("../templates/interviews/parenthood.yaml"),
    ),
    (
        "beliefs-and-values",
        include_str!("../templates/interviews/beliefs-and-values.yaml"),
    ),
    (
        "turning-points",
        include_str!("../templates/interviews/turning-points.yaml"),
    ),
    (
        "advice-and-messages",
        include_str!("../templates/interviews/advice-and-messages.yaml"),
    ),
    (
        "objects-and-places",
        include_str!("../templates/interviews/objects-and-places.yaml"),
    ),
];

#[derive(Debug)]
pub struct InterviewError(pub String);

impl fmt::Display for InterviewError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.0)
    }
}

impl std::error::Error for InterviewError {}

#[derive(Debug, Clone)]
pub struct Question {
    pub id: String,
    pub prompt: String,
    pub followups: Vec<String>,
    pub tags: Vec<String>,
}

#[derive(Debug, Clone)]
pub struct QuestionBank {
    pub name: String,
    pub description: String,
    pub questions: Vec<Question>,
}

impl QuestionBank {
    pub fn question(&self, id: &str) -> Option<&Question> {
        self.questions.iter().find(|question| question.id == id)
    }

    pub fn parse(text: &str) -> Result<QuestionBank, InterviewError> {
        let document = yaml::parse(text).map_err(InterviewError)?;
        let name = yaml::get_string(&document, "name")
            .ok_or_else(|| InterviewError("question bank has no name".to_owned()))?;
        let description = yaml::get_string(&document, "description").unwrap_or_default();

        let mut questions = Vec::new();
        if let Some(serde_yaml_ng::Value::Sequence(items)) = document.get("questions") {
            for item in items {
                let Some(id) = yaml::get_string(item, "id") else {
                    continue;
                };
                let Some(prompt) = yaml::get_string(item, "prompt") else {
                    continue;
                };
                questions.push(Question {
                    id,
                    prompt,
                    followups: yaml::get_list(item, "followups"),
                    tags: yaml::get_list(item, "tags"),
                });
            }
        }
        Ok(QuestionBank {
            name,
            description,
            questions,
        })
    }
}

/// Names of every bank available, built-in plus any in `extra_dir`.
pub fn list_banks(extra_dir: Option<&Path>) -> Vec<String> {
    let mut names: Vec<String> = BUILTIN_BANKS
        .iter()
        .map(|(name, _)| (*name).to_owned())
        .collect();

    if let Some(directory) = extra_dir {
        if let Ok(entries) = cap::fs::read_dir_sorted(directory) {
            for path in entries {
                if path.extension().and_then(|e| e.to_str()) != Some("yaml") {
                    continue;
                }
                if let Some(stem) = path.file_stem().and_then(|s| s.to_str()) {
                    if !names.iter().any(|existing| existing == stem) {
                        names.push(stem.to_owned());
                    }
                }
            }
        }
    }
    names.sort();
    names
}

/// Load a bank by name. A user-supplied file of the same name takes
/// precedence over the built-in one, so the shipped questions can be
/// edited without patching the binary.
pub fn load_bank(name: &str, extra_dir: Option<&Path>) -> Result<QuestionBank, InterviewError> {
    if let Some(directory) = extra_dir {
        let path = directory.join(format!("{name}.yaml"));
        if cap::fs::is_file(&path) {
            let text = cap::fs::read_to_string(&path).map_err(|error| {
                InterviewError(format!("failed to read {}: {error}", path.display()))
            })?;
            return QuestionBank::parse(&text);
        }
    }

    for (bank_name, text) in BUILTIN_BANKS {
        if bank_name == name {
            return QuestionBank::parse(text);
        }
    }

    let available = list_banks(extra_dir).join(", ");
    Err(InterviewError(format!(
        "no question bank named {name:?}. Available: {available}"
    )))
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SessionStatus {
    InProgress,
    Complete,
}

impl SessionStatus {
    pub fn as_str(self) -> &'static str {
        match self {
            SessionStatus::InProgress => "in_progress",
            SessionStatus::Complete => "complete",
        }
    }

    pub fn from_str_lossy(raw: &str) -> SessionStatus {
        match raw {
            "complete" => SessionStatus::Complete,
            _otherwise => SessionStatus::InProgress,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SessionMode {
    Text,
    Voice,
}

impl SessionMode {
    pub fn as_str(self) -> &'static str {
        match self {
            SessionMode::Text => "text",
            SessionMode::Voice => "voice",
        }
    }

    pub fn from_str_lossy(raw: &str) -> SessionMode {
        match raw {
            "voice" => SessionMode::Voice,
            _otherwise => SessionMode::Text,
        }
    }
}

#[derive(Debug, Clone)]
pub struct Session {
    pub session_id: String,
    pub bank: String,
    pub mode: SessionMode,
    pub status: SessionStatus,
    pub started_at: String,
    pub updated_at: String,
    pub answered: Vec<String>,
    pub skipped: Vec<String>,
    /// question id -> story id, so the transcript and the stories it
    /// produced can always be matched back up.
    pub stories: Vec<(String, String)>,
    pub transcript: String,
}

impl Session {
    pub fn render(&self) -> String {
        let mut emitter = Emitter::new();
        emitter
            .string("session_id", &self.session_id)
            .string("bank", &self.bank)
            .string("mode", self.mode.as_str())
            .string("status", self.status.as_str())
            .string("started_at", &self.started_at)
            .string("updated_at", &self.updated_at)
            .list("answered", &self.answered)
            .list("skipped", &self.skipped)
            .string_map("stories", &self.stories);

        let body = self.transcript.trim_matches('\n');
        format!("---\n{}---\n\n{body}\n", emitter.finish())
    }

    pub fn parse(text: &str) -> Result<Session, InterviewError> {
        let rest = text.strip_prefix("---\n").ok_or_else(|| {
            InterviewError("session file is missing its frontmatter block".to_owned())
        })?;
        let (frontmatter, body) = rest.split_once("\n---\n").ok_or_else(|| {
            InterviewError("session file is missing its closing frontmatter delimiter".to_owned())
        })?;
        let document = yaml::parse(frontmatter).map_err(InterviewError)?;

        Ok(Session {
            session_id: yaml::get_string(&document, "session_id")
                .ok_or_else(|| InterviewError("session file has no session_id".to_owned()))?,
            bank: yaml::get_string(&document, "bank")
                .ok_or_else(|| InterviewError("session file has no bank".to_owned()))?,
            mode: yaml::get_string(&document, "mode")
                .map_or(SessionMode::Text, |raw| SessionMode::from_str_lossy(&raw)),
            status: yaml::get_string(&document, "status")
                .map_or(SessionStatus::InProgress, |raw| {
                    SessionStatus::from_str_lossy(&raw)
                }),
            started_at: yaml::get_string(&document, "started_at")
                .unwrap_or_else(clock::now_timestamp),
            updated_at: yaml::get_string(&document, "updated_at")
                .unwrap_or_else(clock::now_timestamp),
            answered: yaml::get_list(&document, "answered"),
            skipped: yaml::get_list(&document, "skipped"),
            stories: yaml::get_string_map(&document, "stories"),
            transcript: body.to_owned(),
        })
    }

    /// The story id recorded for a question, if it was answered.
    pub fn story_for(&self, question_id: &str) -> Option<&str> {
        self.stories
            .iter()
            .find(|(question, _)| question == question_id)
            .map(|(_, story)| story.as_str())
    }
}

pub fn session_path(archive_root: &Path, session_id: &str) -> PathBuf {
    archive_root
        .join("interviews")
        .join(format!("{session_id}.md"))
}

/// Allocate a session id of the form `<date>-<bank>-session-NN`, skipping
/// any that already exist so two sittings in one day never collide.
pub fn new_session_id(archive_root: &Path, bank_name: &str) -> String {
    let date = clock::today();
    let mut counter = 1_u32;
    loop {
        let candidate = format!("{date}-{bank_name}-session-{counter:02}");
        if !cap::fs::exists(&session_path(archive_root, &candidate)) {
            return candidate;
        }
        counter = counter.saturating_add(1);
    }
}

pub fn new_session(archive_root: &Path, bank_name: &str, mode: SessionMode) -> Session {
    let now = clock::now_timestamp();
    Session {
        session_id: new_session_id(archive_root, bank_name),
        bank: bank_name.to_owned(),
        mode,
        status: SessionStatus::InProgress,
        started_at: now.clone(),
        updated_at: now,
        answered: Vec::new(),
        skipped: Vec::new(),
        stories: Vec::new(),
        transcript: String::new(),
    }
}

pub fn save_session(archive_root: &Path, session: &mut Session) -> Result<PathBuf, InterviewError> {
    session.updated_at = clock::now_timestamp();
    let path = session_path(archive_root, &session.session_id);
    cap::fs::write(&path, &session.render())
        .map_err(|error| InterviewError(format!("failed to write session file: {error}")))?;
    Ok(path)
}

pub fn load_session(archive_root: &Path, session_id: &str) -> Result<Session, InterviewError> {
    let path = session_path(archive_root, session_id);
    if !cap::fs::exists(&path) {
        return Err(InterviewError(format!(
            "no interview session {session_id:?} at {}",
            path.display()
        )));
    }
    let text = cap::fs::read_to_string(&path)
        .map_err(|error| InterviewError(format!("failed to read session file: {error}")))?;
    Session::parse(&text)
}

/// The next unanswered, unskipped question, or `None` when the bank is done.
pub fn next_question<'bank>(
    bank: &'bank QuestionBank,
    session: &Session,
) -> Option<&'bank Question> {
    bank.questions.iter().find(|question| {
        !session.answered.iter().any(|id| id == &question.id)
            && !session.skipped.iter().any(|id| id == &question.id)
    })
}

/// Save an answer as a story file, then update the session.
///
/// The order matters and is the whole reliability story of this module: the
/// story file is written and flushed first, so a crash between the two
/// writes costs at most a line of bookkeeping, never the answer itself.
pub fn record_answer(
    archive_root: &Path,
    session: &mut Session,
    question: &Question,
    answer_text: &str,
    visibility: Visibility,
) -> Result<Story, InterviewError> {
    let answer = answer_text.trim();
    if answer.is_empty() {
        return Err(InterviewError("cannot record an empty answer".to_owned()));
    }

    // The session id starts with the date the session began.
    let session_date: String = session.session_id.chars().take(10).collect();
    // A long prompt makes an unwieldy title, so fall back to the question id.
    let title = if question.prompt.chars().count() <= 60 {
        question.prompt.clone()
    } else {
        question.id.replace('-', " ")
    };

    let story = story::new_story(NewStory {
        title,
        date: Some(session_date),
        body: answer.to_owned(),
        tags: question.tags.clone(),
        source: Some(format!("interview:{}", session.session_id)),
        recorded_at: Some(clock::now_timestamp()),
        visibility: Some(visibility),
        id: Some(format!("{}-{}", session.session_id, question.id)),
        ..NewStory::default()
    })
    .map_err(|error| InterviewError(error.0))?;

    story::save_story(archive_root, &story, true).map_err(|error| InterviewError(error.0))?;

    if !session.answered.iter().any(|id| id == &question.id) {
        session.answered.push(question.id.clone());
    }
    session.skipped.retain(|id| id != &question.id);
    session
        .stories
        .retain(|(question_id, _)| question_id != &question.id);
    session
        .stories
        .push((question.id.clone(), story.id.clone()));
    session
        .transcript
        .push_str(&format!("\n## {}\n\n{answer}\n", question.prompt));

    save_session(archive_root, session)?;
    Ok(story)
}

pub fn skip_question(
    archive_root: &Path,
    session: &mut Session,
    question: &Question,
) -> Result<(), InterviewError> {
    let already_handled = session.skipped.iter().any(|id| id == &question.id)
        || session.answered.iter().any(|id| id == &question.id);
    if !already_handled {
        session.skipped.push(question.id.clone());
    }
    session
        .transcript
        .push_str(&format!("\n## {}\n\n(skipped)\n", question.prompt));
    save_session(archive_root, session)?;
    Ok(())
}

pub fn mark_complete(archive_root: &Path, session: &mut Session) -> Result<(), InterviewError> {
    session.status = SessionStatus::Complete;
    save_session(archive_root, session)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::{
        list_banks, load_bank, load_session, mark_complete, new_session, next_question,
        record_answer, save_session, skip_question, SessionMode, SessionStatus,
    };
    use crate::core::story::{load_story, Visibility};
    use crate::core::vault::Vault;

    fn archive() -> (tempfile::TempDir, Vault) {
        let temp = tempfile::TempDir::new().expect("temp dir");
        let vault = Vault::init(&temp.path().join("arch"), "Jane Doe", 2, 3).expect("init");
        (temp, vault)
    }

    #[test]
    fn ships_every_required_bank() {
        let banks = list_banks(None);
        for required in [
            "childhood",
            "family-and-origins",
            "school",
            "work",
            "love-and-partnership",
            "parenthood",
            "beliefs-and-values",
            "turning-points",
            "advice-and-messages",
            "objects-and-places",
        ] {
            assert!(
                banks.iter().any(|name| name == required),
                "missing {required}"
            );
        }
    }

    #[test]
    fn every_shipped_bank_parses_and_has_questions() {
        for name in list_banks(None) {
            let bank = load_bank(&name, None).expect("bank parses");
            assert!(!bank.questions.is_empty(), "{name} has no questions");
            assert!(!bank.description.is_empty(), "{name} has no description");
            for question in &bank.questions {
                assert!(!question.id.is_empty());
                assert!(!question.prompt.is_empty());
            }
        }
    }

    #[test]
    fn loads_a_named_question() {
        let bank = load_bank("childhood", None).expect("bank");
        let question = bank.question("earliest-memory").expect("question");
        assert!(!question.prompt.is_empty());
        assert!(!question.followups.is_empty());
    }

    #[test]
    fn an_unknown_bank_lists_the_available_ones() {
        let error = load_bank("does-not-exist", None).expect_err("should fail");
        assert!(error.0.contains("childhood"));
    }

    #[test]
    fn a_user_supplied_bank_overrides_a_builtin() {
        let temp = tempfile::TempDir::new().expect("temp dir");
        let directory = temp.path();
        crate::cap::fs::write(
            &directory.join("childhood.yaml"),
            "name: childhood\ndescription: My own questions\nquestions:\n  - id: mine\n    prompt: My question?\n",
        )
        .expect("write");

        let bank = load_bank("childhood", Some(directory)).expect("bank");
        assert_eq!(bank.description, "My own questions");
        assert_eq!(bank.questions.len(), 1);
    }

    #[test]
    fn an_answer_becomes_a_story_immediately() {
        let (_temp, vault) = archive();
        let bank = load_bank("childhood", None).expect("bank");
        let mut session = new_session(&vault.root, "childhood", SessionMode::Text);
        save_session(&vault.root, &mut session).expect("save");

        let question = next_question(&bank, &session).expect("a question").clone();
        let story = record_answer(
            &vault.root,
            &mut session,
            &question,
            "I remember the garden.",
            Visibility::Family,
        )
        .expect("record");

        let path = vault.root.join(story.relative_path());
        let written = load_story(&path).expect("story on disk");
        assert_eq!(written.body.trim(), "I remember the garden.");
        assert_eq!(
            written.source.as_deref(),
            Some(format!("interview:{}", session.session_id).as_str())
        );
        assert_eq!(written.tags, question.tags);
        assert!(session.answered.iter().any(|id| id == &question.id));
    }

    #[test]
    fn a_session_resumes_where_it_left_off() {
        let (_temp, vault) = archive();
        let bank = load_bank("childhood", None).expect("bank");
        let mut session = new_session(&vault.root, "childhood", SessionMode::Text);
        save_session(&vault.root, &mut session).expect("save");

        let first = next_question(&bank, &session).expect("question").clone();
        record_answer(
            &vault.root,
            &mut session,
            &first,
            "An answer.",
            Visibility::Family,
        )
        .expect("record");

        // Reload from disk, as a fresh process would after a crash.
        let reloaded = load_session(&vault.root, &session.session_id).expect("reload");
        assert_eq!(reloaded.answered, session.answered);
        assert!(reloaded.story_for(&first.id).is_some());

        let second = next_question(&bank, &reloaded).expect("next question");
        assert_ne!(second.id, first.id);
    }

    #[test]
    fn a_skipped_question_creates_no_story_and_is_not_asked_again() {
        let (_temp, vault) = archive();
        let bank = load_bank("childhood", None).expect("bank");
        let mut session = new_session(&vault.root, "childhood", SessionMode::Text);

        let question = next_question(&bank, &session).expect("question").clone();
        skip_question(&vault.root, &mut session, &question).expect("skip");

        assert!(session.skipped.iter().any(|id| id == &question.id));
        assert!(session.story_for(&question.id).is_none());

        let reloaded = load_session(&vault.root, &session.session_id).expect("reload");
        let next = next_question(&bank, &reloaded).expect("next");
        assert_ne!(next.id, question.id);
    }

    #[test]
    fn an_empty_answer_is_refused() {
        let (_temp, vault) = archive();
        let bank = load_bank("childhood", None).expect("bank");
        let mut session = new_session(&vault.root, "childhood", SessionMode::Text);
        let question = next_question(&bank, &session).expect("question").clone();

        assert!(record_answer(
            &vault.root,
            &mut session,
            &question,
            "   ",
            Visibility::Family
        )
        .is_err());
    }

    #[test]
    fn completion_is_recorded() {
        let (_temp, vault) = archive();
        let mut session = new_session(&vault.root, "childhood", SessionMode::Text);
        save_session(&vault.root, &mut session).expect("save");
        mark_complete(&vault.root, &mut session).expect("complete");

        let reloaded = load_session(&vault.root, &session.session_id).expect("reload");
        assert_eq!(reloaded.status, SessionStatus::Complete);
    }

    #[test]
    fn two_sessions_on_one_day_get_distinct_ids() {
        let (_temp, vault) = archive();
        let mut first = new_session(&vault.root, "childhood", SessionMode::Text);
        save_session(&vault.root, &mut first).expect("save");
        let second = new_session(&vault.root, "childhood", SessionMode::Text);
        assert_ne!(first.session_id, second.session_id);
    }

    #[test]
    fn the_transcript_records_answers_and_skips() {
        let (_temp, vault) = archive();
        let bank = load_bank("childhood", None).expect("bank");
        let mut session = new_session(&vault.root, "childhood", SessionMode::Text);

        let first = next_question(&bank, &session).expect("question").clone();
        record_answer(
            &vault.root,
            &mut session,
            &first,
            "Told a story.",
            Visibility::Family,
        )
        .expect("record");
        let second = next_question(&bank, &session).expect("question").clone();
        skip_question(&vault.root, &mut session, &second).expect("skip");

        let reloaded = load_session(&vault.root, &session.session_id).expect("reload");
        assert!(reloaded.transcript.contains("Told a story."));
        assert!(reloaded.transcript.contains("(skipped)"));
        assert!(reloaded.transcript.contains(&first.prompt));
    }
}
