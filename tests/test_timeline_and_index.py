from legacy.core import index as index_mod
from legacy.core import manifest as manifest_mod
from legacy.core import timeline as timeline_mod
from legacy.core.story import iter_stories, new_story, save_story
from legacy.core.vault import Vault


def _seed_archive(tmp_path):
    vault = Vault.init(tmp_path / "arch", subject_name="Jane Doe", threshold=2, shares=4)
    save_story(
        vault.root,
        new_story(title="Starting university", date="1994-09-15", body="I packed a suitcase."),
    )
    save_story(
        vault.root,
        new_story(title="Childhood home", date="1970s", body="We lived by the sea."),
    )
    save_story(vault.root, new_story(title="Undated memory", body="No idea when."))
    return vault


def test_build_timeline_groups_by_year_and_sorts(tmp_path):
    vault = _seed_archive(tmp_path)
    stories = list(iter_stories(vault.root))
    text = timeline_mod.render_timeline(stories)
    assert text.index("## 1970") < text.index("## 1994") < text.index("## Undated")
    assert "Starting university" in text
    assert "Childhood home" in text
    assert "Undated memory" in text


def test_rebuild_index_and_search(tmp_path):
    vault = _seed_archive(tmp_path)
    count = index_mod.rebuild_index(vault.root, vault.index_db_path)
    assert count == 3

    results = index_mod.search(vault.index_db_path, "suitcase")
    assert any(r["id"] == "1994-09-15-starting-university" for r in results)


def test_search_tolerates_punctuation_in_query(tmp_path):
    vault = _seed_archive(tmp_path)
    index_mod.rebuild_index(vault.root, vault.index_db_path)

    # A question mark (and other FTS5 syntax characters) must not raise --
    # the replica passes raw user questions straight through to search().
    results = index_mod.search(vault.index_db_path, "university?")
    assert any(r["id"] == "1994-09-15-starting-university" for r in results)


def test_search_respects_visibility_filter(tmp_path):
    vault = Vault.init(tmp_path / "arch", subject_name="Jane Doe")
    save_story(
        vault.root,
        new_story(title="Secret plan", body="executor eyes only", visibility="executor-only"),
    )
    save_story(
        vault.root,
        new_story(title="Family story", body="for everyone close", visibility="family"),
    )
    index_mod.rebuild_index(vault.root, vault.index_db_path)

    family_only = index_mod.search(vault.index_db_path, "plan OR story", visibilities=["family"])
    assert all(r["visibility"] == "family" for r in family_only)


def test_rebuild_derived_state_writes_manifest_that_verifies(tmp_path):
    vault = _seed_archive(tmp_path)
    result = timeline_mod.rebuild_derived_state(vault)
    assert result.story_count == 3
    assert result.timeline_path.exists()
    assert result.manifest_path.exists()

    problems = manifest_mod.verify_manifest(vault.root)
    assert problems == []


def test_verify_manifest_detects_tamper(tmp_path):
    vault = _seed_archive(tmp_path)
    timeline_mod.rebuild_derived_state(vault)

    story_file = next(vault.timeline_dir.glob("*/*.md"))
    story_file.write_text(story_file.read_text() + "\ntampered\n")

    problems = manifest_mod.verify_manifest(vault.root)
    assert any(reason == "checksum mismatch" for _, reason in problems)


def test_verify_manifest_detects_missing_file(tmp_path):
    vault = _seed_archive(tmp_path)
    timeline_mod.rebuild_derived_state(vault)

    story_file = next(vault.timeline_dir.glob("*/*.md"))
    story_file.unlink()

    problems = manifest_mod.verify_manifest(vault.root)
    assert any(reason == "listed in manifest but missing on disk" for _, reason in problems)
