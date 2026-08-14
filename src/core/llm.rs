//! Optional LLM features: tag suggestion and the replica's answers.
//!
//! Nothing here runs unless explicitly invoked (`legacy tag`, `legacy ask`)
//! and configured with an API key. The archive is complete and valuable
//! without this module ever being called; delete it and everything else
//! still works, which is the point — the replica is a *reader* of the
//! archive, not a component of it.
//!
//! Any OpenAI-compatible `base_url` is accepted, so a locally hosted model
//! can be substituted for a hosted one.

use std::fmt;
use std::path::Path;

use crate::cap;
use crate::core::index;
use crate::core::story::{self, Story, Visibility};

pub const DEFAULT_MODEL: &str = "gpt-4o-mini";
pub const DEFAULT_BASE_URL: &str = "https://api.openai.com/v1";

/// What the replica may draw on. Executor-only material is excluded here,
/// in code, rather than by prompt instruction — a model cannot be talked
/// out of a document it was never given.
pub const REPLICA_VISIBILITIES: [Visibility; 2] = [Visibility::Public, Visibility::Family];

/// What the replica says when the stories do not answer the question. A
/// plain admission is the correct answer; a plausible invention is not.
pub const REFUSAL: &str = "they never talked about that with me";

#[derive(Debug)]
pub struct LlmError(pub String);

impl fmt::Display for LlmError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.0)
    }
}

impl std::error::Error for LlmError {}

#[derive(Debug, Clone)]
pub struct LlmSettings {
    pub api_key: Option<String>,
    pub base_url: String,
    pub model: String,
}

impl LlmSettings {
    /// Read configuration from the environment.
    ///
    /// The `LEGACY_`-prefixed names win, so this program can be pointed at
    /// a different model than everything else on the machine. When they
    /// are absent it falls back to the conventional `OPENAI_*` names, so
    /// a shell that already has a key exported just works.
    pub fn from_env() -> LlmSettings {
        LlmSettings {
            api_key: crate::core::env::first(&["LEGACY_LLM_API_KEY", "OPENAI_API_KEY"]),
            base_url: crate::core::env::first(&["LEGACY_LLM_BASE_URL", "OPENAI_BASE_URL"])
                .unwrap_or_else(|| DEFAULT_BASE_URL.to_owned()),
            model: crate::core::env::first(&["LEGACY_LLM_MODEL", "OPENAI_MODEL"])
                .unwrap_or_else(|| DEFAULT_MODEL.to_owned()),
        }
    }

    fn require_key(&self) -> Result<&str, LlmError> {
        self.api_key.as_deref().ok_or_else(|| {
            LlmError(
                "No LLM configured. Set LEGACY_LLM_API_KEY or OPENAI_API_KEY (and \
                 optionally LEGACY_LLM_BASE_URL or OPENAI_BASE_URL to point at a \
                 local or self-hosted OpenAI-compatible model) to use this feature. \
                 Everything else in this program works without it."
                    .to_owned(),
            )
        })
    }
}

/// One chat completion. Deliberately the only network shape this module
/// uses, so a local server needs to implement just this endpoint.
fn complete(settings: &LlmSettings, prompt: &str) -> Result<String, LlmError> {
    let key = settings.require_key()?;
    let url = format!(
        "{}/chat/completions",
        settings.base_url.trim_end_matches('/')
    );

    let request = serde_json::json!({
        "model": settings.model,
        "temperature": 0,
        "messages": [{ "role": "user", "content": prompt }],
    });

    let body = cap::net::post_json(&url, key, &request.to_string())
        .map_err(|error| LlmError(format!("LLM request failed: {error}")))?;

    let parsed: serde_json::Value = serde_json::from_str(&body)
        .map_err(|error| LlmError(format!("could not parse LLM response: {error}")))?;

    if let Some(message) = parsed
        .get("error")
        .and_then(|error| error.get("message"))
        .and_then(|message| message.as_str())
    {
        return Err(LlmError(format!("LLM returned an error: {message}")));
    }

    parsed
        .get("choices")
        .and_then(|choices| choices.get(0))
        .and_then(|choice| choice.get("message"))
        .and_then(|message| message.get("content"))
        .and_then(|content| content.as_str())
        .map(|content| content.trim().to_owned())
        .ok_or_else(|| LlmError("LLM response contained no message content".to_owned()))
}

