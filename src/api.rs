//! REST API: mirrors `cli` one route per command, JSON in and out.
//!
//! Binds to 127.0.0.1 by default. There is no authentication in v1 — this
//! is meant for a single local user driving the same machine a GUI would,
//! not a multi-tenant service. If you expose it beyond localhost, put a
//! reverse proxy with auth in front of it yourself.
//!
//! There is no server-side state beyond an interview session id, which is
//! just the name of an already-persisted file, so restarting the process
//! loses nothing.

use std::path::{Path, PathBuf};

use serde_json::{json, Value};

use crate::cap;
use crate::core::interview::{self, SessionMode};
use crate::core::llm;
use crate::core::manifest;
use crate::core::media::{self, IngestOptions};
use crate::core::seal;
use crate::core::story::{self, NewStory, Visibility};
use crate::core::timeline;
use crate::core::vault::{self, Vault};

/// An HTTP status and a JSON body.
struct Reply {
    status: u16,
    body: Value,
}

impl Reply {
    fn ok(body: Value) -> Reply {
        Reply { status: 200, body }
    }

    fn error(status: u16, detail: &str) -> Reply {
        Reply {
            status,
            body: json!({ "detail": detail }),
        }
    }
}

/// Start the server. Blocks until the process is stopped.
pub fn serve(host: &str, port: u16) -> Result<(), String> {
    let listener = cap::net::bind(host, port)?;

    println!("legacy API listening on http://{}", listener.address);
    if !listener.is_loopback {
        println!(
            "WARNING: bound to a non-loopback address with no authentication. \
             Put a reverse proxy with auth in front of this."
        );
    }

    for mut request in listener.server.incoming_requests() {
        let method = request.method().as_str().to_owned();
        let url = request.url().to_owned();
        let (path, _query) = split_url(&url);

        if method == "GET" && path == "/" {
            let mut response = tiny_http::Response::from_string(include_str!("web.html"));
            if let Ok(header) = tiny_http::Header::from_bytes(
                &b"Content-Type"[..],
                &b"text/html; charset=utf-8"[..],
            ) {
                response.add_header(header);
            }
            let _ignored = request.respond(response);
            continue;
        }

        let mut body = String::new();
        let _ignored = std::io::Read::read_to_string(request.as_reader(), &mut body);
        let reply = route(&method, &url, &body);

        let mut response =
            tiny_http::Response::from_string(reply.body.to_string()).with_status_code(reply.status);
        // The header name is a literal, so this parse cannot fail; if it
        // somehow did, sending the body without it beats panicking.
        if let Ok(header) =
            tiny_http::Header::from_bytes(&b"Content-Type"[..], &b"application/json"[..])
        {
            response.add_header(header);
        }
        let _ignored = request.respond(response);
    }
    Ok(())
}

/// Split a URL into its path and query string.
fn split_url(url: &str) -> (&str, &str) {
    url.split_once('?').unwrap_or((url, ""))
}

/// Look up a query parameter, decoding `%XX` and `+`.
fn query_param(query: &str, name: &str) -> Option<String> {
    for pair in query.split('&') {
        let Some((key, value)) = pair.split_once('=') else {
            continue;
        };
        if key == name {
            return Some(percent_decode(value));
        }
    }
    None
}

fn percent_decode(value: &str) -> String {
    let bytes = value.replace('+', " ").into_bytes();
    let mut out: Vec<u8> = Vec::with_capacity(bytes.len());
    let mut index = 0;
    while index < bytes.len() {
        match bytes.get(index) {
            Some(b'%') => {
                let high = bytes.get(index + 1).and_then(|b| hex_value(*b));
                let low = bytes.get(index + 2).and_then(|b| hex_value(*b));
                match (high, low) {
                    (Some(high), Some(low)) => {
                        out.push(high * 16 + low);
                        index += 3;
                    }
                    _incomplete => {
                        out.push(b'%');
                        index += 1;
                    }
                }
            }
            Some(byte) => {
                out.push(*byte);
                index += 1;
            }
            None => break,
        }
    }
    String::from_utf8_lossy(&out).into_owned()
}

