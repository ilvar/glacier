from pathlib import Path

import pytest

from legacy.core.story import StoryError, iter_stories, load_story, new_story, save_story


def test_new_story_id_from_date_and_title():
    s = new_story(title="Started university", date="1994-09-15")
    assert s.id == "1994-09-15-started-university"
    assert s.date == "1994-09-15"
    assert s.date_precision == "day"
    assert s.visibility == "family"
    assert s.tags_generated_by is None


def test_new_story_undated():
    s = new_story(title="Dad and the fishing trip")
    assert s.id == "dad-and-the-fishing-trip"
    assert s.date is None
    assert s.date_precision == "unknown"


def test_new_story_rejects_empty_title():
    with pytest.raises(StoryError):
        new_story(title="   ")


def test_relative_path_buckets_by_year():
    s = new_story(title="Anecdote", date="1978-03")
    assert s.relative_path() == Path("timeline/1978/1978-03-anecdote.md")

    undated = new_story(title="Undated thing")
    assert undated.relative_path() == Path("timeline/undated/undated-thing.md")


def test_round_trip_through_disk(tmp_path):
    s = new_story(
        title="Starting university",
        date="1994-09-15",
        body="I packed one suitcase.",
        people=["mary-oconnor"],
        places=["dublin"],
        tags=["education", "leaving-home"],
        visibility="family",
    )
    path = save_story(tmp_path, s)
    assert path.exists()

    loaded = load_story(path)
    assert loaded.id == s.id
    assert loaded.title == s.title
    assert loaded.date == s.date
    assert loaded.date_precision == s.date_precision
    assert loaded.people == ["mary-oconnor"]
    assert loaded.places == ["dublin"]
    assert loaded.tags == ["education", "leaving-home"]
    assert loaded.visibility == "family"
    assert "I packed one suitcase." in loaded.body
    assert "tags_generated_by: null" in path.read_text()


def test_save_story_refuses_overwrite(tmp_path):
    s = new_story(title="Once only")
    save_story(tmp_path, s)
    with pytest.raises(StoryError):
        save_story(tmp_path, s)
    # overwrite=True is fine
    save_story(tmp_path, s, overwrite=True)


def test_new_story_rejects_bad_visibility():
    s = new_story(title="Bad vis", visibility="top-secret")
    with pytest.raises(StoryError):
        s.render()


def test_iter_stories(tmp_path):
    save_story(tmp_path, new_story(title="One", date="1990"))
    save_story(tmp_path, new_story(title="Two", date="1995"))
    save_story(tmp_path, new_story(title="Three"))
    ids = sorted(s.id for s in iter_stories(tmp_path))
    assert ids == ["1990-one", "1995-two", "three"]


def test_parse_rejects_missing_frontmatter():
    from legacy.core.story import parse_story

    with pytest.raises(StoryError):
        parse_story("just some text, no frontmatter")
