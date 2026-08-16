//! Command-line entry point. Every command here has a REST twin in `api`.

use std::fmt::Write as _;
use std::io::Write as _;
use std::path::{Path, PathBuf};

use crate::args::Args;
use crate::cap;
use crate::core::interview::{self, SessionMode};
use crate::core::manifest::{self, Problem};
use crate::core::media::{self, IngestOptions};
use crate::core::seal;
use crate::core::story::{self, NewStory, Visibility};
use crate::core::timeline;
use crate::core::vault::Vault;

pub const HELP: &str = "\
legacy — a self-hosted life-story vault and digital replica

USAGE
  legacy <command> [options]

COMMANDS
  init <path> --subject NAME [--threshold N] [--shares N]
      Create a new, empty archive.

  story add [--title T] [--date D] [--body TEXT] [--people a,b]
            [--places a,b] [--tags a,b] [--visibility V]
      Add a story. Reads the body from stdin when --body is omitted.
      Dates may be YYYY-MM-DD, YYYY-MM, YYYY, YYYYs, or omitted.

  story list [--year Y] [--person SLUG] [--tag TAG]
      List stories, optionally filtered.

  interview start <bank> [--voice] [--suggest-followups]
  interview resume <session-id> [--suggest-followups]
  interview banks
      Run a guided interview. Each answer is saved as a story
      immediately, so a session can be interrupted at any point.

  media ingest <dir> [--date-from-exif] [--date D] [--visibility V]
      Copy media into the archive with a metadata sidecar per file.

  timeline build
      Regenerate timeline.md, the search index, MANIFEST.sha256,
      and README.txt.

  verify
      Check MANIFEST.sha256 against disk, and par2-verify sealed blobs.

  seal [--tiers family,executor-only] [--plaintext-escrow]
       [--write-shares-to FILE]
      Encrypt each visibility tier and print its shares ONCE.

  unseal --share \"...\" --share \"...\" [--output DIR]
      Reconstruct a key from shares and decrypt the matching tier.

  tag [--dry-run | --apply]
      Suggest tags for stories with an LLM. Previews by default.

  ask \"question\"
      Ask the replica. Answers only from stored stories, with citations.

  tui
      Browse and add stories in a full-screen terminal interface.
      Records audio with ctrl-r while adding a story.

  serve [--host H] [--port N]
      Run the REST API. Binds to 127.0.0.1 by default.

GLOBAL OPTIONS
  -a, --archive PATH   Archive root (default: current directory)
  -h, --help           Show this help

The LLM and speech features are opt-in and need LEGACY_LLM_API_KEY or
LEGACY_OPENAI_API_KEY. Everything else works with no network access.
";

/// Exit codes: 0 success, 1 the operation reported a problem, 2 misuse.
pub const EXIT_OK: i32 = 0;
pub const EXIT_PROBLEM: i32 = 1;
pub const EXIT_MISUSE: i32 = 2;

/// Verbs that take a subcommand. Everything else takes one verb, so a
/// positional value is never mistaken for a subcommand.
const GROUP_COMMANDS: [&str; 4] = ["story", "interview", "media", "timeline"];