fn hex_value(byte: u8) -> Option<u8> {
    match byte {
        b'0'..=b'9' => Some(byte - b'0'),
        b'a'..=b'f' => Some(byte - b'a' + 10),
        b'A'..=b'F' => Some(byte - b'A' + 10),
        _not_hex => None,
    }
}

fn route(method: &str, url: &str, body: &str) -> Reply {
    let (path, query) = split_url(url);
    let parsed_body: Value = if body.trim().is_empty() {
        json!({})
    } else {
        match serde_json::from_str(body) {
            Ok(value) => value,
            Err(error) => return Reply::error(400, &format!("invalid JSON body: {error}")),
        }
    };

    match (method, path) {
        ("POST", "/init") => handle_init(&parsed_body),
        ("POST", "/stories") => handle_create_story(&parsed_body),
        ("GET", "/stories") => handle_list_stories(query),
        ("POST", "/timeline/build") => handle_timeline_build(&parsed_body),
        ("GET", "/verify") => handle_verify(query),
        ("POST", "/media/ingest") => handle_media_ingest(&parsed_body),
        ("POST", "/interviews") => handle_interview_start(&parsed_body),
        ("POST", "/seal") => handle_seal(&parsed_body),
        ("POST", "/unseal") => handle_unseal(&parsed_body),
        ("POST", "/tag") => handle_tag(&parsed_body),
        ("POST", "/enrich") => handle_enrich(&parsed_body),
        ("POST", "/ask") => handle_ask(&parsed_body),
        ("GET", "/banks") => handle_banks(),
        ("GET", "/archive") => handle_archive(query),
        (_method, other) => {
            if let Some(rest) = other.strip_prefix("/interviews/") {
                return route_interview(method, rest, &parsed_body, query);
            }
            Reply::error(404, &format!("no route for {method} {other}"))
        }
    }
}

fn route_interview(method: &str, rest: &str, body: &Value, query: &str) -> Reply {
    let (session_id, action) = match rest.split_once('/') {
        Some((id, action)) => (id, action),
        None => (rest, ""),
    };
    match (method, action) {
        ("GET", "") => handle_interview_get(session_id, query),
        ("POST", "answer") => handle_interview_answer(session_id, body),
        ("POST", "skip") => handle_interview_skip(session_id, body),
        (_method, _action) => Reply::error(404, "no such interview route"),
    }
}

fn body_str(body: &Value, key: &str) -> Option<String> {
    body.get(key).and_then(Value::as_str).map(str::to_owned)
}

fn body_list(body: &Value, key: &str) -> Vec<String> {
    body.get(key)
        .and_then(Value::as_array)
        .map(|items| {
            items
                .iter()
                .filter_map(Value::as_str)
                .map(str::to_owned)
                .collect()
        })
        .unwrap_or_default()
}

fn body_bool(body: &Value, key: &str) -> bool {
    body.get(key).and_then(Value::as_bool).unwrap_or(false)
}

/// Resolve and open the archive a request works on.
///
/// A request that names no archive gets the default one, created if it
/// does not exist yet, so the web UI needs no path typed into it before
/// anything works. A request that does name one is opened as given and
/// never created implicitly — a wrong path should say so rather than
/// silently become a new empty archive.
fn vault_from(archive: Option<String>) -> Result<Vault, Reply> {
    match archive {
        Some(archive) => {
            Vault::open(Path::new(&archive)).map_err(|error| Reply::error(404, &error.0))
        }
        None => {
            vault::open_or_init(&vault::default_root()).map_err(|error| Reply::error(500, &error.0))
        }
    }
}

fn visibility_from(body: &Value) -> Result<Option<Visibility>, Reply> {
    match body_str(body, "visibility") {
        Some(raw) => Visibility::parse(&raw)
            .map(Some)
            .map_err(|error| Reply::error(400, &error.0)),
        None => Ok(None),
    }
}

fn story_json(story: &story::Story) -> Value {
    json!({
        "id": story.id,
        "title": story.title,
        "date": story.date,
        "date_precision": story.date_precision.as_str(),
        "people": story.people,
        "places": story.places,
        "tags": story.tags,
        "media": story.media,
        "source": story.source,
        "recorded_at": story.recorded_at,
        "tags_generated_by": story.tags_generated_by,
        "visibility": story.visibility.as_str(),
        "body": story.body,
        "path": story.relative_path().display().to_string(),
    })
}