// ----------------------------------------------------------------- tags --

#[derive(Debug, Clone)]
pub struct TagSuggestion {
    pub story_id: String,
    pub current_tags: Vec<String>,
    pub suggested_tags: Vec<String>,
}

impl TagSuggestion {
    /// Suggested tags the story does not already carry.
    pub fn new_tags(&self) -> Vec<String> {
        self.suggested_tags
            .iter()
            .filter(|tag| !self.current_tags.contains(tag))
            .cloned()
            .collect()
    }
}

const TAG_PROMPT: &str = "Suggest 3 to 6 short, lowercase, hyphenated topical tags for the \
following life story. Respond with ONLY a comma-separated list of tags, nothing else.\n\n\
Title: {title}\n\n{body}";

pub fn suggest_tags(settings: &LlmSettings, story: &Story) -> Result<TagSuggestion, LlmError> {
    let prompt = TAG_PROMPT
        .replace("{title}", &story.title)
        .replace("{body}", &story.body);
    let text = complete(settings, &prompt)?;
    Ok(TagSuggestion {
        story_id: story.id.clone(),
        current_tags: story.tags.clone(),
        suggested_tags: parse_tag_list(&text),
    })
}

/// Normalize a model's comma-separated reply into clean, unique tags.
pub fn parse_tag_list(text: &str) -> Vec<String> {
    let mut tags: Vec<String> = text
        .split(',')
        .map(|tag| tag.trim().to_lowercase())
        .filter(|tag| !tag.is_empty())
        .collect();
    tags.sort();
    tags.dedup();
    tags
}

/// Write suggested tags into a story's frontmatter, recording which model
/// produced them. The body — the person's own words — is never touched.
pub fn apply_tag_suggestion(
    archive_root: &Path,
    suggestion: &TagSuggestion,
    model_id: &str,
) -> Result<Story, LlmError> {
    let stories = story::iter_stories(archive_root).map_err(|error| LlmError(error.0))?;
    let mut target = stories
        .into_iter()
        .find(|candidate| candidate.id == suggestion.story_id)
        .ok_or_else(|| LlmError(format!("story {:?} not found", suggestion.story_id)))?;

    let body_before = target.body.clone();

    let mut tags = target.tags.clone();
    for tag in &suggestion.suggested_tags {
        if !tags.contains(tag) {
            tags.push(tag.clone());
        }
    }
    tags.sort();
    target.tags = tags;
    target.tags_generated_by = Some(model_id.to_owned());

    debug_assert_eq!(body_before, target.body, "tagging must not rewrite prose");
    story::save_story(archive_root, &target, true).map_err(|error| LlmError(error.0))?;
    Ok(target)
}

/// Suggest tags for every story a model has not already tagged.
pub fn suggest_tags_for_archive(
    settings: &LlmSettings,
    archive_root: &Path,
) -> Result<Vec<TagSuggestion>, LlmError> {
    let stories = story::iter_stories(archive_root).map_err(|error| LlmError(error.0))?;
    let mut suggestions = Vec::new();
    for story in stories {
        if story.tags_generated_by.is_some() {
            continue;
        }
        suggestions.push(suggest_tags(settings, &story)?);
    }
    Ok(suggestions)
}

const FOLLOWUP_PROMPT: &str = "A person was just asked the following interview question and gave \
this answer. Suggest ONE short, natural follow-up question to ask next. Respond with only the \
question, nothing else.\n\nQuestion: {question}\n\nAnswer: {answer}";

pub fn suggest_followup(
    settings: &LlmSettings,
    question: &str,
    answer: &str,
) -> Result<String, LlmError> {
    let prompt = FOLLOWUP_PROMPT
        .replace("{question}", question)
        .replace("{answer}", answer);
    complete(settings, &prompt)
}

// -------------------------------------------------------------- replica --

#[derive(Debug, Clone)]
pub struct AskResult {
    pub answer: String,
    pub cited_story_ids: Vec<String>,
}

