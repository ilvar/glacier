"""legacy: command-line entry point. Every command here has a REST twin in api.py."""

from __future__ import annotations

from pathlib import Path

import typer

from legacy.core import interview as interview_mod
from legacy.core import llm as llm_mod
from legacy.core import manifest as manifest_mod
from legacy.core import media as media_mod
from legacy.core import seal as seal_mod
from legacy.core import story as story_mod
from legacy.core import timeline as timeline_mod
from legacy.core import voice as voice_mod
from legacy.core.vault import Vault, VaultError

app = typer.Typer(no_args_is_help=True, add_completion=False)
story_app = typer.Typer(no_args_is_help=True, help="Manage story files.")
timeline_app = typer.Typer(no_args_is_help=True, help="Chronological index and search.")
media_app = typer.Typer(no_args_is_help=True, help="Ingest and manage media files.")
interview_app = typer.Typer(no_args_is_help=True, help="Guided interview sessions.")
app.add_typer(story_app, name="story")
app.add_typer(timeline_app, name="timeline")
app.add_typer(media_app, name="media")
app.add_typer(interview_app, name="interview")


def _err(msg: str) -> None:
    typer.secho(msg, fg=typer.colors.RED, err=True)
    raise typer.Exit(code=1)


@app.command()
def init(
    path: Path = typer.Argument(..., help="Directory to create the archive in."),
    subject: str = typer.Option(..., "--subject", help="Name of the person this archive is about."),
    threshold: int = typer.Option(
        3, "--threshold", help="Shares required to unlock (default tier config)."
    ),
    shares: int = typer.Option(
        5, "--shares", help="Total shares to generate (default tier config)."
    ),
):
    """Create a new, empty archive."""
    try:
        vault = Vault.init(path, subject_name=subject, threshold=threshold, shares=shares)
    except VaultError as e:
        _err(str(e))
        return
    typer.echo(f"Initialized archive for {subject!r} at {vault.root}")
    typer.echo(f"Wrote {vault.config_path}, {vault.readme_path}")


def _split_csv(value: str | None) -> list[str]:
    if not value:
        return []
    return [v.strip() for v in value.split(",") if v.strip()]


@story_app.command("add")
def story_add(
    archive: Path = typer.Option(Path("."), "--archive", "-a", help="Archive root."),
    title: str = typer.Option(..., "--title"),
    date: str | None = typer.Option(
        None, "--date", help="YYYY-MM-DD, YYYY-MM, YYYY, YYYYs, or omitted."
    ),
    body: str | None = typer.Option(
        None, "--body", help="Story text. If omitted, reads from stdin."
    ),
    people: str | None = typer.Option(None, "--people", help="Comma-separated person slugs."),
    places: str | None = typer.Option(None, "--places", help="Comma-separated place slugs."),
    tags: str | None = typer.Option(None, "--tags", help="Comma-separated tags."),
    visibility: str = typer.Option(
        "family", "--visibility", help="public | family | executor-only"
    ),
):
    """Add a new story file."""
    vault = Vault.open(archive)
    if body is None:
        typer.echo("Enter story text, end with Ctrl-D:", err=True)
        body = typer.get_text_stream("stdin").read()

    try:
        story = story_mod.new_story(
            title=title,
            date=date,
            body=body,
            people=_split_csv(people),
            places=_split_csv(places),
            tags=_split_csv(tags),
            visibility=visibility,
        )
        path = story_mod.save_story(vault.root, story)
    except story_mod.StoryError as e:
        _err(str(e))
        return
    typer.echo(f"Wrote {path.relative_to(vault.root)}")