fn handle_init(body: &Value) -> Reply {
    let (Some(path), Some(subject)) = (body_str(body, "path"), body_str(body, "subject")) else {
        return Reply::error(400, "init needs \"path\" and \"subject\"");
    };
    let threshold = body.get("threshold").and_then(Value::as_u64).unwrap_or(3);
    let shares = body.get("shares").and_then(Value::as_u64).unwrap_or(5);

    match Vault::init(Path::new(&path), &subject, threshold, shares) {
        Ok(vault) => match vault.load_config() {
            Ok(config) => Reply::ok(json!({
                "root": vault.root.display().to_string(),
                "subject": config.subject_name,
                "schema_version": config.schema_version,
            })),
            Err(error) => Reply::error(500, &error.0),
        },
        Err(error) => Reply::error(400, &error.0),
    }
}

fn handle_create_story(body: &Value) -> Reply {
    let vault = match vault_from(body_str(body, "archive")) {
        Ok(vault) => vault,
        Err(reply) => return reply,
    };
    let visibility = match visibility_from(body) {
        Ok(visibility) => visibility,
        Err(reply) => return reply,
    };

    let mut request = NewStory {
        // Left optional so a model can supply it; `new_story` still
        // refuses a story that ends up with no title at all.
        title: body_str(body, "title").unwrap_or_default(),
        date: body_str(body, "date"),
        body: body_str(body, "body").unwrap_or_default(),
        people: body_list(body, "people"),
        places: body_list(body, "places"),
        tags: body_list(body, "tags"),
        visibility,
        ..NewStory::default()
    };

    // Enrichment is a convenience, never a gate: if the model is missing,
    // misconfigured or simply down, the story is still saved exactly as
    // it was typed. Losing what someone wrote because a metadata helper
    // failed would be indefensible.
    let mut enriched: Vec<String> = Vec::new();
    let mut enrich_error: Option<String> = None;
    if body.get("enrich").and_then(Value::as_bool).unwrap_or(true) {
        let settings = llm::LlmSettings::from_env();
        if settings.api_key.is_some() {
            match llm::enrich_new_story(&settings, &mut request) {
                Ok(filled) => enriched = filled,
                Err(error) => enrich_error = Some(error.0),
            }
        }
    }

    let built = match story::new_story(request) {
        Ok(built) => built,
        Err(error) => return Reply::error(400, &error.0),
    };
    match story::save_story(&vault.root, &built, false) {
        Ok(_path) => {
            let mut payload = story_json(&built);
            if let Some(object) = payload.as_object_mut() {
                object.insert("enriched_fields".to_owned(), json!(enriched));
                if let Some(error) = enrich_error {
                    object.insert("enrich_error".to_owned(), json!(error));
                }
            }
            Reply::ok(payload)
        }
        Err(error) => Reply::error(400, &error.0),
    }
}

fn handle_list_stories(query: &str) -> Reply {
    let vault = match vault_from(query_param(query, "archive")) {
        Ok(vault) => vault,
        Err(reply) => return reply,
    };
    let stories = match story::iter_stories(&vault.root) {
        Ok(stories) => stories,
        Err(error) => return Reply::error(500, &error.0),
    };

    let year = query_param(query, "year").and_then(|value| value.parse::<u32>().ok());
    let person = query_param(query, "person");
    let tag = query_param(query, "tag");

    let filtered: Vec<Value> = stories
        .iter()
        .filter(|story| year.is_none() || story.fuzzy_date().year() == year)
        .filter(|story| {
            person
                .as_ref()
                .is_none_or(|person| story.people.contains(person))
        })
        .filter(|story| tag.as_ref().is_none_or(|tag| story.tags.contains(tag)))
        .map(story_json)
        .collect();

    Reply::ok(Value::Array(filtered))
}

fn handle_timeline_build(body: &Value) -> Reply {
    let vault = match vault_from(body_str(body, "archive")) {
        Ok(vault) => vault,
        Err(reply) => return reply,
    };
    match timeline::rebuild_derived_state(&vault) {
        Ok(result) => Reply::ok(json!({
            "story_count": result.story_count,
            "timeline_path": result.timeline_path.display().to_string(),
            "index_path": result.index_path.display().to_string(),
            "manifest_path": result.manifest_path.display().to_string(),
            "readme_path": result.readme_path.display().to_string(),
        })),
        Err(error) => Reply::error(500, &error.0),
    }
}

