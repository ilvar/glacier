"""A rebuildable SQLite FTS5 index over story files.

This is never the source of truth -- it can be deleted at any time and
rebuilt from the Markdown files under timeline/ with `legacy timeline build`.
It exists purely so `legacy story list` filters and the replica's retrieval
step (llm.py) don't have to re-parse every file on every query.
"""

from __future__ import annotations

import sqlite3
from pathlib import Path

from legacy.core.story import Story, iter_stories

_SCHEMA = """
CREATE VIRTUAL TABLE IF NOT EXISTS stories USING fts5(
    id UNINDEXED,
    title,
    body,
    tags,
    people,
    places,
    date UNINDEXED,
    year UNINDEXED,
    visibility UNINDEXED
);
"""


def rebuild_index(archive_root: Path, db_path: Path) -> int:
    """Rebuild the FTS index from scratch. Returns the number of stories indexed."""
    db_path.parent.mkdir(parents=True, exist_ok=True)
    if db_path.exists():
        db_path.unlink()
    conn = sqlite3.connect(db_path)
    try:
        conn.executescript(_SCHEMA)
        count = 0
        for story in iter_stories(archive_root):
            _insert(conn, story)
            count += 1
        conn.commit()
        return count
    finally:
        conn.close()


def _insert(conn: sqlite3.Connection, story: Story) -> None:
    conn.execute(
        "INSERT INTO stories (id, title, body, tags, people, places, date, year, visibility) "
        "VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?)",
        (
            story.id,
            story.title,
            story.body,
            " ".join(story.tags),
            " ".join(story.people),
            " ".join(story.places),
            story.date or "",
            story.fuzzy_date.year or 0,
            story.visibility,
        ),
    )


def search(
    db_path: Path,
    query: str,
    visibilities: list[str] | None = None,
    limit: int = 10,
) -> list[dict]:
    """Full-text search over indexed stories. `visibilities` restricts results
    to those tiers (used by the replica to enforce visibility gating)."""
    if not db_path.exists():
        raise FileNotFoundError(f"no index at {db_path}; run `legacy timeline build` first")
    conn = sqlite3.connect(db_path)
    conn.row_factory = sqlite3.Row
    try:
        sql = (
            "SELECT id, title, date, visibility, "
            "snippet(stories, 2, '[', ']', '...', 12) AS snippet "
            "FROM stories WHERE stories MATCH ?"
        )
        params: list = [query]
        if visibilities is not None:
            placeholders = ",".join("?" for _ in visibilities)
            sql += f" AND visibility IN ({placeholders})"
            params.extend(visibilities)
        sql += " ORDER BY rank LIMIT ?"
        params.append(limit)
        rows = conn.execute(sql, params).fetchall()
        return [dict(row) for row in rows]
    finally:
        conn.close()