/// Run the CLI and return a process exit code.
pub fn run(argv: &[String]) -> i32 {
    let args = Args::parse_with(argv, &GROUP_COMMANDS);

    if args.has_flag("help") || args.command.is_empty() {
        print!("{HELP}");
        return EXIT_OK;
    }

    let result = match args.verb(0) {
        Some("init") => cmd_init(&args),
        Some("story") => match args.verb(1) {
            Some("add") => cmd_story_add(&args),
            Some("list") => cmd_story_list(&args),
            Some(other) => Err(misuse(&format!("unknown story subcommand {other:?}"))),
            None => Err(misuse("story needs a subcommand: add or list")),
        },
        Some("interview") => match args.verb(1) {
            Some("start") => cmd_interview_start(&args),
            Some("resume") => cmd_interview_resume(&args),
            Some("banks") => cmd_interview_banks(&args),
            Some(other) => Err(misuse(&format!("unknown interview subcommand {other:?}"))),
            None => Err(misuse("interview needs a subcommand: start, resume, banks")),
        },
        Some("media") => match args.verb(1) {
            Some("ingest") => cmd_media_ingest(&args),
            Some(other) => Err(misuse(&format!("unknown media subcommand {other:?}"))),
            None => Err(misuse("media needs a subcommand: ingest")),
        },
        Some("timeline") => match args.verb(1) {
            Some("build") => cmd_timeline_build(&args),
            Some(other) => Err(misuse(&format!("unknown timeline subcommand {other:?}"))),
            None => Err(misuse("timeline needs a subcommand: build")),
        },
        Some("verify") => cmd_verify(&args),
        Some("seal") => cmd_seal(&args),
        Some("unseal") => cmd_unseal(&args),
        Some("tag") => cmd_tag(&args),
        Some("ask") => cmd_ask(&args),
        Some("serve") => cmd_serve(&args),
        Some("tui") => cmd_tui(&args),
        Some(other) => Err(misuse(&format!(
            "unknown command {other:?}. Run `legacy --help`."
        ))),
        None => Err(misuse("no command given")),
    };

    match result {
        Ok(outcome) => {
            print!("{}", outcome.stdout);
            outcome.code
        }
        Err(failure) => {
            let mut stderr = std::io::stderr();
            let _ignored = writeln!(stderr, "{}", failure.message);
            failure.code
        }
    }
}

/// What a command produced: text for stdout and the exit code to use.
pub struct Outcome {
    pub stdout: String,
    pub code: i32,
}

impl Outcome {
    fn ok(stdout: String) -> Outcome {
        Outcome {
            stdout,
            code: EXIT_OK,
        }
    }
}

pub struct Failure {
    pub message: String,
    pub code: i32,
}

fn misuse(message: &str) -> Failure {
    Failure {
        message: message.to_owned(),
        code: EXIT_MISUSE,
    }
}

fn failed(message: String) -> Failure {
    Failure {
        message,
        code: EXIT_PROBLEM,
    }
}

fn archive_root(args: &Args) -> PathBuf {
    PathBuf::from(args.option("archive").unwrap_or("."))
}

fn open_vault(args: &Args) -> Result<Vault, Failure> {
    Vault::open(&archive_root(args)).map_err(|error| failed(error.0))
}

fn parse_visibility(args: &Args) -> Result<Option<Visibility>, Failure> {
    match args.option("visibility") {
        Some(raw) => Visibility::parse(raw)
            .map(Some)
            .map_err(|error| misuse(&error.0)),
        None => Ok(None),
    }
}

fn cmd_init(args: &Args) -> Result<Outcome, Failure> {
    let Some(path) = args.positional(0) else {
        return Err(misuse(
            "init needs a path: legacy init ./my-archive --subject NAME",
        ));
    };
    let Some(subject) = args.option("subject") else {
        return Err(misuse("init needs --subject NAME"));
    };
    let threshold = args.option_u64("threshold", 3);
    let shares = args.option_u64("shares", 5);
    if threshold == 0 || threshold > shares {
        return Err(misuse(&format!(
            "--threshold {threshold} of --shares {shares} is impossible; need 1 <= threshold <= shares"
        )));
    }

    let vault = Vault::init(Path::new(path), subject, threshold, shares)
        .map_err(|error| failed(error.0))?;

    let mut out = String::new();
    let _ = writeln!(
        out,
        "Initialized archive for {subject:?} at {}",
        vault.root.display()
    );
    let _ = writeln!(
        out,
        "Wrote {}, {}",
        vault.config_path().display(),
        vault.readme_path().display()
    );
    Ok(Outcome::ok(out))
}

