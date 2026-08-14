"""Seal/unseal: builds one encrypted tar per visibility tier and splits its
key with SLIP-0039, or unwinds that process given enough shares.

Sealing never touches or deletes the plaintext archive -- it is purely
additive. `sealed/<tier>.tar.age` is a distributable snapshot for
keyholders; the author's own working copy stays exactly as it was.

Tier membership:
  - `visibility: public` stories are never sealed; they stay plain in
    timeline/ permanently, since they are meant to be shared with anyone.
  - `visibility: family` / `visibility: executor-only` stories go into the
    matching tier's bundle, along with any media/ files they reference.
  - people/, places/, and interviews/ (session transcripts + audio) are
    included in the "family" tier bundle only -- they are the shared
    reference material and raw interview sessions, which default to
    family-level sensitivity. They are not duplicated into executor-only.
"""

from __future__ import annotations

import shutil
import tarfile
import tempfile
from dataclasses import dataclass
from pathlib import Path

from legacy.core import crypto
from legacy.core.story import Story, iter_stories
from legacy.core.vault import ArchiveConfig, Vault

FAMILY_EXTRA_DIRS = ("people", "places", "interviews")


class SealError(ValueError):
    pass


@dataclass
class SealedTierResult:
    tier: str
    plaintext_escrow: bool
    story_count: int
    output_path: Path  # .tar.age, or the plaintext dir if escrowed
    par2_path: Path | None
    shares: list[str] | None  # None if plaintext_escrow
    identity_sha256: str | None


def _stories_for_tier(vault_root: Path, tier: str) -> list[Story]:
    return [s for s in iter_stories(vault_root) if s.visibility == tier]


def _stage_tier(vault_root: Path, tier: str, staging: Path) -> int:
    """Copy this tier's files into `staging`, preserving relative paths.
    Returns the number of stories staged."""
    stories = _stories_for_tier(vault_root, tier)
    media_paths: set[str] = set()

    for story in stories:
        rel = story.relative_path()
        src = vault_root / rel
        dst = staging / rel
        dst.parent.mkdir(parents=True, exist_ok=True)
        dst.write_bytes(src.read_bytes())
        for media_rel in story.media:
            media_paths.add(media_rel)

    for media_rel in media_paths:
        src = vault_root / media_rel
        if not src.exists():
            continue
        dst = staging / media_rel
        dst.parent.mkdir(parents=True, exist_ok=True)
        dst.write_bytes(src.read_bytes())
        sidecar_src = src.with_name(src.name + ".yaml")
        if sidecar_src.exists():
            sidecar_dst = dst.with_name(dst.name + ".yaml")
            sidecar_dst.write_bytes(sidecar_src.read_bytes())

    if tier == "family":
        for dirname in FAMILY_EXTRA_DIRS:
            src_dir = vault_root / dirname
            if not src_dir.exists():
                continue
            for src in src_dir.rglob("*"):
                if not src.is_file():
                    continue
                rel = src.relative_to(vault_root)
                dst = staging / rel
                dst.parent.mkdir(parents=True, exist_ok=True)
                dst.write_bytes(src.read_bytes())

    return len(stories)


def seal_tier(
    vault: Vault,
    config: ArchiveConfig,
    tier: str,
    *,
    plaintext_escrow: bool = False,
    par2_redundancy: int = 15,
) -> SealedTierResult:
    if tier not in config.tiers:
        raise SealError(f"no tier config for {tier!r} in archive.yaml")
    tier_config = config.tiers[tier]
    vault.sealed_dir.mkdir(parents=True, exist_ok=True)

    with tempfile.TemporaryDirectory() as tmp:
        staging = Path(tmp) / "staging"
        staging.mkdir()
        story_count = _stage_tier(vault.root, tier, staging)

        if plaintext_escrow:
            out_dir = vault.sealed_dir / tier
            if out_dir.exists():
                shutil.rmtree(out_dir)
            _copy_tree(staging, out_dir)
            tier_config.plaintext_escrow = True
            tier_config.identity_sha256 = None
            return SealedTierResult(
                tier=tier,
                plaintext_escrow=True,
                story_count=story_count,
                output_path=out_dir,
                par2_path=None,
                shares=None,
                identity_sha256=None,
            )

        tar_path = Path(tmp) / f"{tier}.tar"
        with tarfile.open(tar_path, "w") as tf:
            tf.add(staging, arcname=".")

        identity = crypto.generate_identity()
        out_path = vault.sealed_dir / f"{tier}.tar.age"
        crypto.age_encrypt(tar_path, identity.recipient, out_path)

        shares = crypto.split_identity(identity.identity, tier_config.threshold, tier_config.shares)

        par2_path = crypto.par2_create(out_path, redundancy_percent=par2_redundancy)

        tier_config.plaintext_escrow = False
        tier_config.identity_sha256 = identity.sha256

        return SealedTierResult(
            tier=tier,
            plaintext_escrow=False,
            story_count=story_count,
            output_path=out_path,
            par2_path=par2_path,
            shares=shares,
            identity_sha256=identity.sha256,
        )


def _copy_tree(src: Path, dst: Path) -> None:
    dst.mkdir(parents=True, exist_ok=True)
    for path in src.rglob("*"):
        rel = path.relative_to(src)
        target = dst / rel
        if path.is_dir():
            target.mkdir(parents=True, exist_ok=True)
        else:
            target.parent.mkdir(parents=True, exist_ok=True)
            target.write_bytes(path.read_bytes())


@dataclass
class UnsealResult:
    tier: str
    output_dir: Path
    file_count: int


def unseal(
    vault: Vault,
    config: ArchiveConfig,
    shares: list[str],
    output_base: Path,
) -> UnsealResult:
    """Reconstruct the identity from `shares`, figure out which tier it
    belongs to by comparing SHA-256 against archive.yaml, and decrypt."""
    try:
        identity = crypto.combine_shares(shares)
    except crypto.CryptoError as e:
        raise SealError(str(e)) from e

    digest = crypto.identity_sha256(identity)
    matching_tier = None
    for name, tier_config in config.tiers.items():
        if tier_config.identity_sha256 == digest:
            matching_tier = name
            break

    if matching_tier is None:
        raise SealError(
            "reconstructed a key, but its SHA-256 does not match any tier in archive.yaml. "
            "Check that you have enough correct shares for the same tier."
        )

    sealed_path = vault.sealed_dir / f"{matching_tier}.tar.age"
    if not sealed_path.exists():
        raise SealError(f"key matches tier {matching_tier!r} but {sealed_path} does not exist")

    output_dir = output_base / matching_tier
    with tempfile.TemporaryDirectory() as tmp:
        tar_path = Path(tmp) / "archive.tar"
        crypto.age_decrypt(sealed_path, identity, tar_path)
        output_dir.mkdir(parents=True, exist_ok=True)
        with tarfile.open(tar_path, "r") as tf:
            tf.extractall(output_dir, filter="data")

    count = sum(1 for p in output_dir.rglob("*") if p.is_file())
    return UnsealResult(tier=matching_tier, output_dir=output_dir, file_count=count)