@story_app.command("list")
def story_list(
    archive: Path = typer.Option(Path("."), "--archive", "-a", help="Archive root."),
    year: int | None = typer.Option(None, "--year"),
    person: str | None = typer.Option(None, "--person"),
    tag: str | None = typer.Option(None, "--tag"),
):
    """List stories, optionally filtered."""
    vault = Vault.open(archive)
    for story in story_mod.iter_stories(vault.root):
        if year is not None and story.fuzzy_date.year != year:
            continue
        if person is not None and person not in story.people:
            continue
        if tag is not None and tag not in story.tags:
            continue
        date_label = story.date or "undated"
        typer.echo(f"{story.id}\t{date_label}\t{story.visibility}\t{story.title}")


def _get_text_answer() -> str | None:
    """Reads lines until 'END'. Returns None for SKIP, raises _Quit for QUIT."""
    lines: list[str] = []
    while True:
        try:
            line = input("> ")
        except EOFError:
            raise _Quit() from None
        stripped = line.strip()
        if not lines and stripped.upper() == "SKIP":
            return None
        if not lines and stripped.upper() == "QUIT":
            raise _Quit()
        if stripped.upper() == "END":
            return "\n".join(lines)
        lines.append(line)


def _get_voice_answer(vault: Vault, session, question, voice_settings) -> str | None:
    typer.echo("Type SKIP or QUIT, or press Enter to start recording.")
    try:
        first = input("> ").strip().upper()
    except EOFError:
        raise _Quit() from None
    if first == "SKIP":
        return None
    if first == "QUIT":
        raise _Quit()

    wav_path = vault.interviews_dir / f"{session.session_id}-{question.id}.wav"
    try:
        voice_mod.record_to_wav(wav_path)
        transcript = voice_mod.transcribe(voice_settings, wav_path)
    except voice_mod.VoiceError as e:
        _err(str(e))
        raise _Quit() from None
    typer.echo(f"Transcript: {transcript}")
    confirm = input("Use this? [Y/n] ").strip().lower()
    if confirm == "n":
        typer.echo("Not saved; the recording is kept, resume this session to try again.\n")
        return None
    return transcript


class _Quit(Exception):
    pass


def _run_interview_loop(
    vault: Vault,
    session,
    bank,
    *,
    voice: bool = False,
    voice_settings=None,
    suggest_followups: bool = False,
    llm_settings=None,
) -> None:
    typer.echo(f"Session {session.session_id!r} ({bank.description})")
    if voice:
        typer.echo("Voice mode: press Enter to record, Enter again to stop.")
    else:
        typer.echo("Answer each question, then finish with a line containing just 'END'.")
    typer.echo("Type 'SKIP' alone to skip a question, or 'QUIT' to stop for now.\n")

    while True:
        question = interview_mod.next_question(bank, session)
        if question is None:
            interview_mod.mark_complete(vault.root, session)
            typer.secho("All questions answered. Session complete.", fg=typer.colors.GREEN)
            return

        typer.echo(f"— {question.prompt}")
        for followup in question.followups:
            typer.echo(f"    ({followup})")

        try:
            answer = (
                _get_voice_answer(vault, session, question, voice_settings)
                if voice
                else _get_text_answer()
            )
        except _Quit:
            typer.echo(
                f"\nStopped. Resume later with: legacy interview resume {session.session_id}"
            )
            return

        if answer is None:
            interview_mod.skip_question(vault.root, session, question)
            typer.echo("(skipped)\n")
            continue

        story = interview_mod.record_answer(vault.root, session, bank, question, answer)
        typer.echo(f"Saved {story.relative_path()}\n")

        if suggest_followups and llm_settings is not None:
            try:
                followup = llm_mod.suggest_followup(llm_settings, question.prompt, answer)
                typer.secho(
                    f"[AI-suggested follow-up, optional]: {followup}\n", fg=typer.colors.CYAN
                )
            except llm_mod.LLMError:
                pass