fn cmd_story_add(args: &Args) -> Result<Outcome, Failure> {
    let vault = open_vault(args)?;
    let Some(title) = args.option("title") else {
        return Err(misuse("story add needs --title"));
    };

    let body = match args.option("body") {
        Some(body) => body.to_owned(),
        None => read_stdin()?,
    };

    let request = NewStory {
        title: title.to_owned(),
        date: args.option("date").map(str::to_owned),
        body,
        people: args.option_csv("people"),
        places: args.option_csv("places"),
        tags: args.option_csv("tags"),
        visibility: parse_visibility(args)?,
        ..NewStory::default()
    };

    let story = story::new_story(request).map_err(|error| failed(error.0))?;
    let path = story::save_story(&vault.root, &story, false).map_err(|error| failed(error.0))?;
    let relative = path.strip_prefix(&vault.root).unwrap_or(&path);
    Ok(Outcome::ok(format!("Wrote {}\n", relative.display())))
}

fn read_stdin() -> Result<String, Failure> {
    use std::io::Read as _;
    let mut stderr = std::io::stderr();
    let _ignored = writeln!(stderr, "Enter story text, end with Ctrl-D:");
    let mut body = String::new();
    std::io::stdin()
        .read_to_string(&mut body)
        .map_err(|error| failed(format!("failed to read stdin: {error}")))?;
    Ok(body)
}

fn cmd_story_list(args: &Args) -> Result<Outcome, Failure> {
    let vault = open_vault(args)?;
    let stories = story::iter_stories(&vault.root).map_err(|error| failed(error.0))?;

    let year = args
        .option("year")
        .and_then(|value| value.parse::<u32>().ok());
    let person = args.option("person");
    let tag = args.option("tag");

    let mut out = String::new();
    for entry in stories {
        if year.is_some() && entry.fuzzy_date().year() != year {
            continue;
        }
        if let Some(person) = person {
            if !entry.people.iter().any(|value| value == person) {
                continue;
            }
        }
        if let Some(tag) = tag {
            if !entry.tags.iter().any(|value| value == tag) {
                continue;
            }
        }
        let date = entry.date.clone().unwrap_or_else(|| "undated".to_owned());
        let _ = writeln!(
            out,
            "{}\t{date}\t{}\t{}",
            entry.id, entry.visibility, entry.title
        );
    }
    Ok(Outcome::ok(out))
}

fn bank_dir(args: &Args) -> Option<PathBuf> {
    args.option("bank-dir").map(PathBuf::from)
}

fn cmd_interview_banks(args: &Args) -> Result<Outcome, Failure> {
    let mut out = String::new();
    for name in interview::list_banks(bank_dir(args).as_deref()) {
        match interview::load_bank(&name, bank_dir(args).as_deref()) {
            Ok(bank) => {
                let _ = writeln!(
                    out,
                    "{name}\t{} questions\t{}",
                    bank.questions.len(),
                    bank.description
                );
            }
            Err(_unreadable) => {
                let _ = writeln!(out, "{name}\t(could not be read)");
            }
        }
    }
    Ok(Outcome::ok(out))
}

fn cmd_interview_start(args: &Args) -> Result<Outcome, Failure> {
    let vault = open_vault(args)?;
    let Some(bank_name) = args.positional(0) else {
        return Err(misuse(
            "interview start needs a bank name, e.g. `legacy interview start childhood`",
        ));
    };
    let directory = bank_dir(args);
    let bank =
        interview::load_bank(bank_name, directory.as_deref()).map_err(|error| failed(error.0))?;

    let voice = args.has_flag("voice");
    let mode = if voice {
        SessionMode::Voice
    } else {
        SessionMode::Text
    };
    let mut session = interview::new_session(&vault.root, bank_name, mode);
    interview::save_session(&vault.root, &mut session).map_err(|error| failed(error.0))?;

    run_interview(&vault, &mut session, &bank, args)
}

fn cmd_interview_resume(args: &Args) -> Result<Outcome, Failure> {
    let vault = open_vault(args)?;
    let Some(session_id) = args.positional(0) else {
        return Err(misuse("interview resume needs a session id"));
    };
    let mut session =
        interview::load_session(&vault.root, session_id).map_err(|error| failed(error.0))?;
    let directory = bank_dir(args);
    let bank = interview::load_bank(&session.bank, directory.as_deref())
        .map_err(|error| failed(error.0))?;

    if session.status == interview::SessionStatus::Complete {
        return Ok(Outcome::ok(format!(
            "Session {session_id:?} is already complete.\n"
        )));
    }
    run_interview(&vault, &mut session, &bank, args)
}

