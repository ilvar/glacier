"""Media ingestion: copies original files into media/, verbatim, with a
YAML sidecar describing each one. Never transcodes, never discards the
original -- the sidecar is purely descriptive metadata alongside it.
"""

from __future__ import annotations

import datetime
import json
import mimetypes
import shutil
import subprocess
from dataclasses import dataclass, field
from pathlib import Path

import yaml

from legacy.core.crypto import sha256_file
from legacy.core.dates import FuzzyDate, fuzzy_date_from_frontmatter
from legacy.core.story import VISIBILITIES, slugify

_MEDIA_EXTENSIONS = {
    ".jpg", ".jpeg", ".png", ".heic", ".tif", ".tiff", ".gif", ".bmp", ".webp",
    ".wav", ".mp3", ".m4a", ".flac", ".ogg",
    ".mp4", ".mov", ".avi", ".mkv", ".webm",
}

_SIDECAR_FIELD_ORDER = [
    "original_filename",
    "ingested_at",
    "sha256",
    "size_bytes",
    "mime_type",
    "date",
    "date_precision",
    "date_source",
    "width",
    "height",
    "duration_seconds",
    "visibility",
    "caption",
]


class MediaError(ValueError):
    pass


@dataclass
class MediaSidecar:
    original_filename: str
    sha256: str
    size_bytes: int
    mime_type: str | None = None
    date: str | None = None
    date_precision: str = "unknown"
    date_source: str = "unknown"  # exif | filesystem | manual | unknown
    width: int | None = None
    height: int | None = None
    duration_seconds: float | None = None
    visibility: str = "family"
    caption: str | None = None
    ingested_at: str = field(default_factory=lambda: _now_iso())

    def to_dict(self) -> dict:
        if self.visibility not in VISIBILITIES:
            raise MediaError(f"invalid visibility {self.visibility!r}")
        data = {
            "original_filename": self.original_filename,
            "ingested_at": self.ingested_at,
            "sha256": self.sha256,
            "size_bytes": self.size_bytes,
            "mime_type": self.mime_type,
            "date": self.date,
            "date_precision": self.date_precision,
            "date_source": self.date_source,
            "width": self.width,
            "height": self.height,
            "duration_seconds": self.duration_seconds,
            "visibility": self.visibility,
            "caption": self.caption,
        }
        return {k: data[k] for k in _SIDECAR_FIELD_ORDER}

    def render(self) -> str:
        return yaml.safe_dump(self.to_dict(), sort_keys=False, allow_unicode=True)


@dataclass
class IngestedFile:
    source: Path
    dest: Path
    sidecar_path: Path
    sidecar: MediaSidecar


def _now_iso() -> str:
    return datetime.datetime.now(datetime.UTC).strftime("%Y-%m-%dT%H:%M:%SZ")


def iter_media_files(source_dir: Path):
    for path in sorted(source_dir.rglob("*")):
        if path.is_file() and path.suffix.lower() in _MEDIA_EXTENSIONS:
            yield path


def _ffprobe_tags(path: Path) -> dict | None:
    """Best-effort metadata via ffprobe. Returns None if ffprobe is unavailable
    or the file can't be probed; callers must degrade gracefully."""
    if shutil.which("ffprobe") is None:
        return None
    try:
        result = subprocess.run(
            [
                "ffprobe",
                "-v", "quiet",
                "-print_format", "json",
                "-show_format",
                "-show_streams",
                str(path),
            ],
            capture_output=True,
            text=True,
            check=True,
        )
        return json.loads(result.stdout)
    except (subprocess.CalledProcessError, json.JSONDecodeError):
        return None


def _extract_date_from_ffprobe(probe: dict) -> FuzzyDate | None:
    tags = (probe.get("format") or {}).get("tags") or {}
    for key in ("creation_time", "date", "DateTimeOriginal", "com.apple.quicktime.creationdate"):
        raw = tags.get(key)
        if not raw:
            continue
        for fmt in ("%Y-%m-%dT%H:%M:%S", "%Y:%m:%d %H:%M:%S", "%Y-%m-%d"):
            try:
                dt = datetime.datetime.strptime(raw[: len(fmt) + 2].split(".")[0], fmt)
                return fuzzy_date_from_frontmatter(dt.strftime("%Y-%m-%d"), "day")
            except ValueError:
                continue
    return None