fn handle_verify(query: &str) -> Reply {
    let vault = match vault_from(query_param(query, "archive")) {
        Ok(vault) => vault,
        Err(reply) => return reply,
    };
    match manifest::verify_manifest(&vault.root) {
        Ok(findings) => {
            let problems: Vec<Value> = findings
                .iter()
                .map(|finding| {
                    json!({
                        "path": finding.path.display().to_string(),
                        "reason": finding.problem.as_str(),
                    })
                })
                .collect();
            Reply::ok(json!({ "ok": findings.is_empty(), "problems": problems }))
        }
        Err(error) => Reply::error(500, &error.0),
    }
}

fn handle_media_ingest(body: &Value) -> Reply {
    let vault = match vault_from(body_str(body, "archive")) {
        Ok(vault) => vault,
        Err(reply) => return reply,
    };
    let Some(source) = body_str(body, "source") else {
        return Reply::error(400, "media ingest needs a \"source\" directory");
    };
    let visibility = match visibility_from(body) {
        Ok(visibility) => visibility,
        Err(reply) => return reply,
    };

    let options = IngestOptions {
        date_from_exif: body_bool(body, "date_from_exif"),
        manual_date: body_str(body, "date"),
        visibility,
    };

    match media::ingest_directory(Path::new(&source), &vault.media_dir(), &options) {
        Ok(items) => {
            let payload: Vec<Value> = items
                .iter()
                .map(|item| {
                    json!({
                        "source": item.source.display().to_string(),
                        "dest": item.destination.display().to_string(),
                        "sidecar_path": item.sidecar_path.display().to_string(),
                        "sha256": item.sidecar.sha256,
                        "date": item.sidecar.date,
                        "visibility": item.sidecar.visibility.as_str(),
                    })
                })
                .collect();
            Reply::ok(Value::Array(payload))
        }
        Err(error) => Reply::error(400, &error.0),
    }
}

fn session_json(session: &interview::Session, bank: &interview::QuestionBank) -> Value {
    let next = interview::next_question(bank, session).map(|question| {
        json!({
            "id": question.id,
            "prompt": question.prompt,
            "followups": question.followups,
            "tags": question.tags,
        })
    });
    let stories: Vec<Value> = session
        .stories
        .iter()
        .map(|(question, story)| json!({ "question_id": question, "story_id": story }))
        .collect();

    json!({
        "session_id": session.session_id,
        "bank": session.bank,
        "mode": session.mode.as_str(),
        "status": session.status.as_str(),
        "answered": session.answered,
        "skipped": session.skipped,
        "stories": stories,
        "next_question": next,
    })
}

fn handle_banks() -> Reply {
    let names = interview::list_banks(None);
    let banks: Vec<Value> = names
        .iter()
        .filter_map(|name| interview::load_bank(name, None).ok())
        .map(|bank| {
            json!({
                "name": bank.name,
                "description": bank.description,
                "question_count": bank.questions.len(),
            })
        })
        .collect();
    Reply::ok(Value::Array(banks))
}

/// Which archive this server is writing to, so the UI can say so without
/// making the operator supply the path it already resolved itself.
fn handle_archive(query: &str) -> Reply {
    let vault = match vault_from(query_param(query, "archive")) {
        Ok(vault) => vault,
        Err(reply) => return reply,
    };
    let config = match vault.load_config() {
        Ok(config) => config,
        Err(error) => return Reply::error(500, &error.0),
    };
    let story_count = story::iter_stories(&vault.root)
        .map(|stories| stories.len())
        .unwrap_or(0);
    Reply::ok(json!({
        "root": vault.root.display().to_string(),
        "subject": config.subject_name,
        "story_count": story_count,
    }))
}