/// Refuse to run past an archive's configured sunset date.
pub fn check_sunset(replica_sunset: Option<&str>) -> Result<(), LlmError> {
    let Some(sunset) = replica_sunset else {
        return Ok(());
    };
    if sunset.trim().is_empty() {
        return Ok(());
    }
    let Some(sunset_ordinal) = crate::core::clock::parse_date_ordinal(sunset) else {
        return Err(LlmError(format!(
            "archive.yaml has an unreadable replica sunset date {sunset:?}; \
             expected YYYY-MM-DD"
        )));
    };
    if crate::core::clock::today_ordinal() >= sunset_ordinal {
        return Err(LlmError(format!(
            "This replica's sunset date ({sunset}) has passed. `ask` refuses to run \
             past it, as configured in archive.yaml."
        )));
    }
    Ok(())
}

/// Words too common to narrow a search, dropped when building the
/// retrieval query.
const STOPWORDS: [&str; 34] = [
    "a", "an", "the", "is", "are", "was", "were", "be", "been", "being", "do", "does", "did",
    "have", "has", "had", "of", "to", "in", "on", "at", "for", "with", "about", "what", "when",
    "where", "who", "why", "how", "you", "your", "that", "this",
];

/// Turn a natural-language question into an FTS query.
///
/// FTS5 defaults to AND semantics, under which a full sentence matches
/// almost nothing — few stories contain every word of "what did you do
/// after school?". Dropping stopwords and joining the rest with OR gets
/// candidates back; ranking then does the actual work.
pub fn retrieval_query(question: &str) -> String {
    let words: Vec<String> = question
        .split(|character: char| !character.is_alphanumeric())
        .filter(|word| !word.is_empty())
        .map(str::to_lowercase)
        .collect();

    let keywords: Vec<String> = words
        .iter()
        .filter(|word| !STOPWORDS.contains(&word.as_str()))
        .cloned()
        .collect();

    let chosen = if keywords.is_empty() { words } else { keywords };
    if chosen.is_empty() {
        question.to_owned()
    } else {
        chosen.join(" OR ")
    }
}

const ASK_PROMPT: &str = "You are answering questions in the voice of the person these stories \
are about, using ONLY the stories provided below -- never invent details. Quote their own \
phrasing where you can. If the stories do not answer the question, respond with exactly: \
{refusal}\n\nEnd your answer with a final line: \"Sources: <comma-separated story ids you \
used>\". If you could not answer, write \"Sources: none\".\n\nStories:\n{context}\n\n\
Question: {question}";

/// Answer a question from the archive, citing the stories used.
pub fn ask(
    settings: &LlmSettings,
    archive_root: &Path,
    index_db_path: &Path,
    question: &str,
    replica_sunset: Option<&str>,
) -> Result<AskResult, LlmError> {
    check_sunset(replica_sunset)?;

    let hits = index::search(
        index_db_path,
        &retrieval_query(question),
        Some(&REPLICA_VISIBILITIES),
        6,
    )
    .map_err(|error| LlmError(error.0))?;

    if hits.is_empty() {
        return Ok(AskResult {
            answer: REFUSAL.to_owned(),
            cited_story_ids: Vec::new(),
        });
    }

    let all_stories = story::iter_stories(archive_root).map_err(|error| LlmError(error.0))?;
    let retrieved: Vec<Story> = hits
        .iter()
        .filter_map(|hit| all_stories.iter().find(|story| story.id == hit.id).cloned())
        .collect();

    let context = retrieved
        .iter()
        .map(|story| format!("[{}] {}\n{}", story.id, story.title, story.body.trim()))
        .collect::<Vec<String>>()
        .join("\n\n");

    let prompt = ASK_PROMPT
        .replace("{refusal}", REFUSAL)
        .replace("{context}", &context)
        .replace("{question}", question);
    let reply = complete(settings, &prompt)?;

    Ok(split_sources(&reply, &retrieved))
}

