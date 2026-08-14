import datetime

import pytest

from legacy.core import index as index_mod
from legacy.core import llm as llm_mod
from legacy.core.story import new_story, save_story
from legacy.core.vault import Vault


class _FakeMessage:
    def __init__(self, content):
        self.content = content


class _FakeChoice:
    def __init__(self, content):
        self.message = _FakeMessage(content)


class _FakeResponse:
    def __init__(self, content):
        self.choices = [_FakeChoice(content)]


class _FakeCompletions:
    def __init__(self, reply: str):
        self.reply = reply
        self.last_prompt: str | None = None

    def create(self, model, messages, temperature=0):
        self.last_prompt = messages[0]["content"]
        return _FakeResponse(self.reply)


class _FakeChat:
    def __init__(self, reply: str):
        self.completions = _FakeCompletions(reply)


class _FakeClient:
    def __init__(self, reply: str):
        self.chat = _FakeChat(reply)


def _patch_client(monkeypatch, reply: str) -> _FakeClient:
    client = _FakeClient(reply)
    monkeypatch.setattr(llm_mod, "get_client", lambda settings: client)
    return client


def test_settings_from_env(monkeypatch):
    monkeypatch.setenv("LEGACY_LLM_API_KEY", "sk-test")
    monkeypatch.setenv("LEGACY_LLM_BASE_URL", "http://localhost:1234/v1")
    monkeypatch.setenv("LEGACY_LLM_MODEL", "local-model")
    settings = llm_mod.LLMSettings.from_env()
    assert settings.api_key == "sk-test"
    assert settings.base_url == "http://localhost:1234/v1"
    assert settings.model == "local-model"


def test_get_client_requires_api_key():
    with pytest.raises(llm_mod.LLMError):
        llm_mod.get_client(llm_mod.LLMSettings(api_key=None))


def test_suggest_tags_parses_and_dedups(monkeypatch):
    _patch_client(monkeypatch, "Education, Leaving-Home, education")
    story = new_story(title="Starting university", date="1994", tags=["education"])
    suggestion = llm_mod.suggest_tags(llm_mod.LLMSettings(api_key="x"), story)
    assert suggestion.suggested_tags == ["education", "leaving-home"]
    assert suggestion.new_tags == ["leaving-home"]


def test_apply_tag_suggestion_writes_frontmatter(tmp_path, monkeypatch):
    vault = Vault.init(tmp_path / "arch", subject_name="Jane Doe")
    story = new_story(title="Starting university", date="1994")
    save_story(vault.root, story)

    suggestion = llm_mod.TagSuggestion(
        story_id=story.id, current_tags=[], suggested_tags=["education"]
    )
    updated = llm_mod.apply_tag_suggestion(vault.root, story, suggestion, "gpt-4o-mini")
    assert updated.tags_generated_by == "gpt-4o-mini"

    from legacy.core.story import load_story

    reloaded = load_story(vault.root / story.relative_path())
    assert reloaded.tags == ["education"]
    assert reloaded.tags_generated_by == "gpt-4o-mini"


def test_suggest_tags_for_archive_skips_already_model_tagged(tmp_path, monkeypatch):
    vault = Vault.init(tmp_path / "arch", subject_name="Jane Doe")
    s1 = new_story(title="Untagged", date="1990")
    save_story(vault.root, s1)
    s2 = new_story(title="Already tagged", date="1991")
    s2.tags_generated_by = "gpt-4o-mini"
    save_story(vault.root, s2)

    client = _patch_client(monkeypatch, "some, tags")
    suggestions = llm_mod.suggest_tags_for_archive(llm_mod.LLMSettings(api_key="x"), vault.root)
    assert len(suggestions) == 1
    assert suggestions[0].story_id == s1.id
    assert client.chat.completions.last_prompt is not None


def test_check_sunset_passes_with_no_sunset():
    llm_mod.check_sunset(None)  # should not raise


def test_check_sunset_raises_when_past():
    yesterday = (datetime.date.today() - datetime.timedelta(days=1)).isoformat()
    with pytest.raises(llm_mod.LLMError):
        llm_mod.check_sunset(yesterday)