/// One turn of the interview loop: what the person typed.
enum Turn {
    Answer(String),
    Skip,
    Quit,
}

fn read_answer(voice: bool, vault: &Vault, session_id: &str, question_id: &str) -> Turn {
    use std::io::BufRead as _;

    if voice {
        return read_voice_answer(vault, session_id, question_id);
    }

    let stdin = std::io::stdin();
    let mut lines: Vec<String> = Vec::new();
    loop {
        let mut buffer = String::new();
        print_prompt("> ");
        match stdin.lock().read_line(&mut buffer) {
            Ok(0) => return Turn::Quit,
            Ok(_) => {}
            Err(_unreadable) => return Turn::Quit,
        }
        let trimmed = buffer.trim_end_matches('\n').to_owned();
        let upper = trimmed.trim().to_uppercase();
        if lines.is_empty() && upper == "SKIP" {
            return Turn::Skip;
        }
        if lines.is_empty() && upper == "QUIT" {
            return Turn::Quit;
        }
        if upper == "END" {
            return Turn::Answer(lines.join("\n"));
        }
        lines.push(trimmed);
    }
}

fn read_voice_answer(vault: &Vault, session_id: &str, question_id: &str) -> Turn {
    use crate::core::voice;
    use std::io::BufRead as _;

    println!("Press Enter to start recording, or type SKIP or QUIT.");
    let mut buffer = String::new();
    print_prompt("> ");
    if std::io::stdin().lock().read_line(&mut buffer).is_err() {
        return Turn::Quit;
    }
    match buffer.trim().to_uppercase().as_str() {
        "SKIP" => return Turn::Skip,
        "QUIT" => return Turn::Quit,
        _proceed => {}
    }

    // One WAV per question, not per session, so a session paused and
    // resumed across several sittings never needs to append to an
    // in-progress recording.
    let wav_path = vault
        .interviews_dir()
        .join(format!("{session_id}-{question_id}.wav"));
    let settings = voice::VoiceSettings::from_env();

    if let Err(error) = voice::record_to_wav(&wav_path) {
        eprintln!("{}", error.0);
        return Turn::Quit;
    }
    match voice::transcribe(&settings, &wav_path) {
        Ok(transcript) => {
            println!("Transcript: {transcript}");
            println!(
                "The recording is kept at {} regardless.",
                wav_path.display()
            );
            Turn::Answer(transcript)
        }
        Err(error) => {
            eprintln!("{}", error.0);
            eprintln!(
                "The recording was still saved to {}; transcribe it later.",
                wav_path.display()
            );
            Turn::Quit
        }
    }
}

fn print_prompt(text: &str) {
    print!("{text}");
    let _ignored = std::io::stdout().flush();
}

fn run_interview(
    vault: &Vault,
    session: &mut interview::Session,
    bank: &interview::QuestionBank,
    args: &Args,
) -> Result<Outcome, Failure> {
    let voice = session.mode == SessionMode::Voice;
    let suggest = args.has_flag("suggest-followups");
    let visibility = parse_visibility(args)?.unwrap_or(Visibility::Family);

    println!("Session {:?} ({})", session.session_id, bank.description);
    if voice {
        println!("Voice mode: press Enter to record, Enter again to stop.");
    } else {
        println!("Answer each question, then finish with a line containing just 'END'.");
    }
    println!("Type 'SKIP' alone to skip a question, or 'QUIT' to stop for now.\n");

    loop {
        let Some(question) = interview::next_question(bank, session).cloned() else {
            interview::mark_complete(&vault.root, session).map_err(|error| failed(error.0))?;
            return Ok(Outcome::ok(
                "All questions answered. Session complete.\n".to_owned(),
            ));
        };

        println!("— {}", question.prompt);
        for followup in &question.followups {
            println!("    ({followup})");
        }

        match read_answer(voice, vault, &session.session_id, &question.id) {
            Turn::Quit => {
                return Ok(Outcome::ok(format!(
                    "\nStopped. Resume later with: legacy interview resume {}\n",
                    session.session_id
                )));
            }
            Turn::Skip => {
                interview::skip_question(&vault.root, session, &question)
                    .map_err(|error| failed(error.0))?;
                println!("(skipped)\n");
            }
            Turn::Answer(answer) => {
                let story =
                    interview::record_answer(&vault.root, session, &question, &answer, visibility)
                        .map_err(|error| failed(error.0))?;
                println!("Saved {}\n", story.relative_path().display());

                if suggest {
                    suggest_followup(&question.prompt, &answer);
                }
            }
        }
    }
}

