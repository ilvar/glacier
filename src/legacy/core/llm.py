"""Optional LLM features: tag suggestion and the replica's retrieval-grounded
answers. Nothing here runs unless explicitly invoked (`legacy tag`, `legacy
ask`) and configured with an API key -- the archive is fully usable without
ever importing this module.

Accepts any OpenAI-compatible `base_url` so a local model server can be
substituted; this is a thin client, not a place to reimplement anything.
"""

from __future__ import annotations

import datetime
import os
import re
from dataclasses import dataclass, field
from pathlib import Path

from legacy.core import index as index_mod
from legacy.core.story import Story, iter_stories, save_story

DEFAULT_MODEL = "gpt-4o-mini"

# The replica must never surface executor-only material, regardless of
# whether the plaintext happens to be locally readable (it always is, on
# the author's own machine, since `seal` never deletes anything).
REPLICA_VISIBILITIES = ("public", "family")

REFUSAL = "they never talked about that with me"


class LLMError(RuntimeError):
    pass


@dataclass
class LLMSettings:
    api_key: str | None = None
    base_url: str | None = None
    model: str = DEFAULT_MODEL

    @classmethod
    def from_env(cls) -> LLMSettings:
        return cls(
            api_key=os.environ.get("LEGACY_LLM_API_KEY"),
            base_url=os.environ.get("LEGACY_LLM_BASE_URL"),
            model=os.environ.get("LEGACY_LLM_MODEL", DEFAULT_MODEL),
        )


def get_client(settings: LLMSettings):
    if not settings.api_key:
        raise LLMError(
            "No LLM configured. Set LEGACY_LLM_API_KEY (and optionally "
            "LEGACY_LLM_BASE_URL to point at a local/self-hosted OpenAI-compatible "
            "model) to use this feature. Everything else in this program works "
            "without it."
        )
    try:
        from openai import OpenAI
    except ImportError as e:
        raise LLMError(
            "The 'openai' package is required for LLM features. Install with: "
            "uv pip install -e '.[llm]'"
        ) from e
    return OpenAI(api_key=settings.api_key, base_url=settings.base_url)


def _complete(settings: LLMSettings, prompt: str) -> str:
    client = get_client(settings)
    response = client.chat.completions.create(
        model=settings.model,
        messages=[{"role": "user", "content": prompt}],
        temperature=0,
    )
    return (response.choices[0].message.content or "").strip()


# ------------------------------------------------------------------ tags --


@dataclass
class TagSuggestion:
    story_id: str
    current_tags: list[str]
    suggested_tags: list[str]

    @property
    def new_tags(self) -> list[str]:
        current = set(self.current_tags)
        return [t for t in self.suggested_tags if t not in current]


_TAG_PROMPT = """Suggest 3 to 6 short, lowercase, hyphenated topical tags for the \
following life story. Respond with ONLY a comma-separated list of tags, nothing else.

Title: {title}

{body}"""


def suggest_tags(settings: LLMSettings, story: Story) -> TagSuggestion:
    text = _complete(settings, _TAG_PROMPT.format(title=story.title, body=story.body))
    tags = sorted({t.strip().lower() for t in text.split(",") if t.strip()})
    return TagSuggestion(story_id=story.id, current_tags=list(story.tags), suggested_tags=tags)


def apply_tag_suggestion(
    vault_root: Path, story: Story, suggestion: TagSuggestion, model_id: str
) -> Story:
    story.tags = sorted(set(story.tags) | set(suggestion.suggested_tags))
    story.tags_generated_by = model_id
    save_story(vault_root, story, overwrite=True)
    return story


def suggest_tags_for_archive(
    settings: LLMSettings, vault_root: Path, only_untagged_by_model: bool = True
) -> list[TagSuggestion]:
    suggestions = []
    for story in iter_stories(vault_root):
        if only_untagged_by_model and story.tags_generated_by is not None:
            continue
        suggestions.append(suggest_tags(settings, story))
    return suggestions


_FOLLOWUP_PROMPT = """A person was just asked the following interview question and gave this \
answer. Suggest ONE short, natural follow-up question to ask next. Respond with only the \
question, nothing else.

Question: {question}

Answer: {answer}"""


def suggest_followup(settings: LLMSettings, question: str, answer: str) -> str:
    return _complete(settings, _FOLLOWUP_PROMPT.format(question=question, answer=answer))


# --------------------------------------------------------------- replica --


@dataclass
class AskResult:
    answer: str
    cited_story_ids: list[str] = field(default_factory=list)


def check_sunset(replica_sunset: str | None) -> None:
    if not replica_sunset:
        return
    sunset_date = datetime.date.fromisoformat(replica_sunset)
    if datetime.date.today() >= sunset_date:
        raise LLMError(
            f"This replica's sunset date ({replica_sunset}) has passed. "
            f"`ask` refuses to run past it, as configured in archive.yaml."
        )


_ASK_PROMPT = """You are answering questions in the voice of the person these stories \
are about, using ONLY the stories provided below -- never invent details. Quote their \
own phrasing where you can. If the stories do not answer the question, respond with \
exactly: {refusal}

End your answer with a final line: "Sources: <comma-separated story ids you used>". \
If you could not answer, write "Sources: none".

Stories:
{context}

Question: {question}"""


def _load_story_by_id(vault_root: Path, story_id: str) -> Story | None:
    for story in iter_stories(vault_root):
        if story.id == story_id:
            return story
    return None


_STOPWORDS = {
    "a", "an", "the", "is", "are", "was", "were", "be", "been", "being",
    "do", "does", "did", "have", "has", "had", "of", "to", "in", "on",
    "at", "for", "with", "about", "what", "when", "where", "who", "why",
    "how", "you", "your", "i", "me", "my", "it", "that", "this",
}


def _retrieval_query(question: str) -> str:
    """index.search() uses FTS5's default AND semantics, which is too
    strict for a natural-language question -- most words in "what did you
    do after school?" won't co-occur in any single story. Build a looser
    OR-of-keywords query instead."""
    words = re.findall(r"\w+", question.lower())
    keywords = [w for w in words if w not in _STOPWORDS] or words
    return " OR ".join(keywords) if keywords else question


def ask(
    settings: LLMSettings,
    vault_root: Path,
    index_db_path: Path,
    question: str,
    replica_sunset: str | None = None,
    limit: int = 6,
) -> AskResult:
    check_sunset(replica_sunset)

    hits = index_mod.search(
        index_db_path,
        _retrieval_query(question),
        visibilities=list(REPLICA_VISIBILITIES),
        limit=limit,
    )
    if not hits:
        return AskResult(answer=REFUSAL, cited_story_ids=[])

    stories = [_load_story_by_id(vault_root, h["id"]) for h in hits]
    stories = [s for s in stories if s is not None]
    context = "\n\n".join(f"[{s.id}] {s.title}\n{s.body}" for s in stories)

    text = _complete(
        settings, _ASK_PROMPT.format(refusal=REFUSAL, context=context, question=question)
    )

    cited: list[str] = []
    for line in text.splitlines():
        if line.strip().lower().startswith("sources:"):
            ids_part = line.split(":", 1)[1]
            cited = [i.strip() for i in ids_part.split(",") if i.strip() and i.strip() != "none"]
            text = text[: text.rfind(line)].strip()
            break

    if not cited and text.strip() != REFUSAL:
        cited = [s.id for s in stories]

    return AskResult(answer=text.strip(), cited_story_ids=cited)