/// Separate the model's prose from its trailing `Sources:` line.
fn split_sources(reply: &str, retrieved: &[Story]) -> AskResult {
    let mut answer_lines: Vec<&str> = Vec::new();
    let mut cited: Vec<String> = Vec::new();
    let mut found_sources = false;

    for line in reply.lines() {
        let trimmed = line.trim();
        if !found_sources && trimmed.to_lowercase().starts_with("sources:") {
            found_sources = true;
            if let Some((_, list)) = trimmed.split_once(':') {
                cited = list
                    .split(',')
                    .map(|value| value.trim().to_owned())
                    .filter(|value| !value.is_empty() && value.to_lowercase() != "none")
                    .collect();
            }
            continue;
        }
        if !found_sources {
            answer_lines.push(line);
        }
    }

    let answer = answer_lines.join("\n").trim().to_owned();

    // Only claim sources the retrieval step actually supplied, so a model
    // cannot cite a story it was never shown.
    let verified: Vec<String> = cited
        .into_iter()
        .filter(|id| retrieved.iter().any(|story| &story.id == id))
        .collect();

    let cited_story_ids = if verified.is_empty() && answer != REFUSAL {
        retrieved.iter().map(|story| story.id.clone()).collect()
    } else {
        verified
    };

    AskResult {
        answer,
        cited_story_ids,
    }
}

#[cfg(test)]
mod tests {
    use super::{
        check_sunset, parse_tag_list, retrieval_query, split_sources, LlmSettings, REFUSAL,
    };
    use crate::core::story::{new_story, NewStory};

    fn story(id: &str) -> crate::core::story::Story {
        let mut built = new_story(NewStory {
            title: "A story".to_owned(),
            ..NewStory::default()
        })
        .expect("story");
        built.id = id.to_owned();
        built
    }

    #[test]
    fn settings_come_from_the_environment_with_defaults() {
        let settings = LlmSettings::from_env();
        assert!(!settings.model.is_empty());
        assert!(settings.base_url.starts_with("http"));
    }

    #[test]
    fn a_missing_api_key_is_an_actionable_error() {
        let settings = LlmSettings {
            api_key: None,
            base_url: "http://localhost".to_owned(),
            model: "test".to_owned(),
        };
        let error = super::complete(&settings, "hello").expect_err("should refuse");
        assert!(error.0.contains("LEGACY_LLM_API_KEY"));
        assert!(error.0.contains("works without it"));
    }

    #[test]
    fn tag_lists_are_normalized_and_deduplicated() {
        assert_eq!(
            parse_tag_list("Education, Leaving-Home, education, "),
            vec!["education", "leaving-home"]
        );
        assert!(parse_tag_list("   ").is_empty());
    }

    #[test]
    fn new_tags_excludes_ones_already_present() {
        let suggestion = super::TagSuggestion {
            story_id: "x".to_owned(),
            current_tags: vec!["education".to_owned()],
            suggested_tags: vec!["education".to_owned(), "leaving-home".to_owned()],
        };
        assert_eq!(suggestion.new_tags(), vec!["leaving-home"]);
    }

    #[test]
    fn retrieval_query_drops_stopwords_and_joins_with_or() {
        let query = retrieval_query("What did you do after school?");
        assert!(query.contains(" OR "));
        assert!(query.contains("school"));
        assert!(!query.split(" OR ").any(|word| word == "what"));
    }

    #[test]
    fn retrieval_query_keeps_something_when_everything_is_a_stopword() {
        let query = retrieval_query("what did you do");
        assert!(!query.is_empty());
    }

    #[test]
    fn sunset_gating() {
        assert!(check_sunset(None).is_ok());
        assert!(check_sunset(Some("")).is_ok());
        assert!(check_sunset(Some("2100-01-01")).is_ok());
        assert!(check_sunset(Some("2020-01-01")).is_err());
        assert!(check_sunset(Some("not-a-date")).is_err());
    }

    #[test]
    fn parses_a_trailing_sources_line() {
        let retrieved = vec![story("1994-university")];
        let result = split_sources("I packed a suitcase.\nSources: 1994-university", &retrieved);
        assert_eq!(result.answer, "I packed a suitcase.");
        assert_eq!(result.cited_story_ids, vec!["1994-university"]);
    }

    #[test]
    fn discards_citations_for_stories_the_model_never_saw() {
        let retrieved = vec![story("real-story")];
        let result = split_sources("An answer.\nSources: invented-story", &retrieved);
        // Falls back to what was actually retrieved rather than repeating
        // a fabricated id.
        assert_eq!(result.cited_story_ids, vec!["real-story"]);
    }

    #[test]
    fn a_refusal_cites_nothing() {
        let retrieved = vec![story("real-story")];
        let result = split_sources(&format!("{REFUSAL}\nSources: none"), &retrieved);
        assert_eq!(result.answer, REFUSAL);
        assert!(result.cited_story_ids.is_empty());
    }
}