/// Ask the model for one optional follow-up. Always clearly labelled, and
/// a failure here never interrupts the interview.
fn suggest_followup(prompt: &str, answer: &str) {
    use crate::core::llm;
    let settings = llm::LlmSettings::from_env();
    match llm::suggest_followup(&settings, prompt, answer) {
        Ok(followup) => println!("[AI-suggested follow-up, optional]: {followup}\n"),
        Err(error) => eprintln!("(no follow-up suggested: {})\n", error.0),
    }
}

fn cmd_media_ingest(args: &Args) -> Result<Outcome, Failure> {
    let vault = open_vault(args)?;
    let Some(source) = args.positional(0) else {
        return Err(misuse("media ingest needs a source directory"));
    };

    let options = IngestOptions {
        date_from_exif: args.has_flag("date-from-exif"),
        manual_date: args.option("date").map(str::to_owned),
        visibility: parse_visibility(args)?,
    };

    let ingested = media::ingest_directory(Path::new(source), &vault.media_dir(), &options)
        .map_err(|error| failed(error.0))?;

    if ingested.is_empty() {
        return Ok(Outcome::ok(format!(
            "No media files found under {source}\n"
        )));
    }

    let mut out = String::new();
    for item in &ingested {
        let relative = item
            .destination
            .strip_prefix(&vault.root)
            .unwrap_or(&item.destination);
        let name = item
            .source
            .file_name()
            .and_then(|name| name.to_str())
            .unwrap_or("(unnamed)");
        let _ = writeln!(out, "{name} -> {}", relative.display());
    }
    let _ = writeln!(out, "Ingested {} file(s).", ingested.len());
    Ok(Outcome::ok(out))
}

fn cmd_timeline_build(args: &Args) -> Result<Outcome, Failure> {
    let vault = open_vault(args)?;
    let result = timeline::rebuild_derived_state(&vault).map_err(|error| failed(error.0))?;

    let mut out = String::new();
    let _ = writeln!(out, "Indexed {} stories.", result.story_count);
    for path in [
        &result.timeline_path,
        &result.manifest_path,
        &result.readme_path,
    ] {
        let relative = path.strip_prefix(&vault.root).unwrap_or(path);
        let _ = writeln!(out, "Wrote {}", relative.display());
    }
    Ok(Outcome::ok(out))
}

fn cmd_verify(args: &Args) -> Result<Outcome, Failure> {
    let vault = open_vault(args)?;
    let findings = manifest::verify_manifest(&vault.root).map_err(|error| failed(error.0))?;

    let mut out = String::new();
    let mut problems = findings.len();

    for finding in &findings {
        let _ = writeln!(out, "FAIL  {}: {}", finding.path.display(), finding.problem);
    }
    if findings.is_empty() {
        let _ = writeln!(out, "OK    MANIFEST.sha256 matches all files");
    }

    // par2 covers the sealed blobs, which the manifest alone cannot repair.
    if cap::fs::is_dir(&vault.sealed_dir()) {
        let entries = cap::fs::read_dir_sorted(&vault.sealed_dir()).unwrap_or_default();
        for path in entries {
            let Some(name) = path.file_name().and_then(|name| name.to_str()) else {
                continue;
            };
            // Only the index file; the .volNN+NN.par2 blocks are its data.
            if !name.ends_with(".tar.age.par2") {
                continue;
            }
            match crate::core::crypto::par2_verify(&path) {
                Ok(true) => {
                    let _ = writeln!(out, "OK    {name} recovery data intact");
                }
                Ok(false) => {
                    let _ = writeln!(out, "FAIL  {name} recovery data check failed");
                    problems += 1;
                }
                Err(error) => {
                    let _ = writeln!(out, "WARN  {name}: {}", error.0);
                }
            }
        }
    }

    // A missing manifest is reported but is not itself a corruption.
    let corrupted = findings
        .iter()
        .any(|finding| finding.problem != Problem::ManifestAbsent)
        || problems > findings.len();

    Ok(Outcome {
        stdout: out,
        code: if corrupted || !findings.is_empty() {
            EXIT_PROBLEM
        } else {
            EXIT_OK
        },
    })
}