fn handle_interview_start(body: &Value) -> Reply {
    let vault = match vault_from(body_str(body, "archive")) {
        Ok(vault) => vault,
        Err(reply) => return reply,
    };
    let Some(bank_name) = body_str(body, "bank") else {
        return Reply::error(400, "starting an interview needs a \"bank\"");
    };
    if body_str(body, "mode").as_deref() == Some("voice") {
        return Reply::error(
            400,
            "voice mode needs a local microphone and is only available on the CLI",
        );
    }

    let bank = match interview::load_bank(&bank_name, None) {
        Ok(bank) => bank,
        Err(error) => return Reply::error(400, &error.0),
    };
    let mut session = interview::new_session(&vault.root, &bank_name, SessionMode::Text);
    match interview::save_session(&vault.root, &mut session) {
        Ok(_path) => Reply::ok(session_json(&session, &bank)),
        Err(error) => Reply::error(500, &error.0),
    }
}

/// Load a session and its bank together, since every route needs both.
fn load_session_and_bank(
    vault: &Vault,
    session_id: &str,
) -> Result<(interview::Session, interview::QuestionBank), Reply> {
    let session = interview::load_session(&vault.root, session_id)
        .map_err(|error| Reply::error(404, &error.0))?;
    let bank =
        interview::load_bank(&session.bank, None).map_err(|error| Reply::error(500, &error.0))?;
    Ok((session, bank))
}

fn handle_interview_get(session_id: &str, query: &str) -> Reply {
    let vault = match vault_from(query_param(query, "archive")) {
        Ok(vault) => vault,
        Err(reply) => return reply,
    };
    match load_session_and_bank(&vault, session_id) {
        Ok((session, bank)) => Reply::ok(session_json(&session, &bank)),
        Err(reply) => reply,
    }
}

fn handle_interview_answer(session_id: &str, body: &Value) -> Reply {
    let vault = match vault_from(body_str(body, "archive")) {
        Ok(vault) => vault,
        Err(reply) => return reply,
    };
    let Some(answer) = body_str(body, "answer") else {
        return Reply::error(400, "an answer needs \"answer\" text");
    };
    let visibility = match visibility_from(body) {
        Ok(visibility) => visibility.unwrap_or(Visibility::Family),
        Err(reply) => return reply,
    };

    let (mut session, bank) = match load_session_and_bank(&vault, session_id) {
        Ok(pair) => pair,
        Err(reply) => return reply,
    };
    let Some(question) = interview::next_question(&bank, &session).cloned() else {
        return Reply::error(400, "session already complete");
    };

    if let Err(error) =
        interview::record_answer(&vault.root, &mut session, &question, &answer, visibility)
    {
        return Reply::error(400, &error.0);
    }
    if interview::next_question(&bank, &session).is_none() {
        if let Err(error) = interview::mark_complete(&vault.root, &mut session) {
            return Reply::error(500, &error.0);
        }
    }
    Reply::ok(session_json(&session, &bank))
}

fn handle_interview_skip(session_id: &str, body: &Value) -> Reply {
    let vault = match vault_from(body_str(body, "archive")) {
        Ok(vault) => vault,
        Err(reply) => return reply,
    };
    let (mut session, bank) = match load_session_and_bank(&vault, session_id) {
        Ok(pair) => pair,
        Err(reply) => return reply,
    };
    let Some(question) = interview::next_question(&bank, &session).cloned() else {
        return Reply::error(400, "session already complete");
    };
    match interview::skip_question(&vault.root, &mut session, &question) {
        Ok(()) => Reply::ok(session_json(&session, &bank)),
        Err(error) => Reply::error(500, &error.0),
    }
}

fn handle_seal(body: &Value) -> Reply {
    let vault = match vault_from(body_str(body, "archive")) {
        Ok(vault) => vault,
        Err(reply) => return reply,
    };
    let mut config = match vault.load_config() {
        Ok(config) => config,
        Err(error) => return Reply::error(500, &error.0),
    };

    let requested = body_list(body, "tiers");
    let tiers: Vec<String> = if requested.is_empty() {
        config.tiers.iter().map(|(name, _)| name.clone()).collect()
    } else {
        requested
    };
    let escrow = body_bool(body, "plaintext_escrow");

    let mut results = Vec::new();
    for tier in &tiers {
        let use_escrow = escrow && tier == "family";
        match seal::seal_tier(&vault, &mut config, tier, use_escrow) {
            Ok(result) => results.push(json!({
                "tier": result.tier,
                "plaintext_escrow": result.plaintext_escrow,
                "story_count": result.story_count,
                "output_path": result.output_path.display().to_string(),
                "par2_created": result.par2_path.is_some(),
                "identity_sha256": result.identity_sha256,
                "shares": result.shares,
            })),
            Err(error) => return Reply::error(400, &error.0),
        }
    }

    if let Err(error) = vault.save_config(&config) {
        return Reply::error(500, &error.0);
    }
    if let Err(error) = timeline::rebuild_derived_state(&vault) {
        return Reply::error(500, &error.0);
    }
    Reply::ok(Value::Array(results))
}

