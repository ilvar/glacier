"""Archive open/create and top-level path resolution.

`archive.yaml` is the only piece of archive-wide configuration. Everything
else the program needs is either derived from it or rebuildable from the
plain files on disk (see index.py, timeline.py).
"""

from __future__ import annotations

import datetime
from dataclasses import dataclass, field
from pathlib import Path

import yaml

SCHEMA_VERSION = 1

ARCHIVE_DIRS = ["timeline/undated", "people", "places", "media", "interviews", "sealed"]

DEFAULT_TIERS = ("family", "executor-only")


class VaultError(Exception):
    pass


@dataclass
class TierConfig:
    threshold: int
    shares: int
    identity_sha256: str | None = None
    plaintext_escrow: bool = False

    def to_dict(self) -> dict:
        return {
            "threshold": self.threshold,
            "shares": self.shares,
            "identity_sha256": self.identity_sha256,
            "plaintext_escrow": self.plaintext_escrow,
        }

    @classmethod
    def from_dict(cls, d: dict) -> TierConfig:
        return cls(
            threshold=d["threshold"],
            shares=d["shares"],
            identity_sha256=d.get("identity_sha256"),
            plaintext_escrow=d.get("plaintext_escrow", False),
        )


@dataclass
class ArchiveConfig:
    subject_name: str
    schema_version: int = SCHEMA_VERSION
    created_at: str = field(default_factory=lambda: _now_iso())
    tiers: dict[str, TierConfig] = field(default_factory=dict)
    keyholders: list[dict] = field(default_factory=list)
    replica_sunset: str | None = None

    def to_dict(self) -> dict:
        return {
            "schema_version": self.schema_version,
            "subject": {"name": self.subject_name},
            "created_at": self.created_at,
            "tiers": {name: tier.to_dict() for name, tier in self.tiers.items()},
            "keyholders": self.keyholders,
            "replica": {"sunset": self.replica_sunset},
        }

    @classmethod
    def from_dict(cls, d: dict) -> ArchiveConfig:
        return cls(
            subject_name=d["subject"]["name"],
            schema_version=d.get("schema_version", SCHEMA_VERSION),
            created_at=d.get("created_at", _now_iso()),
            tiers={k: TierConfig.from_dict(v) for k, v in (d.get("tiers") or {}).items()},
            keyholders=d.get("keyholders") or [],
            replica_sunset=(d.get("replica") or {}).get("sunset"),
        )

    def save(self, path: Path) -> None:
        text = yaml.safe_dump(self.to_dict(), sort_keys=False, allow_unicode=True)
        path.write_text(text, encoding="utf-8")

    @classmethod
    def load(cls, path: Path) -> ArchiveConfig:
        data = yaml.safe_load(path.read_text(encoding="utf-8"))
        return cls.from_dict(data)


def _now_iso() -> str:
    return datetime.datetime.now(datetime.UTC).strftime("%Y-%m-%dT%H:%M:%SZ")


class Vault:
    """A single archive on disk."""

    def __init__(self, root: Path):
        self.root = Path(root)

    @property
    def config_path(self) -> Path:
        return self.root / "archive.yaml"

    @property
    def manifest_path(self) -> Path:
        return self.root / "MANIFEST.sha256"

    @property
    def readme_path(self) -> Path:
        return self.root / "README.txt"

    @property
    def timeline_dir(self) -> Path:
        return self.root / "timeline"

    @property
    def people_dir(self) -> Path:
        return self.root / "people"

    @property
    def places_dir(self) -> Path:
        return self.root / "places"

    @property
    def media_dir(self) -> Path:
        return self.root / "media"

    @property
    def interviews_dir(self) -> Path:
        return self.root / "interviews"

    @property
    def sealed_dir(self) -> Path:
        return self.root / "sealed"

    @property
    def index_db_path(self) -> Path:
        return self.root / ".legacy" / "index.sqlite3"

    def exists(self) -> bool:
        return self.config_path.exists()

    def load_config(self) -> ArchiveConfig:
        if not self.exists():
            raise VaultError(f"{self.root} is not a legacy archive (no archive.yaml)")
        return ArchiveConfig.load(self.config_path)

    def save_config(self, config: ArchiveConfig) -> None:
        config.save(self.config_path)

    @classmethod
    def init(
        cls,
        root: Path,
        subject_name: str,
        threshold: int = 3,
        shares: int = 5,
    ) -> Vault:
        root = Path(root)
        if root.exists() and any(root.iterdir()):
            raise VaultError(f"{root} already exists and is not empty")
        root.mkdir(parents=True, exist_ok=True)
        for rel in ARCHIVE_DIRS:
            (root / rel).mkdir(parents=True, exist_ok=True)

        config = ArchiveConfig(
            subject_name=subject_name,
            tiers={
                "family": TierConfig(threshold=threshold, shares=shares),
                "executor-only": TierConfig(threshold=threshold, shares=shares),
            },
        )
        vault = cls(root)
        vault.save_config(config)
        vault.manifest_path.touch()

        from legacy.core.readme import render_readme

        vault.readme_path.write_text(render_readme(config), encoding="utf-8")
        return vault

    @classmethod
    def open(cls, root: Path) -> Vault:
        vault = cls(root)
        if not vault.exists():
            raise VaultError(f"{root} is not a legacy archive (no archive.yaml found)")
        return vault
