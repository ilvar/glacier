"""MANIFEST.sha256: a plain-text checksum list, one line per file.

Format matches `sha256sum` output exactly so `sha256sum -c MANIFEST.sha256`
works without this program.
"""

from __future__ import annotations

from pathlib import Path

from legacy.core.crypto import sha256_file

# Files/dirs never listed in the manifest (it lists itself, and volatile
# rebuildable state).
_EXCLUDE_DIRS = {".legacy", ".git"}
_EXCLUDE_NAMES = {"MANIFEST.sha256"}


def iter_manifest_files(archive_root: Path):
    for path in sorted(archive_root.rglob("*")):
        if not path.is_file():
            continue
        rel = path.relative_to(archive_root)
        if rel.name in _EXCLUDE_NAMES:
            continue
        if any(part in _EXCLUDE_DIRS for part in rel.parts):
            continue
        yield rel


def build_manifest(archive_root: Path) -> str:
    lines = []
    for rel in iter_manifest_files(archive_root):
        digest = sha256_file(archive_root / rel)
        lines.append(f"{digest}  {rel.as_posix()}\n")
    return "".join(lines)


def write_manifest(archive_root: Path) -> Path:
    manifest_path = archive_root / "MANIFEST.sha256"
    manifest_path.write_text(build_manifest(archive_root), encoding="utf-8")
    return manifest_path


def verify_manifest(archive_root: Path) -> list[tuple[Path, str]]:
    """Return a list of (relative_path, problem) for files that fail verification."""
    manifest_path = archive_root / "MANIFEST.sha256"
    problems: list[tuple[Path, str]] = []
    if not manifest_path.exists():
        return [(Path("MANIFEST.sha256"), "manifest file is missing")]

    expected: dict[str, str] = {}
    for line in manifest_path.read_text(encoding="utf-8").splitlines():
        if not line.strip():
            continue
        digest, _, rel = line.partition("  ")
        expected[rel] = digest

    seen = set()
    for rel in iter_manifest_files(archive_root):
        rel_str = rel.as_posix()
        seen.add(rel_str)
        if rel_str not in expected:
            problems.append((rel, "present on disk but not in manifest"))
            continue
        actual = sha256_file(archive_root / rel)
        if actual != expected[rel_str]:
            problems.append((rel, "checksum mismatch"))

    for rel_str in expected:
        if rel_str not in seen:
            problems.append((Path(rel_str), "listed in manifest but missing on disk"))

    return problems