fn handle_unseal(body: &Value) -> Reply {
    let vault = match vault_from(body_str(body, "archive")) {
        Ok(vault) => vault,
        Err(reply) => return reply,
    };
    let config = match vault.load_config() {
        Ok(config) => config,
        Err(error) => return Reply::error(500, &error.0),
    };
    let shares = body_list(body, "shares");
    if shares.is_empty() {
        return Reply::error(400, "unsealing needs a \"shares\" array");
    }
    let output_base = body_str(body, "output")
        .map(PathBuf::from)
        .unwrap_or_else(|| vault.root.join("unsealed"));

    match seal::unseal(&vault, &config, &shares, &output_base) {
        Ok(result) => Reply::ok(json!({
            "tier": result.tier,
            "output_dir": result.output_dir.display().to_string(),
            "file_count": result.file_count,
        })),
        Err(error) => Reply::error(400, &error.0),
    }
}

/// Backfill empty tags and people on stories already in the archive.
///
/// Defaults to a preview, like `/tag`: nothing is written unless the
/// caller asks for it, so the operator sees what a model wants to add to
/// their archive before it lands in the files.
fn handle_enrich(body: &Value) -> Reply {
    let vault = match vault_from(body_str(body, "archive")) {
        Ok(vault) => vault,
        Err(reply) => return reply,
    };
    let settings = llm::LlmSettings::from_env();
    let apply = body_bool(body, "apply");

    let reports = match llm::enrich_archive(&settings, &vault.root, apply) {
        Ok(reports) => reports,
        Err(error) => return Reply::error(400, &error.0),
    };

    let payload: Vec<Value> = reports
        .iter()
        .map(|report| {
            json!({
                "story_id": report.story_id,
                "filled": report.filled,
                "title": report.title,
                "tags": report.tags,
                "people": report.people,
            })
        })
        .collect();

    Reply::ok(json!({
        "applied": apply,
        "changed": payload.len(),
        "stories": payload,
    }))
}

fn handle_tag(body: &Value) -> Reply {
    let vault = match vault_from(body_str(body, "archive")) {
        Ok(vault) => vault,
        Err(reply) => return reply,
    };
    let settings = llm::LlmSettings::from_env();
    let apply = body_bool(body, "apply");

    let suggestions = match llm::suggest_tags_for_archive(&settings, &vault.root) {
        Ok(suggestions) => suggestions,
        Err(error) => return Reply::error(400, &error.0),
    };

    let mut payload = Vec::new();
    for suggestion in &suggestions {
        let new_tags = suggestion.new_tags();
        let mut applied = false;
        if apply && !new_tags.is_empty() {
            if let Err(error) = llm::apply_tag_suggestion(&vault.root, suggestion, &settings.model)
            {
                return Reply::error(500, &error.0);
            }
            applied = true;
        }
        payload.push(json!({
            "story_id": suggestion.story_id,
            "current_tags": suggestion.current_tags,
            "suggested_tags": suggestion.suggested_tags,
            "new_tags": new_tags,
            "applied": applied,
        }));
    }
    Reply::ok(Value::Array(payload))
}

fn handle_ask(body: &Value) -> Reply {
    let vault = match vault_from(body_str(body, "archive")) {
        Ok(vault) => vault,
        Err(reply) => return reply,
    };
    let Some(question) = body_str(body, "question") else {
        return Reply::error(400, "asking needs a \"question\"");
    };
    let config = match vault.load_config() {
        Ok(config) => config,
        Err(error) => return Reply::error(500, &error.0),
    };
    let settings = llm::LlmSettings::from_env();

    match llm::ask(
        &settings,
        &vault.root,
        &vault.index_db_path(),
        &question,
        config.replica_sunset.as_deref(),
    ) {
        Ok(result) => Reply::ok(json!({
            "answer": result.answer,
            "cited_story_ids": result.cited_story_ids,
        })),
        Err(error) => Reply::error(400, &error.0),
    }
}

