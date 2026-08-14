"""Fuzzy date parsing shared by story, timeline, and media ingestion.

Life stories rarely come with a precise date. We support four precise forms
(day/month/year/decade) plus "unknown", and never invent precision the user
did not give us.
"""

from __future__ import annotations

import re
from dataclasses import dataclass

_DAY = re.compile(r"^(?P<y>\d{4})-(?P<m>\d{2})-(?P<d>\d{2})$")
_MONTH = re.compile(r"^(?P<y>\d{4})-(?P<m>\d{2})$")
_YEAR = re.compile(r"^(?P<y>\d{4})$")
_DECADE = re.compile(r"^(?P<y>\d{3})0s$")

PRECISIONS = ("day", "month", "year", "decade", "unknown")


@dataclass(frozen=True)
class FuzzyDate:
    """A date known to some precision, or not known at all.

    `value` is the canonical string stored in frontmatter (e.g. "1994-09-15",
    "1994-09", "1994", "1970s"), or None when precision is "unknown".
    `year` is used to pick the timeline/<year>/ bucket, or None for undated.
    """

    value: str | None
    precision: str
    year: int | None

    @property
    def sort_key(self) -> tuple[int, int, int]:
        """Best-effort chronological sort key; unknown dates sort last."""
        if self.value is None:
            return (9999, 99, 99)
        m = _DAY.match(self.value)
        if m:
            return (int(m["y"]), int(m["m"]), int(m["d"]))
        m = _MONTH.match(self.value)
        if m:
            return (int(m["y"]), int(m["m"]), 0)
        m = _YEAR.match(self.value)
        if m:
            return (int(m["y"]), 0, 0)
        m = _DECADE.match(self.value)
        if m:
            return (int(m["y"]) * 10, 0, 0)
        return (9999, 99, 99)


def parse_fuzzy_date(raw: str | None) -> FuzzyDate:
    """Parse a user-supplied date string into a FuzzyDate.

    Accepts: YYYY-MM-DD, YYYY-MM, YYYY, YYYYs (decade), "unknown", or None.
    Raises ValueError on anything else rather than guessing.
    """
    if raw is None or raw.strip().lower() in ("", "unknown"):
        return FuzzyDate(value=None, precision="unknown", year=None)

    raw = raw.strip()
    if _DAY.match(raw):
        return FuzzyDate(value=raw, precision="day", year=int(raw[:4]))
    if _MONTH.match(raw):
        return FuzzyDate(value=raw, precision="month", year=int(raw[:4]))
    if _YEAR.match(raw):
        return FuzzyDate(value=raw, precision="year", year=int(raw))
    if _DECADE.match(raw):
        decade = int(raw[:3]) * 10
        return FuzzyDate(value=raw, precision="decade", year=decade)
    raise ValueError(
        f"Unrecognized date {raw!r}; use YYYY-MM-DD, YYYY-MM, YYYY, YYYYs, or 'unknown'"
    )


def fuzzy_date_from_frontmatter(value: str | None, precision: str | None) -> FuzzyDate:
    """Reconstruct a FuzzyDate from stored frontmatter fields (trusts precision)."""
    if value is None or precision == "unknown":
        return FuzzyDate(value=None, precision="unknown", year=None)
    m = _DECADE.match(value)
    if m:
        return FuzzyDate(value=value, precision="decade", year=int(value[:3]) * 10)
    return FuzzyDate(value=value, precision=precision or "unknown", year=int(value[:4]))