fn cmd_seal(args: &Args) -> Result<Outcome, Failure> {
    let vault = open_vault(args)?;
    let mut config = vault.load_config().map_err(|error| failed(error.0))?;

    let tiers: Vec<String> = if args.option("tiers").is_some() {
        args.option_csv("tiers")
    } else {
        config.tiers.iter().map(|(name, _)| name.clone()).collect()
    };
    let escrow_requested = args.has_flag("plaintext-escrow");

    let mut out = String::new();
    let mut share_lines: Vec<String> = Vec::new();

    for tier in &tiers {
        // Escrow only ever applies to the family tier: leaving
        // executor-only material in the clear would defeat its purpose.
        let escrow = escrow_requested && tier == "family";
        let result =
            seal::seal_tier(&vault, &mut config, tier, escrow).map_err(|error| failed(error.0))?;

        let relative = result
            .output_path
            .strip_prefix(&vault.root)
            .unwrap_or(&result.output_path);

        if result.plaintext_escrow {
            let _ = writeln!(
                out,
                "\n[{tier}] plaintext escrow: {} stories copied UNENCRYPTED to {}",
                result.story_count,
                relative.display()
            );
            continue;
        }

        let _ = writeln!(out, "\n[{tier}] sealed {} stories.", result.story_count);
        let _ = writeln!(out, "  {}", relative.display());
        if result.par2_path.is_none() {
            let _ = writeln!(
                out,
                "  warning: par2 not installed, no recovery data created"
            );
        }
        if let Some(digest) = &result.identity_sha256 {
            let _ = writeln!(out, "  identity sha256: {digest}");
        }

        let threshold = config.tier(tier).map_or(0, |entry| entry.threshold);
        let shares = result.shares.unwrap_or_default();
        let _ = writeln!(
            out,
            "  {} shares generated, {threshold} needed to unlock:",
            shares.len()
        );
        for (position, share) in shares.iter().enumerate() {
            let _ = writeln!(out, "    share {}: {share}", position + 1);
            share_lines.push(format!("# {tier} share {}\n{share}\n", position + 1));
        }
    }

    vault
        .save_config(&config)
        .map_err(|error| failed(error.0))?;
    timeline::rebuild_derived_state(&vault).map_err(|error| failed(error.0))?;

    if !share_lines.is_empty() {
        let _ = writeln!(
            out,
            "\nWARNING: shares were printed above ONLY. They are not stored anywhere in \
             this archive and cannot be recovered if lost. Distribute them to your \
             keyholders now."
        );
    }

    // Writing shares to a file defeats the point, so it takes an explicit
    // flag and says so out loud.
    if let Some(destination) = args.option("write-shares-to") {
        cap::fs::write(Path::new(destination), &share_lines.join("\n"))
            .map_err(|error| failed(format!("failed to write shares: {error}")))?;
        let _ = writeln!(
            out,
            "Also wrote shares to {destination} — this file is now as sensitive as the key."
        );
    }

    Ok(Outcome::ok(out))
}

fn cmd_unseal(args: &Args) -> Result<Outcome, Failure> {
    let vault = open_vault(args)?;
    let config = vault.load_config().map_err(|error| failed(error.0))?;

    let shares = args.option_all("share");
    if shares.is_empty() {
        return Err(misuse(
            "unseal needs at least one --share \"word word ...\" (repeat up to the threshold)",
        ));
    }

    let output_base = args
        .option("output")
        .map(PathBuf::from)
        .unwrap_or_else(|| vault.root.join("unsealed"));

    let result =
        seal::unseal(&vault, &config, &shares, &output_base).map_err(|error| failed(error.0))?;

    Ok(Outcome::ok(format!(
        "Unlocked tier {:?}: {} files\nExtracted to {}\n",
        result.tier,
        result.file_count,
        result.output_dir.display()
    )))
}