def _dimensions_from_ffprobe(probe: dict) -> tuple[int | None, int | None, float | None]:
    width = height = None
    duration = None
    for stream in probe.get("streams") or []:
        if stream.get("codec_type") == "video" or stream.get("codec_type") == "image":
            width = stream.get("width", width)
            height = stream.get("height", height)
    fmt_duration = (probe.get("format") or {}).get("duration")
    if fmt_duration is not None:
        try:
            duration = float(fmt_duration)
        except ValueError:
            duration = None
    return width, height, duration


def _unique_dest(media_dir: Path, year_bucket: str, base_slug: str, suffix: str) -> Path:
    bucket_dir = media_dir / year_bucket
    n = 1
    while True:
        candidate = bucket_dir / f"{base_slug}-{n:03d}{suffix}"
        if not candidate.exists():
            return candidate
        n += 1


def ingest_file(
    source: Path,
    media_dir: Path,
    *,
    date_from_exif: bool = False,
    manual_date: str | None = None,
    visibility: str = "family",
) -> IngestedFile:
    if not source.is_file():
        raise MediaError(f"{source} is not a file")

    digest = sha256_file(source)
    size_bytes = source.stat().st_size
    mime_type, _ = mimetypes.guess_type(source.name)

    fuzzy: FuzzyDate | None = None
    date_source = "manual" if manual_date else "unknown"
    width = height = None
    duration = None

    probe = _ffprobe_tags(source)
    if probe is not None:
        width, height, duration = _dimensions_from_ffprobe(probe)
        if date_from_exif and manual_date is None:
            fuzzy = _extract_date_from_ffprobe(probe)
            if fuzzy is not None:
                date_source = "exif"

    if manual_date is not None:
        from legacy.core.dates import parse_fuzzy_date

        fuzzy = parse_fuzzy_date(manual_date)
    elif fuzzy is None:
        fuzzy = FuzzyDate(value=None, precision="unknown", year=None)
        date_source = "unknown"

    year_bucket = str(fuzzy.year) if fuzzy.year is not None else "undated"
    slug = slugify(source.stem) or "media"
    date_prefix = f"{fuzzy.value}-" if fuzzy.value else ""
    base_slug = f"{date_prefix}{slug}"
    dest = _unique_dest(media_dir, year_bucket, base_slug, source.suffix.lower())
    dest.parent.mkdir(parents=True, exist_ok=True)
    shutil.copy2(source, dest)

    sidecar = MediaSidecar(
        original_filename=source.name,
        sha256=digest,
        size_bytes=size_bytes,
        mime_type=mime_type,
        date=fuzzy.value,
        date_precision=fuzzy.precision,
        date_source=date_source,
        width=width,
        height=height,
        duration_seconds=duration,
        visibility=visibility,
    )
    sidecar_path = dest.with_name(dest.name + ".yaml")
    sidecar_path.write_text(sidecar.render(), encoding="utf-8")

    return IngestedFile(source=source, dest=dest, sidecar_path=sidecar_path, sidecar=sidecar)


def ingest_directory(
    source_dir: Path,
    media_dir: Path,
    *,
    date_from_exif: bool = False,
    manual_date: str | None = None,
    visibility: str = "family",
) -> list[IngestedFile]:
    return [
        ingest_file(
            path,
            media_dir,
            date_from_exif=date_from_exif,
            manual_date=manual_date,
            visibility=visibility,
        )
        for path in iter_media_files(source_dir)
    ]


def load_sidecar(path: Path) -> MediaSidecar:
    data = yaml.safe_load(path.read_text(encoding="utf-8")) or {}
    return MediaSidecar(
        original_filename=data["original_filename"],
        sha256=data["sha256"],
        size_bytes=data["size_bytes"],
        mime_type=data.get("mime_type"),
        date=data.get("date"),
        date_precision=data.get("date_precision", "unknown"),
        date_source=data.get("date_source", "unknown"),
        width=data.get("width"),
        height=data.get("height"),
        duration_seconds=data.get("duration_seconds"),
        visibility=data.get("visibility", "family"),
        caption=data.get("caption"),
        ingested_at=data.get("ingested_at", _now_iso()),
    )
