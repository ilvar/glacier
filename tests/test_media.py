from pathlib import Path

from legacy.core import media as media_mod
from legacy.core.vault import Vault


def _make_source(tmp_path: Path, name: str, content: bytes = b"fake-jpeg-bytes") -> Path:
    src_dir = tmp_path / "incoming"
    src_dir.mkdir(exist_ok=True)
    path = src_dir / name
    path.write_bytes(content)
    return path


def test_ingest_file_copies_and_writes_sidecar(tmp_path):
    vault = Vault.init(tmp_path / "arch", subject_name="Jane Doe")
    src = _make_source(tmp_path, "IMG_0001.JPG")

    result = media_mod.ingest_file(src, vault.media_dir, manual_date="1994-09-15")

    assert result.dest.exists()
    assert result.dest.read_bytes() == src.read_bytes()
    assert result.dest.parent.name == "1994"
    assert result.dest.name.startswith("1994-09-15-img-0001-001")

    assert result.sidecar_path.exists()
    loaded = media_mod.load_sidecar(result.sidecar_path)
    assert loaded.original_filename == "IMG_0001.JPG"
    assert loaded.sha256 == result.sidecar.sha256
    assert loaded.date == "1994-09-15"
    assert loaded.date_precision == "day"
    assert loaded.date_source == "manual"
    assert loaded.visibility == "family"


def test_ingest_file_without_date_goes_to_undated(tmp_path):
    vault = Vault.init(tmp_path / "arch", subject_name="Jane Doe")
    src = _make_source(tmp_path, "scan.png")

    result = media_mod.ingest_file(src, vault.media_dir)

    assert result.dest.parent.name == "undated"
    assert result.sidecar.date is None
    assert result.sidecar.date_precision == "unknown"
    assert result.sidecar.date_source == "unknown"


def test_ingest_file_rejects_bad_visibility(tmp_path):
    vault = Vault.init(tmp_path / "arch", subject_name="Jane Doe")
    src = _make_source(tmp_path, "scan.png")

    import pytest

    with pytest.raises(media_mod.MediaError):
        media_mod.ingest_file(src, vault.media_dir, visibility="top-secret")


def test_ingest_directory_handles_multiple_files_and_collisions(tmp_path):
    vault = Vault.init(tmp_path / "arch", subject_name="Jane Doe")
    src_dir = tmp_path / "incoming"
    src_dir.mkdir()
    (src_dir / "a.jpg").write_bytes(b"one")
    (src_dir / "b.jpg").write_bytes(b"two")
    (src_dir / "notes.txt").write_bytes(b"ignored, not a media extension")

    results = media_mod.ingest_directory(src_dir, vault.media_dir, manual_date="2000")

    assert len(results) == 2
    dest_names = {r.dest.name for r in results}
    assert len(dest_names) == 2  # no filename collisions


def test_ingest_preserves_bytes_exactly(tmp_path):
    vault = Vault.init(tmp_path / "arch", subject_name="Jane Doe")
    content = bytes(range(256)) * 4
    src = _make_source(tmp_path, "raw.wav", content)

    result = media_mod.ingest_file(src, vault.media_dir, manual_date="2005-01")
    assert result.dest.read_bytes() == content