@interview_app.command("start")
def interview_start(
    bank_name: str = typer.Argument(..., help="Question bank name, e.g. 'childhood'."),
    archive: Path = typer.Option(Path("."), "--archive", "-a", help="Archive root."),
    voice: bool = typer.Option(
        False, "--voice", help="Record and transcribe answers with speech."
    ),
    suggest_followups: bool = typer.Option(
        False,
        "--suggest-followups",
        help="Ask an LLM to suggest one optional follow-up per answer.",
    ),
):
    """Start a new interview session from a question bank."""
    vault = Vault.open(archive)
    try:
        bank = interview_mod.load_bank(bank_name)
        mode = "voice" if voice else "text"
        session = interview_mod.new_session(vault.root, bank_name, mode=mode)
    except interview_mod.InterviewError as e:
        _err(str(e))
        return
    interview_mod.save_session(vault.root, session)
    _run_interview_loop(
        vault,
        session,
        bank,
        voice=voice,
        voice_settings=voice_mod.VoiceSettings.from_env() if voice else None,
        suggest_followups=suggest_followups,
        llm_settings=llm_mod.LLMSettings.from_env() if suggest_followups else None,
    )


@interview_app.command("resume")
def interview_resume(
    session_id: str = typer.Argument(..., help="Session id, e.g. 2026-08-14-childhood-session-01."),
    archive: Path = typer.Option(Path("."), "--archive", "-a", help="Archive root."),
    suggest_followups: bool = typer.Option(
        False,
        "--suggest-followups",
        help="Ask an LLM to suggest one optional follow-up per answer.",
    ),
):
    """Resume an existing interview session."""
    vault = Vault.open(archive)
    try:
        session = interview_mod.load_session(vault.root, session_id)
        bank = interview_mod.load_bank(session.bank)
    except interview_mod.InterviewError as e:
        _err(str(e))
        return
    if session.status == "complete":
        typer.echo(f"Session {session_id!r} is already complete.")
        return
    voice = session.mode == "voice"
    _run_interview_loop(
        vault,
        session,
        bank,
        voice=voice,
        voice_settings=voice_mod.VoiceSettings.from_env() if voice else None,
        suggest_followups=suggest_followups,
        llm_settings=llm_mod.LLMSettings.from_env() if suggest_followups else None,
    )


@media_app.command("ingest")
def media_ingest(
    source: Path = typer.Argument(..., help="Directory of media files to ingest."),
    archive: Path = typer.Option(Path("."), "--archive", "-a", help="Archive root."),
    date_from_exif: bool = typer.Option(
        False, "--date-from-exif", help="Try to read a capture date from embedded metadata."
    ),
    date: str | None = typer.Option(
        None, "--date", help="Override date for every file ingested this run (YYYY[-MM[-DD]])."
    ),
    visibility: str = typer.Option(
        "family", "--visibility", help="public | family | executor-only"
    ),
):
    """Copy media files into the archive with a metadata sidecar per file."""
    vault = Vault.open(archive)
    try:
        ingested = media_mod.ingest_directory(
            source,
            vault.media_dir,
            date_from_exif=date_from_exif,
            manual_date=date,
            visibility=visibility,
        )
    except media_mod.MediaError as e:
        _err(str(e))
        return
    if not ingested:
        typer.echo(f"No media files found under {source}")
        return
    for item in ingested:
        typer.echo(f"{item.source.name} -> {item.dest.relative_to(vault.root)}")
    typer.echo(f"Ingested {len(ingested)} file(s).")