#[cfg(test)]
mod tests {
    use super::{percent_decode, query_param, route, split_url};
    use crate::core::vault::Vault;
    use serde_json::json;

    fn archive() -> (tempfile::TempDir, String) {
        let temp = tempfile::TempDir::new().expect("temp dir");
        let root = temp.path().join("arch");
        Vault::init(&root, "Jane Doe", 2, 3).expect("init");
        let text = root.to_str().expect("utf-8").to_owned();
        (temp, text)
    }

    /// The web UI shows the operator which archive it is writing to
    /// instead of asking them to type the path it already resolved.
    ///
    /// Only the explicitly-named case is exercised here: the no-archive
    /// path resolves through `LEGACY_ARCHIVE`, and cargo runs tests in
    /// parallel threads against one process-wide environment, so setting
    /// it here would race `vault`'s own tests. Both halves of that path
    /// are covered there instead.
    #[test]
    fn archive_endpoint_reports_the_open_archive() {
        let (_temp, root) = archive();
        let reply = route("GET", &format!("/archive?archive={root}"), "");

        assert_eq!(reply.status, 200);
        assert_eq!(
            reply.body.get("subject").and_then(|v| v.as_str()),
            Some("Jane Doe")
        );
        assert_eq!(
            reply.body.get("story_count").and_then(|v| v.as_u64()),
            Some(0)
        );
        assert!(reply
            .body
            .get("root")
            .and_then(|v| v.as_str())
            .is_some_and(|shown| shown.contains("arch")));
    }

    /// Stories that need nothing must cost no tokens and no request, so
    /// running the enrich button twice is cheap and safe. This also keeps
    /// the test itself off the network whatever keys are in the
    /// environment.
    #[test]
    fn enrich_skips_stories_that_need_nothing() {
        let (_temp, root) = archive();
        let created = route(
            "POST",
            "/stories",
            &json!({
                "archive": root,
                "title": "Already described",
                "tags": ["complete"],
                "people": ["someone"],
                "body": "Text.",
                "enrich": false,
            })
            .to_string(),
        );
        assert_eq!(created.status, 200);

        let enriched = route("POST", "/enrich", &json!({ "archive": root }).to_string());
        assert_eq!(enriched.status, 200);
        assert_eq!(
            enriched.body.get("changed").and_then(|v| v.as_u64()),
            Some(0)
        );
        // A preview by default: nothing may be written unless asked.
        assert_eq!(
            enriched.body.get("applied").and_then(|v| v.as_bool()),
            Some(false)
        );
    }

    #[test]
    fn decodes_query_parameters() {
        assert_eq!(split_url("/stories?archive=x"), ("/stories", "archive=x"));
        assert_eq!(percent_decode("a%20b"), "a b");
        assert_eq!(percent_decode("a+b"), "a b");
        assert_eq!(percent_decode("%2Ftmp%2Farch"), "/tmp/arch");
        assert_eq!(
            query_param("archive=%2Ftmp&year=1994", "archive").as_deref(),
            Some("/tmp")
        );
        assert_eq!(query_param("archive=x", "missing"), None);
    }

    #[test]
    fn creates_and_lists_stories() {
        let (_temp, root) = archive();

        let created = route(
            "POST",
            "/stories",
            &json!({
                "archive": root,
                "title": "Starting university",
                "date": "1994-09-15",
                "body": "I packed one suitcase.",
                "enrich": false,
            })
            .to_string(),
        );
        assert_eq!(created.status, 200);
        assert_eq!(
            created.body.get("id").and_then(|v| v.as_str()),
            Some("1994-09-15-starting-university")
        );

        let listed = route("GET", &format!("/stories?archive={root}"), "");
        assert_eq!(listed.status, 200);
        assert_eq!(listed.body.as_array().map(Vec::len), Some(1));

        let filtered = route("GET", &format!("/stories?archive={root}&year=1888"), "");
        assert_eq!(filtered.body.as_array().map(Vec::len), Some(0));
    }

