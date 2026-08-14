"""Interview subsystem: question banks and resumable session state.

Every answer becomes a story file the moment it is given -- see
`record_answer` -- so a crash mid-session never loses anything already
answered. The question bank works fully offline; an LLM may only ever
*suggest* follow-ups, and that path is optional (see llm.py, step 7).
"""

from __future__ import annotations

import datetime
from dataclasses import dataclass, field
from pathlib import Path

import yaml

from legacy.core import story as story_mod

_BANK_DIR = Path(__file__).resolve().parent.parent / "templates" / "interviews"

_SESSION_FIELD_ORDER = [
    "session_id",
    "bank",
    "mode",
    "status",
    "started_at",
    "updated_at",
    "answered",
    "skipped",
    "stories",
]


class InterviewError(ValueError):
    pass


@dataclass
class Question:
    id: str
    prompt: str
    followups: list[str] = field(default_factory=list)
    tags: list[str] = field(default_factory=list)


@dataclass
class QuestionBank:
    name: str
    description: str
    questions: list[Question]

    def question(self, question_id: str) -> Question | None:
        for q in self.questions:
            if q.id == question_id:
                return q
        return None


def list_banks() -> list[str]:
    return sorted(p.stem for p in _BANK_DIR.glob("*.yaml"))


def load_bank(name: str) -> QuestionBank:
    path = _BANK_DIR / f"{name}.yaml"
    if not path.exists():
        available = ", ".join(list_banks())
        raise InterviewError(f"no question bank named {name!r}. Available: {available}")
    data = yaml.safe_load(path.read_text(encoding="utf-8"))
    questions = [
        Question(
            id=q["id"],
            prompt=q["prompt"],
            followups=q.get("followups") or [],
            tags=q.get("tags") or [],
        )
        for q in data.get("questions", [])
    ]
    return QuestionBank(
        name=data["name"], description=data.get("description", ""), questions=questions
    )


@dataclass
class InterviewSession:
    session_id: str
    bank: str
    mode: str = "text"
    status: str = "in_progress"
    started_at: str = field(default_factory=lambda: _now_iso())
    updated_at: str = field(default_factory=lambda: _now_iso())
    answered: list[str] = field(default_factory=list)
    skipped: list[str] = field(default_factory=list)
    stories: dict[str, str] = field(default_factory=dict)  # question_id -> story id
    transcript: str = ""

    def to_frontmatter(self) -> dict:
        data = {
            "session_id": self.session_id,
            "bank": self.bank,
            "mode": self.mode,
            "status": self.status,
            "started_at": self.started_at,
            "updated_at": self.updated_at,
            "answered": list(self.answered),
            "skipped": list(self.skipped),
            "stories": dict(self.stories),
        }
        return {k: data[k] for k in _SESSION_FIELD_ORDER}

    def render(self) -> str:
        fm = yaml.safe_dump(
            self.to_frontmatter(), sort_keys=False, allow_unicode=True, default_flow_style=False
        )
        body = self.transcript.strip("\n")
        return f"---\n{fm}---\n\n{body}\n"


def _now_iso() -> str:
    return datetime.datetime.now(datetime.UTC).strftime("%Y-%m-%dT%H:%M:%SZ")


def new_session_id(vault_root: Path, bank_name: str) -> str:
    date_prefix = datetime.date.today().isoformat()
    interviews_dir = vault_root / "interviews"
    n = 1
    while True:
        candidate = f"{date_prefix}-{bank_name}-session-{n:02d}"
        if not (interviews_dir / f"{candidate}.md").exists():
            return candidate
        n += 1


def session_path(vault_root: Path, session_id: str) -> Path:
    return vault_root / "interviews" / f"{session_id}.md"


def new_session(vault_root: Path, bank_name: str, mode: str = "text") -> InterviewSession:
    load_bank(bank_name)  # validates the bank exists
    session_id = new_session_id(vault_root, bank_name)
    return InterviewSession(session_id=session_id, bank=bank_name, mode=mode)


def save_session(vault_root: Path, session: InterviewSession) -> Path:
    session.updated_at = _now_iso()
    path = session_path(vault_root, session.session_id)
    path.parent.mkdir(parents=True, exist_ok=True)
    path.write_text(session.render(), encoding="utf-8")
    return path


def load_session(vault_root: Path, session_id: str) -> InterviewSession:
    path = session_path(vault_root, session_id)
    if not path.exists():
        raise InterviewError(f"no interview session {session_id!r} at {path}")
    text = path.read_text(encoding="utf-8")
    if not text.startswith("---\n"):
        raise InterviewError(f"{path} is missing its frontmatter block")
    _, fm_text, body = text.split("---\n", 2)
    fm = yaml.safe_load(fm_text) or {}
    return InterviewSession(
        session_id=fm["session_id"],
        bank=fm["bank"],
        mode=fm.get("mode", "text"),
        status=fm.get("status", "in_progress"),
        started_at=fm.get("started_at", _now_iso()),
        updated_at=fm.get("updated_at", _now_iso()),
        answered=fm.get("answered") or [],
        skipped=fm.get("skipped") or [],
        stories=fm.get("stories") or {},
        transcript=body,
    )


def next_question(bank: QuestionBank, session: InterviewSession) -> Question | None:
    done = set(session.answered) | set(session.skipped)
    for q in bank.questions:
        if q.id not in done:
            return q
    return None


def record_answer(
    vault_root: Path,
    session: InterviewSession,
    bank: QuestionBank,
    question: Question,
    answer_text: str,
    *,
    visibility: str = "family",
    source_override: str | None = None,
) -> story_mod.Story:
    """Save the answer as a story file first (so it can never be lost), then
    update and save the session state to mark the question answered."""
    answer_text = answer_text.strip()
    if not answer_text:
        raise InterviewError("cannot record an empty answer")

    session_date = session.session_id[:10]  # YYYY-MM-DD prefix of the session id
    title = question.prompt if len(question.prompt) <= 60 else question.id.replace("-", " ")
    story = story_mod.new_story(
        title=title,
        date=session_date,
        body=answer_text,
        tags=list(question.tags),
        source=source_override or f"interview:{session.session_id}",
        recorded_at=_now_iso(),
        visibility=visibility,
        story_id=f"{session.session_id}-{question.id}",
    )
    story_mod.save_story(vault_root, story, overwrite=True)

    if question.id not in session.answered:
        session.answered.append(question.id)
    session.skipped = [q for q in session.skipped if q != question.id]
    session.stories[question.id] = story.id
    session.transcript += f"\n## {question.prompt}\n\n{answer_text}\n"
    save_session(vault_root, session)

    return story


def skip_question(vault_root: Path, session: InterviewSession, question: Question) -> None:
    if question.id not in session.skipped and question.id not in session.answered:
        session.skipped.append(question.id)
    session.transcript += f"\n## {question.prompt}\n\n(skipped)\n"
    save_session(vault_root, session)


def mark_complete(vault_root: Path, session: InterviewSession) -> None:
    session.status = "complete"
    save_session(vault_root, session)