@app.command()
def seal(
    archive: Path = typer.Option(Path("."), "--archive", "-a", help="Archive root."),
    tiers: str = typer.Option(
        "family,executor-only", "--tiers", help="Comma-separated tiers to seal."
    ),
    plaintext_escrow: bool = typer.Option(
        False,
        "--plaintext-escrow",
        help="Leave the family tier UNENCRYPTED under sealed/family/ (see README.txt section 6).",
    ),
    write_shares_to: Path | None = typer.Option(
        None, "--write-shares-to", help="Also write shares to this file. Off by default."
    ),
):
    """Encrypt each visibility tier into sealed/<tier>.tar.age and print its shares."""
    vault = Vault.open(archive)
    config = vault.load_config()
    tier_names = [t.strip() for t in tiers.split(",") if t.strip()]

    share_lines: list[str] = []
    for tier in tier_names:
        escrow = plaintext_escrow and tier == "family"
        try:
            result = seal_mod.seal_tier(vault, config, tier, plaintext_escrow=escrow)
        except seal_mod.SealError as e:
            _err(str(e))
            return

        if result.plaintext_escrow:
            typer.secho(
                f"\n[{tier}] plaintext escrow: {result.story_count} stories copied "
                f"UNENCRYPTED to {result.output_path.relative_to(vault.root)}",
                fg=typer.colors.YELLOW,
            )
            continue

        typer.secho(f"\n[{tier}] sealed {result.story_count} stories.", fg=typer.colors.GREEN)
        typer.echo(f"  {result.output_path.relative_to(vault.root)}")
        if result.par2_path is None:
            typer.secho(
                "  warning: par2 not installed, no recovery data created", fg=typer.colors.YELLOW
            )
        typer.echo(f"  identity sha256: {result.identity_sha256}")
        typer.echo(
            f"  {len(result.shares)} shares generated, "
            f"{config.tiers[tier].threshold} needed to unlock:"
        )
        for i, share in enumerate(result.shares, 1):
            typer.echo(f"    share {i}: {share}")
            share_lines.append(f"# {tier} share {i}\n{share}\n")

    vault.save_config(config)
    timeline_mod.rebuild_derived_state(vault)

    if share_lines:
        typer.secho(
            "\nWARNING: shares were printed above ONLY. They are not stored anywhere in "
            "this archive and cannot be recovered if lost. Distribute them to your "
            "keyholders now.",
            fg=typer.colors.RED,
            bold=True,
        )
    if write_shares_to is not None:
        write_shares_to.write_text("\n".join(share_lines), encoding="utf-8")
        typer.secho(f"Also wrote shares to {write_shares_to}", fg=typer.colors.YELLOW)


@app.command()
def unseal(
    share: list[str] = typer.Option(
        ..., "--share", help="A share (repeat for each one). Needs the tier's threshold."
    ),
    archive: Path = typer.Option(Path("."), "--archive", "-a", help="Archive root."),
    output: Path | None = typer.Option(
        None, "--output", help="Base directory to extract into (default: <archive>/unsealed/)."
    ),
):
    """Reconstruct a key from shares and decrypt the matching sealed tier."""
    vault = Vault.open(archive)
    config = vault.load_config()
    try:
        result = seal_mod.unseal(vault, config, share, output or (vault.root / "unsealed"))
    except seal_mod.SealError as e:
        _err(str(e))
        return
    typer.secho(f"Unlocked tier {result.tier!r}: {result.file_count} files", fg=typer.colors.GREEN)
    typer.echo(f"Extracted to {result.output_dir}")


@timeline_app.command("build")
def timeline_build(
    archive: Path = typer.Option(Path("."), "--archive", "-a", help="Archive root."),
):
    """Regenerate timeline.md, the search index, MANIFEST.sha256, and README.txt."""
    vault = Vault.open(archive)
    result = timeline_mod.rebuild_derived_state(vault)
    typer.echo(f"Indexed {result.story_count} stories.")
    typer.echo(f"Wrote {result.timeline_path.relative_to(vault.root)}")
    typer.echo(f"Wrote {result.manifest_path.relative_to(vault.root)}")
    typer.echo(f"Wrote {result.readme_path.relative_to(vault.root)}")


