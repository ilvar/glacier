"""Story files: markdown with YAML frontmatter, the atomic unit of the archive.

A Story is read and written as a single ``.md`` file. The frontmatter schema
below is intentionally small and stable -- see the top-level README for the
rationale. Do not add fields without updating README.md and the schema
version in archive.yaml.
"""

from __future__ import annotations

import re
from dataclasses import dataclass, field
from pathlib import Path

import yaml

from legacy.core.dates import FuzzyDate, fuzzy_date_from_frontmatter, parse_fuzzy_date

VISIBILITIES = ("public", "family", "executor-only")

# Field order is fixed so every story file diffs cleanly and reads the same way.
_FIELD_ORDER = [
    "id",
    "title",
    "date",
    "date_precision",
    "people",
    "places",
    "tags",
    "media",
    "source",
    "recorded_at",
    "tags_generated_by",
    "visibility",
]

_FRONTMATTER_RE = re.compile(r"\A---\n(?P<fm>.*?)\n---\n(?P<body>.*)\Z", re.DOTALL)


class StoryError(ValueError):
    pass


def slugify(text: str) -> str:
    text = text.strip().lower()
    text = re.sub(r"[^a-z0-9]+", "-", text)
    return text.strip("-")


@dataclass
class Story:
    id: str
    title: str
    body: str = ""
    date: str | None = None
    date_precision: str = "unknown"
    people: list[str] = field(default_factory=list)
    places: list[str] = field(default_factory=list)
    tags: list[str] = field(default_factory=list)
    media: list[str] = field(default_factory=list)
    source: str | None = None
    recorded_at: str | None = None
    tags_generated_by: str | None = None
    visibility: str = "family"

    @property
    def fuzzy_date(self) -> FuzzyDate:
        return fuzzy_date_from_frontmatter(self.date, self.date_precision)

    def relative_path(self) -> Path:
        year = self.fuzzy_date.year
        bucket = str(year) if year is not None else "undated"
        return Path("timeline") / bucket / f"{self.id}.md"

    def to_frontmatter(self) -> dict:
        if self.visibility not in VISIBILITIES:
            raise StoryError(
                f"invalid visibility {self.visibility!r}, must be one of {VISIBILITIES}"
            )
        data = {
            "id": self.id,
            "title": self.title,
            "date": self.date,
            "date_precision": self.date_precision,
            "people": list(self.people),
            "places": list(self.places),
            "tags": list(self.tags),
            "media": list(self.media),
            "source": self.source,
            "recorded_at": self.recorded_at,
            "tags_generated_by": self.tags_generated_by,
            "visibility": self.visibility,
        }
        return {k: data[k] for k in _FIELD_ORDER}

    def render(self) -> str:
        fm = yaml.safe_dump(
            self.to_frontmatter(), sort_keys=False, allow_unicode=True, default_flow_style=False
        )
        body = self.body.strip("\n")
        return f"---\n{fm}---\n\n{body}\n"


def new_story(
    title: str,
    date: str | None = None,
    body: str = "",
    people: list[str] | None = None,
    places: list[str] | None = None,
    tags: list[str] | None = None,
    media: list[str] | None = None,
    source: str | None = None,
    recorded_at: str | None = None,
    visibility: str = "family",
    story_id: str | None = None,
) -> Story:
    fuzzy = parse_fuzzy_date(date)
    slug = slugify(title)
    if not slug:
        raise StoryError("title must contain at least one alphanumeric character")
    if story_id is None:
        story_id = f"{fuzzy.value}-{slug}" if fuzzy.value else slug
    return Story(
        id=story_id,
        title=title,
        body=body,
        date=fuzzy.value,
        date_precision=fuzzy.precision,
        people=people or [],
        places=places or [],
        tags=tags or [],
        media=media or [],
        source=source,
        recorded_at=recorded_at,
        tags_generated_by=None,
        visibility=visibility,
    )


def parse_story(text: str, story_id_hint: str | None = None) -> Story:
    m = _FRONTMATTER_RE.match(text)
    if not m:
        raise StoryError("file does not contain a --- frontmatter block")
    fm = yaml.safe_load(m.group("fm")) or {}
    body = m.group("body")
    try:
        return Story(
            id=fm["id"],
            title=fm["title"],
            body=body,
            date=fm.get("date"),
            date_precision=fm.get("date_precision", "unknown"),
            people=fm.get("people") or [],
            places=fm.get("places") or [],
            tags=fm.get("tags") or [],
            media=fm.get("media") or [],
            source=fm.get("source"),
            recorded_at=fm.get("recorded_at"),
            tags_generated_by=fm.get("tags_generated_by"),
            visibility=fm.get("visibility", "family"),
        )
    except KeyError as e:
        raise StoryError(f"missing required frontmatter field: {e}") from e


def load_story(path: Path) -> Story:
    return parse_story(path.read_text(encoding="utf-8"), story_id_hint=path.stem)


def save_story(archive_root: Path, story: Story, *, overwrite: bool = False) -> Path:
    path = archive_root / story.relative_path()
    if path.exists() and not overwrite:
        raise StoryError(f"story {story.id!r} already exists at {path}")
    path.parent.mkdir(parents=True, exist_ok=True)
    path.write_text(story.render(), encoding="utf-8")
    return path


def iter_stories(archive_root: Path):
    timeline_dir = archive_root / "timeline"
    if not timeline_dir.exists():
        return
    for path in sorted(timeline_dir.glob("*/*.md")):
        yield load_story(path)
