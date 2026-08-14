"""Builds timeline.md: a single, human-readable chronological index of every
story in the archive. Purely derived -- delete it and `legacy timeline build`
brings it back.
"""

from __future__ import annotations

from dataclasses import dataclass
from pathlib import Path

from legacy.core.story import Story, iter_stories

TIMELINE_FILENAME = "timeline.md"


@dataclass
class RebuildResult:
    story_count: int
    timeline_path: Path
    index_path: Path
    manifest_path: Path
    readme_path: Path


def _group_by_year(stories: list[Story]) -> list[tuple[str, list[Story]]]:
    groups: dict[str, list[Story]] = {}
    for story in sorted(stories, key=lambda s: s.fuzzy_date.sort_key):
        year = story.fuzzy_date.year
        label = str(year) if year is not None else "Undated"
        groups.setdefault(label, []).append(story)
    # sort_key already gives chronological order; preserve first-seen (already sorted) order
    return list(groups.items())


def render_timeline(stories: list[Story]) -> str:
    lines = ["# Timeline", ""]
    if not stories:
        lines.append("(no stories yet)")
        return "\n".join(lines) + "\n"

    for label, group in _group_by_year(stories):
        lines.append(f"## {label}")
        lines.append("")
        for story in group:
            date_label = story.date or "date unknown"
            rel_path = story.relative_path().as_posix()
            lines.append(f"- **{story.title}** ({date_label}) — [{story.id}]({rel_path})")
        lines.append("")
    return "\n".join(lines).rstrip() + "\n"


def build_timeline(archive_root: Path) -> Path:
    stories = list(iter_stories(archive_root))
    path = archive_root / TIMELINE_FILENAME
    path.write_text(render_timeline(stories), encoding="utf-8")
    return path


def rebuild_derived_state(vault) -> RebuildResult:
    """Regenerate everything that is derived from the plain files: README.txt
    (in case archive.yaml changed), timeline.md, the SQLite FTS index, and
    MANIFEST.sha256 (written last, so it covers the other three)."""
    from legacy.core.index import rebuild_index
    from legacy.core.manifest import write_manifest
    from legacy.core.readme import render_readme

    config = vault.load_config()
    vault.readme_path.write_text(render_readme(config), encoding="utf-8")

    stories = list(iter_stories(vault.root))
    timeline_path = vault.root / TIMELINE_FILENAME
    timeline_path.write_text(render_timeline(stories), encoding="utf-8")

    index_path = vault.index_db_path
    rebuild_index(vault.root, index_path)

    manifest_path = write_manifest(vault.root)

    return RebuildResult(
        story_count=len(stories),
        timeline_path=timeline_path,
        index_path=index_path,
        manifest_path=manifest_path,
        readme_path=vault.readme_path,
    )
