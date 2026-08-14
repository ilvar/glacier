"""legacy: command-line entry point. Every command here has a REST twin in api.py."""

from __future__ import annotations

from pathlib import Path

import typer

from legacy.core import story as story_mod
from legacy.core.vault import Vault, VaultError

app = typer.Typer(no_args_is_help=True, add_completion=False)
story_app = typer.Typer(no_args_is_help=True, help="Manage story files.")
app.add_typer(story_app, name="story")


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


if __name__ == "__main__":
    app()
