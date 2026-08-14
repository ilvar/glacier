import shutil

import pytest

from legacy.core import seal as seal_mod
from legacy.core.story import new_story, save_story
from legacy.core.vault import Vault

pytestmark = pytest.mark.skipif(
    shutil.which("age") is None or shutil.which("age-keygen") is None,
    reason="requires age and age-keygen on PATH",
)


def _seed(tmp_path):
    vault = Vault.init(tmp_path / "arch", subject_name="Jane Doe", threshold=2, shares=3)
    save_story(
        vault.root,
        new_story(
            title="Public fact", date="1990", body="Anyone can read this.", visibility="public"
        ),
    )
    save_story(
        vault.root,
        new_story(title="Family story", date="1994", body="For the family.", visibility="family"),
    )
    save_story(
        vault.root,
        new_story(
            title="Executor secret",
            date="2000",
            body="Only the executor should read this.",
            visibility="executor-only",
        ),
    )
    return vault


def test_seal_family_tier_produces_encrypted_blob_and_shares(tmp_path):
    vault = _seed(tmp_path)
    config = vault.load_config()

    result = seal_mod.seal_tier(vault, config, "family")

    assert not result.plaintext_escrow
    assert result.story_count == 1
    assert result.output_path.exists()
    assert result.output_path.name == "family.tar.age"
    assert len(result.shares) == 3
    assert result.identity_sha256 == config.tiers["family"].identity_sha256

    # Public stories never touched by seal.
    assert (vault.root / "timeline" / "1990").exists()


def test_seal_then_unseal_round_trip(tmp_path):
    vault = _seed(tmp_path)
    config = vault.load_config()

    result = seal_mod.seal_tier(vault, config, "executor-only")
    vault.save_config(config)

    reloaded_config = vault.load_config()
    unsealed = seal_mod.unseal(
        vault, reloaded_config, result.shares[:2], vault.root / "unsealed"
    )

    assert unsealed.tier == "executor-only"
    extracted_files = list(unsealed.output_dir.rglob("*.md"))
    assert any("executor-secret" in p.name for p in extracted_files)
    contents = "\n".join(p.read_text() for p in extracted_files)
    assert "Only the executor should read this." in contents


def test_unseal_rejects_wrong_shares(tmp_path):
    vault = _seed(tmp_path)
    config = vault.load_config()
    seal_mod.seal_tier(vault, config, "family")
    vault.save_config(config)

    reloaded_config = vault.load_config()
    bogus_shares = [
        "shadow pistol academic acid black glasses aunt scared oven vitamins spider "
        "smoking listen unusual party fatal"
    ]
    with pytest.raises(seal_mod.SealError):
        seal_mod.unseal(vault, reloaded_config, bogus_shares, vault.root / "unsealed")


def test_seal_plaintext_escrow_leaves_family_unencrypted(tmp_path):
    vault = _seed(tmp_path)
    config = vault.load_config()

    result = seal_mod.seal_tier(vault, config, "family", plaintext_escrow=True)

    assert result.plaintext_escrow
    assert result.shares is None
    assert config.tiers["family"].plaintext_escrow is True
    assert config.tiers["family"].identity_sha256 is None
    md_files = list(result.output_path.rglob("*.md"))
    assert any("For the family." in p.read_text() for p in md_files)


def test_seal_family_includes_people_places_interviews(tmp_path):
    vault = _seed(tmp_path)
    (vault.people_dir / "mary.md").write_text("---\nname: Mary\n---\n\nMary was kind.\n")
    (vault.interviews_dir / "session-01.md").write_text("---\nsession_id: x\n---\n\nTranscript.\n")
    config = vault.load_config()

    result = seal_mod.seal_tier(vault, config, "family", plaintext_escrow=True)

    assert (result.output_path / "people" / "mary.md").exists()
    assert (result.output_path / "interviews" / "session-01.md").exists()


def test_seal_updates_manifest_and_readme(tmp_path):
    from legacy.core import timeline as timeline_mod

    vault = _seed(tmp_path)
    config = vault.load_config()
    seal_mod.seal_tier(vault, config, "family")
    vault.save_config(config)

    result = timeline_mod.rebuild_derived_state(vault)
    readme_text = vault.readme_path.read_text()
    assert config.tiers["family"].identity_sha256 in readme_text

    from legacy.core.manifest import verify_manifest

    assert verify_manifest(vault.root) == []
    assert result.manifest_path.exists()
