import pytest

from legacy.core.vault import ArchiveConfig, Vault, VaultError


def test_init_creates_layout(tmp_path):
    root = tmp_path / "my-archive"
    vault = Vault.init(root, subject_name="Jane Doe", threshold=3, shares=5)

    assert vault.config_path.exists()
    assert vault.readme_path.exists()
    assert vault.manifest_path.exists()
    assert vault.timeline_dir.is_dir()
    assert vault.people_dir.is_dir()
    assert vault.places_dir.is_dir()
    assert vault.media_dir.is_dir()
    assert vault.interviews_dir.is_dir()

    config = vault.load_config()
    assert config.subject_name == "Jane Doe"
    assert config.tiers["family"].threshold == 3
    assert config.tiers["family"].shares == 5
    assert config.tiers["executor-only"].threshold == 3

    readme_text = vault.readme_path.read_text()
    assert "Jane Doe" in readme_text
    assert "age -d -i" in readme_text


def test_init_refuses_nonempty_dir(tmp_path):
    root = tmp_path / "existing"
    root.mkdir()
    (root / "somefile").write_text("hi")
    with pytest.raises(VaultError):
        Vault.init(root, subject_name="Someone")


def test_open_requires_existing_archive(tmp_path):
    with pytest.raises(VaultError):
        Vault.open(tmp_path / "nope")


def test_config_round_trip(tmp_path):
    root = tmp_path / "arch"
    vault = Vault.init(root, subject_name="Jane Doe", threshold=2, shares=4)
    config = vault.load_config()
    config.tiers["family"].identity_sha256 = "a" * 64
    vault.save_config(config)

    reloaded = ArchiveConfig.load(vault.config_path)
    assert reloaded.tiers["family"].identity_sha256 == "a" * 64