    #[test]
    fn builds_the_timeline_and_verifies() {
        let (_temp, root) = archive();
        route(
            "POST",
            "/stories",
            &json!({ "archive": root, "title": "A story", "date": "2000", "enrich": false })
                .to_string(),
        );

        let built = route(
            "POST",
            "/timeline/build",
            &json!({ "archive": root }).to_string(),
        );
        assert_eq!(built.status, 200);
        assert_eq!(
            built.body.get("story_count").and_then(|v| v.as_u64()),
            Some(1)
        );

        let verified = route("GET", &format!("/verify?archive={root}"), "");
        assert_eq!(
            verified.body.get("ok").and_then(|v| v.as_bool()),
            Some(true)
        );
    }

    #[test]
    fn runs_an_interview_over_http() {
        let (_temp, root) = archive();

        let started = route(
            "POST",
            "/interviews",
            &json!({ "archive": root, "bank": "childhood" }).to_string(),
        );
        assert_eq!(started.status, 200);
        let session_id = started
            .body
            .get("session_id")
            .and_then(|v| v.as_str())
            .expect("session id")
            .to_owned();
        assert!(started.body.get("next_question").is_some());

        let answered = route(
            "POST",
            &format!("/interviews/{session_id}/answer"),
            &json!({ "archive": root, "answer": "I remember the garden." }).to_string(),
        );
        assert_eq!(answered.status, 200);
        assert_eq!(
            answered
                .body
                .get("answered")
                .and_then(|v| v.as_array())
                .map(Vec::len),
            Some(1)
        );

        let skipped = route(
            "POST",
            &format!("/interviews/{session_id}/skip"),
            &json!({ "archive": root }).to_string(),
        );
        assert_eq!(
            skipped
                .body
                .get("skipped")
                .and_then(|v| v.as_array())
                .map(Vec::len),
            Some(1)
        );

        let fetched = route(
            "GET",
            &format!("/interviews/{session_id}?archive={root}"),
            "",
        );
        assert_eq!(
            fetched.body.get("session_id").and_then(|v| v.as_str()),
            Some(session_id.as_str())
        );
    }

    #[test]
    fn voice_mode_is_refused_over_http() {
        let (_temp, root) = archive();
        let reply = route(
            "POST",
            "/interviews",
            &json!({ "archive": root, "bank": "childhood", "mode": "voice" }).to_string(),
        );
        assert_eq!(reply.status, 400);
    }

    #[test]
    fn unknown_routes_and_archives_report_cleanly() {
        assert_eq!(route("GET", "/nope", "").status, 404);
        assert_eq!(
            route("GET", "/verify?archive=/does/not/exist", "").status,
            404
        );
        assert_eq!(route("POST", "/stories", "{not json").status, 400);
        assert_eq!(route("POST", "/init", "{}").status, 400);
    }

    #[test]
    fn seal_and_unseal_over_http() {
        if !crate::cap::process::which("age") {
            return;
        }
        let (_temp, root) = archive();
        route(
            "POST",
            "/stories",
            &json!({
                "archive": root,
                "title": "Family story",
                "date": "1994",
                "visibility": "family",
                "body": "For the family.",
                "enrich": false,
            })
            .to_string(),
        );

        let sealed = route(
            "POST",
            "/seal",
            &json!({ "archive": root, "tiers": ["family"] }).to_string(),
        );
        assert_eq!(sealed.status, 200);
        let tiers = sealed.body.as_array().expect("array");
        let first = tiers.first().expect("one tier");
        let shares: Vec<String> = first
            .get("shares")
            .and_then(|v| v.as_array())
            .expect("shares")
            .iter()
            .filter_map(|v| v.as_str())
            .map(str::to_owned)
            .collect();
        assert_eq!(shares.len(), 3);

        let unsealed = route(
            "POST",
            "/unseal",
            &json!({ "archive": root, "shares": &shares[..2] }).to_string(),
        );
        assert_eq!(unsealed.status, 200);
        assert_eq!(
            unsealed.body.get("tier").and_then(|v| v.as_str()),
            Some("family")
        );
    }

    #[test]
    fn banks_are_listed() {
        let reply = route("GET", "/banks", "");
        assert_eq!(reply.status, 200);
        assert!(reply.body.as_array().map(Vec::len).unwrap_or(0) >= 10);
    }
}