@app.command()
def tag(
    archive: Path = typer.Option(Path("."), "--archive", "-a", help="Archive root."),
    dry_run: bool = typer.Option(
        True, "--dry-run/--apply", help="Preview suggestions without writing (default)."
    ),
):
    """Suggest tags for stories via an LLM. Always previewable with --dry-run (the default)."""
    vault = Vault.open(archive)
    settings = llm_mod.LLMSettings.from_env()
    try:
        suggestions = llm_mod.suggest_tags_for_archive(settings, vault.root)
    except llm_mod.LLMError as e:
        _err(str(e))
        return

    if not suggestions:
        typer.echo("No stories need tagging (all already have model-suggested tags).")
        return

    for suggestion in suggestions:
        if not suggestion.new_tags:
            continue
        typer.echo(f"{suggestion.story_id}: +{', '.join(suggestion.new_tags)}")
        if not dry_run:
            from legacy.core.story import iter_stories as _iter

            story = next(s for s in _iter(vault.root) if s.id == suggestion.story_id)
            llm_mod.apply_tag_suggestion(vault.root, story, suggestion, settings.model)

    if dry_run:
        typer.echo("\n(dry run: nothing written. Re-run with --apply to save these tags.)")
    else:
        typer.echo(f"\nApplied tags to {sum(1 for s in suggestions if s.new_tags)} stories.")


@app.command()
def ask(
    question: str = typer.Argument(..., help="A question to ask the replica."),
    archive: Path = typer.Option(Path("."), "--archive", "-a", help="Archive root."),
):
    """Ask the replica a question; it answers only from indexed stories, with citations."""
    vault = Vault.open(archive)
    config = vault.load_config()
    settings = llm_mod.LLMSettings.from_env()
    try:
        result = llm_mod.ask(
            settings,
            vault.root,
            vault.index_db_path,
            question,
            replica_sunset=config.replica_sunset,
        )
    except llm_mod.LLMError as e:
        _err(str(e))
        return
    typer.echo(result.answer)
    if result.cited_story_ids:
        typer.echo(f"\nSources: {', '.join(result.cited_story_ids)}")


@app.command()
def verify(
    archive: Path = typer.Option(Path("."), "--archive", "-a", help="Archive root."),
):
    """Check MANIFEST.sha256 against the files on disk, and par2-verify sealed blobs."""
    vault = Vault.open(archive)
    problems = manifest_mod.verify_manifest(vault.root)
    for rel, reason in problems:
        typer.secho(f"FAIL  {rel}: {reason}", fg=typer.colors.RED)
    if not problems:
        typer.secho("OK    MANIFEST.sha256 matches all files", fg=typer.colors.GREEN)

    from legacy.core import crypto

    if vault.sealed_dir.exists():
        for par2_file in sorted(vault.sealed_dir.glob("*.par2")):
            if par2_file.name.count(".") > 1 and not par2_file.name.endswith(".tar.age.par2"):
                continue  # skip the .vol*.par2 recovery-block files, only check the index
            try:
                ok = crypto.par2_verify(par2_file)
            except crypto.CryptoError as e:
                typer.secho(f"WARN  {par2_file.name}: {e}", fg=typer.colors.YELLOW)
                continue
            if ok:
                typer.secho(
                    f"OK    {par2_file.name} recovery data intact", fg=typer.colors.GREEN
                )
            else:
                typer.secho(
                    f"FAIL  {par2_file.name} recovery data check failed", fg=typer.colors.RED
                )
                problems.append((par2_file, "par2 verification failed"))

    if problems:
        raise typer.Exit(code=1)


@app.command()
def serve(
    host: str = typer.Option(
        "127.0.0.1", "--host", help="Bind address. Localhost only by default."
    ),
    port: int = typer.Option(8000, "--port", help="Port to listen on."),
):
    """Run the REST API (mirrors this CLI 1:1). No auth -- localhost only by default;
    put a reverse proxy with auth in front if you expose this beyond your own machine."""
    import uvicorn

    from legacy.api import app as fastapi_app

    uvicorn.run(fastapi_app, host=host, port=port)


if __name__ == "__main__":
    app()