fn cmd_tag(args: &Args) -> Result<Outcome, Failure> {
    use crate::core::llm;

    let vault = open_vault(args)?;
    let settings = llm::LlmSettings::from_env();
    // Previewing is the default; writing takes an explicit --apply.
    let apply = args.has_flag("apply");

    let suggestions =
        llm::suggest_tags_for_archive(&settings, &vault.root).map_err(|error| failed(error.0))?;

    let mut out = String::new();
    let mut changed = 0_usize;
    for suggestion in &suggestions {
        let new_tags = suggestion.new_tags();
        if new_tags.is_empty() {
            continue;
        }
        let _ = writeln!(out, "{}: +{}", suggestion.story_id, new_tags.join(", "));
        changed += 1;
        if apply {
            llm::apply_tag_suggestion(&vault.root, suggestion, &settings.model)
                .map_err(|error| failed(error.0))?;
        }
    }

    if changed == 0 {
        let _ = writeln!(out, "No new tags suggested.");
    } else if apply {
        let _ = writeln!(out, "\nApplied tags to {changed} stories.");
    } else {
        let _ = writeln!(
            out,
            "\n(dry run: nothing written. Re-run with --apply to save these tags.)"
        );
    }
    Ok(Outcome::ok(out))
}

fn cmd_ask(args: &Args) -> Result<Outcome, Failure> {
    use crate::core::llm;

    let vault = open_vault(args)?;
    let Some(question) = args.positional(0) else {
        return Err(misuse(
            "ask needs a question: legacy ask \"What did you do after school?\"",
        ));
    };
    let config = vault.load_config().map_err(|error| failed(error.0))?;
    let settings = llm::LlmSettings::from_env();

    let answer = llm::ask(
        &settings,
        &vault.root,
        &vault.index_db_path(),
        question,
        config.replica_sunset.as_deref(),
    )
    .map_err(|error| failed(error.0))?;

    let mut out = String::new();
    let _ = writeln!(out, "{}", answer.answer);
    if !answer.cited_story_ids.is_empty() {
        let _ = writeln!(out, "\nSources: {}", answer.cited_story_ids.join(", "));
    }
    Ok(Outcome::ok(out))
}

fn cmd_tui(args: &Args) -> Result<Outcome, Failure> {
    let vault = open_vault(args)?;
    crate::tui::run(vault).map_err(failed)?;
    Ok(Outcome::ok(String::new()))
}

fn cmd_serve(args: &Args) -> Result<Outcome, Failure> {
    let host = args.option("host").unwrap_or("127.0.0.1").to_owned();
    let port = u16::try_from(args.option_u64("port", 8000))
        .map_err(|_| misuse("--port must be between 0 and 65535"))?;

    crate::api::serve(&host, port).map_err(failed)?;
    Ok(Outcome::ok(String::new()))
}

#[cfg(test)]
mod tests {
    use super::{run, EXIT_MISUSE, EXIT_OK, EXIT_PROBLEM};
    use crate::cap;

    fn argv(tokens: &[&str]) -> Vec<String> {
        tokens.iter().map(|token| (*token).to_owned()).collect()
    }

    #[test]
    fn help_succeeds() {
        assert_eq!(run(&argv(&["--help"])), EXIT_OK);
        assert_eq!(run(&argv(&[])), EXIT_OK);
    }

    #[test]
    fn an_unknown_command_is_misuse() {
        assert_eq!(run(&argv(&["frobnicate"])), EXIT_MISUSE);
    }