def test_check_sunset_ok_when_future():
    tomorrow = (datetime.date.today() + datetime.timedelta(days=1)).isoformat()
    llm_mod.check_sunset(tomorrow)  # should not raise


def test_ask_returns_refusal_when_no_hits(tmp_path, monkeypatch):
    vault = Vault.init(tmp_path / "arch", subject_name="Jane Doe")
    index_mod.rebuild_index(vault.root, vault.index_db_path)
    _patch_client(monkeypatch, "should not be called")

    result = llm_mod.ask(
        llm_mod.LLMSettings(api_key="x"), vault.root, vault.index_db_path, "anything?"
    )
    assert result.answer == llm_mod.REFUSAL
    assert result.cited_story_ids == []


def test_ask_parses_sources_line(tmp_path, monkeypatch):
    vault = Vault.init(tmp_path / "arch", subject_name="Jane Doe")
    story = new_story(title="Starting university", date="1994", body="I packed one suitcase.")
    save_story(vault.root, story)
    index_mod.rebuild_index(vault.root, vault.index_db_path)

    _patch_client(monkeypatch, f"I packed a suitcase.\nSources: {story.id}")
    result = llm_mod.ask(
        llm_mod.LLMSettings(api_key="x"), vault.root, vault.index_db_path, "suitcase"
    )
    assert "I packed a suitcase." in result.answer
    assert result.cited_story_ids == [story.id]


def test_ask_finds_hits_for_natural_language_question(tmp_path, monkeypatch):
    vault = Vault.init(tmp_path / "arch", subject_name="Jane Doe")
    story = new_story(title="Starting university", date="1994", body="I packed one suitcase.")
    save_story(vault.root, story)
    index_mod.rebuild_index(vault.root, vault.index_db_path)

    # A full sentence question would match nothing under strict AND
    # semantics; ask() must loosen it enough to actually retrieve the story.
    _patch_client(monkeypatch, f"You packed a suitcase.\nSources: {story.id}")
    result = llm_mod.ask(
        llm_mod.LLMSettings(api_key="x"),
        vault.root,
        vault.index_db_path,
        "What did you pack when you left for university?",
    )
    assert result.cited_story_ids == [story.id]


def test_ask_excludes_executor_only_stories(tmp_path, monkeypatch):
    vault = Vault.init(tmp_path / "arch", subject_name="Jane Doe")
    save_story(
        vault.root,
        new_story(title="Secret", body="executor eyes only content", visibility="executor-only"),
    )
    index_mod.rebuild_index(vault.root, vault.index_db_path)
    _patch_client(monkeypatch, "should not be called")

    result = llm_mod.ask(
        llm_mod.LLMSettings(api_key="x"), vault.root, vault.index_db_path, "executor"
    )
    assert result.answer == llm_mod.REFUSAL


def test_ask_refuses_after_sunset(tmp_path, monkeypatch):
    vault = Vault.init(tmp_path / "arch", subject_name="Jane Doe")
    index_mod.rebuild_index(vault.root, vault.index_db_path)
    yesterday = (datetime.date.today() - datetime.timedelta(days=1)).isoformat()

    with pytest.raises(llm_mod.LLMError):
        llm_mod.ask(
            llm_mod.LLMSettings(api_key="x"),
            vault.root,
            vault.index_db_path,
            "anything?",
            replica_sunset=yesterday,
        )


def test_retrieval_query_drops_stopwords_and_joins_with_or():
    query = llm_mod._retrieval_query("What did you do after school?")
    assert " OR " in query
    assert "what" not in query.lower().split(" or ")
    assert "school" in query.lower()


def test_suggest_followup(monkeypatch):
    _patch_client(monkeypatch, "What did you pack in your suitcase?")
    followup = llm_mod.suggest_followup(
        llm_mod.LLMSettings(api_key="x"), "What happened next?", "I moved out."
    )
    assert followup == "What did you pack in your suitcase?"