    #[test]
    fn init_then_add_then_list_then_build_then_verify() {
        let temp = tempfile::TempDir::new().expect("temp dir");
        let root = temp.path().join("arch");
        let root_text = root.to_str().expect("utf-8 path");

        assert_eq!(
            run(&argv(&["init", root_text, "--subject", "Jane Doe"])),
            EXIT_OK
        );
        assert_eq!(
            run(&argv(&[
                "story",
                "add",
                "--archive",
                root_text,
                "--title",
                "Starting university",
                "--date",
                "1994-09-15",
                "--body",
                "I packed one suitcase.",
            ])),
            EXIT_OK
        );
        assert_eq!(
            run(&argv(&["story", "list", "--archive", root_text])),
            EXIT_OK
        );
        assert_eq!(
            run(&argv(&["timeline", "build", "--archive", root_text])),
            EXIT_OK
        );
        assert_eq!(run(&argv(&["verify", "--archive", root_text])), EXIT_OK);

        assert!(cap::fs::exists(&root.join("timeline.md")));
        assert!(cap::fs::exists(
            &root.join("timeline/1994/1994-09-15-starting-university.md")
        ));
    }

    #[test]
    fn init_refuses_an_impossible_threshold() {
        let temp = tempfile::TempDir::new().expect("temp dir");
        let root = temp.path().join("arch");
        assert_eq!(
            run(&argv(&[
                "init",
                root.to_str().expect("path"),
                "--subject",
                "Jane",
                "--threshold",
                "5",
                "--shares",
                "3",
            ])),
            EXIT_MISUSE
        );
    }

    #[test]
    fn init_without_a_subject_is_misuse() {
        let temp = tempfile::TempDir::new().expect("temp dir");
        assert_eq!(
            run(&argv(&["init", temp.path().to_str().expect("path")])),
            EXIT_MISUSE
        );
    }

    #[test]
    fn verify_reports_tampering_with_a_nonzero_code() {
        let temp = tempfile::TempDir::new().expect("temp dir");
        let root = temp.path().join("arch");
        let root_text = root.to_str().expect("path");

        run(&argv(&["init", root_text, "--subject", "Jane"]));
        run(&argv(&[
            "story",
            "add",
            "--archive",
            root_text,
            "--title",
            "A story",
            "--date",
            "2000",
            "--body",
            "Text.",
        ]));
        run(&argv(&["timeline", "build", "--archive", root_text]));

        let story = root.join("timeline/2000/2000-a-story.md");
        let original = cap::fs::read_to_string(&story).expect("read");
        cap::fs::write(&story, &format!("{original}\ntampered\n")).expect("write");

        assert_eq!(
            run(&argv(&["verify", "--archive", root_text])),
            EXIT_PROBLEM
        );
    }

    #[test]
    fn commands_against_a_missing_archive_fail_cleanly() {
        let temp = tempfile::TempDir::new().expect("temp dir");
        let missing = temp.path().join("nope");
        assert_eq!(
            run(&argv(&[
                "story",
                "list",
                "--archive",
                missing.to_str().expect("path")
            ])),
            EXIT_PROBLEM
        );
    }

    #[test]
    fn story_list_filters_by_tag_and_year() {
        let temp = tempfile::TempDir::new().expect("temp dir");
        let root = temp.path().join("arch");
        let root_text = root.to_str().expect("path");
        run(&argv(&["init", root_text, "--subject", "Jane"]));
        run(&argv(&[
            "story",
            "add",
            "--archive",
            root_text,
            "--title",
            "School",
            "--date",
            "1980",
            "--tags",
            "education",
            "--body",
            "x",
        ]));
        run(&argv(&[
            "story",
            "add",
            "--archive",
            root_text,
            "--title",
            "Work",
            "--date",
            "1990",
            "--tags",
            "career",
            "--body",
            "y",
        ]));

        // Both filters are accepted; exercised for exit status here, with
        // the filtering logic itself covered in core::story.
        assert_eq!(
            run(&argv(&[
                "story",
                "list",
                "--archive",
                root_text,
                "--tag",
                "education"
            ])),
            EXIT_OK
        );
        assert_eq!(
            run(&argv(&[
                "story",
                "list",
                "--archive",
                root_text,
                "--year",
                "1990"
            ])),
            EXIT_OK
        );
    }

    #[test]
    fn interview_banks_lists_the_shipped_set() {
        assert_eq!(run(&argv(&["interview", "banks"])), EXIT_OK);
    }
}
