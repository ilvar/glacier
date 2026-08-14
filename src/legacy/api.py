"""legacy REST API: mirrors cli.py 1:1, JSON in and out.

Binds to 127.0.0.1 by default (see `legacy serve`). There is no
authentication in v1 -- this is meant for a single local user driving the
same machine a GUI would, not a multi-tenant service. If you expose this
beyond localhost, put a reverse proxy with auth in front of it yourself.

No server-side session state beyond the interview session id, which is
just the filename of an already-persisted session file -- restarting this
process loses nothing.
"""

from __future__ import annotations

from pathlib import Path

from fastapi import FastAPI, HTTPException
from pydantic import BaseModel

from legacy.core import interview as interview_mod
from legacy.core import llm as llm_mod
from legacy.core import manifest as manifest_mod
from legacy.core import media as media_mod
from legacy.core import seal as seal_mod
from legacy.core import story as story_mod
from legacy.core import timeline as timeline_mod
from legacy.core.vault import Vault, VaultError

app = FastAPI(title="legacy", description="Local-first life-story vault API (localhost only)")


def _open(archive: str) -> Vault:
    try:
        return Vault.open(Path(archive))
    except VaultError as e:
        raise HTTPException(status_code=404, detail=str(e)) from e


# ---------------------------------------------------------------- init ----


class InitRequest(BaseModel):
    path: str
    subject: str
    threshold: int = 3
    shares: int = 5


class ArchiveInfo(BaseModel):
    root: str
    subject: str
    schema_version: int


@app.post("/init", response_model=ArchiveInfo)
def init_archive(req: InitRequest) -> ArchiveInfo:
    try:
        vault = Vault.init(
            Path(req.path), subject_name=req.subject, threshold=req.threshold, shares=req.shares
        )
    except VaultError as e:
        raise HTTPException(status_code=400, detail=str(e)) from e
    config = vault.load_config()
    return ArchiveInfo(
        root=str(vault.root),
        subject=config.subject_name,
        schema_version=config.schema_version,
    )


# -------------------------------------------------------------- stories ---


class StoryCreateRequest(BaseModel):
    archive: str
    title: str
    date: str | None = None
    body: str = ""
    people: list[str] = []
    places: list[str] = []
    tags: list[str] = []
    visibility: str = "family"


class StoryInfo(BaseModel):
    id: str
    title: str
    date: str | None
    date_precision: str
    people: list[str]
    places: list[str]
    tags: list[str]
    media: list[str]
    source: str | None
    visibility: str
    body: str
    path: str

    @classmethod
    def from_story(cls, story: story_mod.Story, root: Path) -> StoryInfo:
        return cls(
            id=story.id,
            title=story.title,
            date=story.date,
            date_precision=story.date_precision,
            people=story.people,
            places=story.places,
            tags=story.tags,
            media=story.media,
            source=story.source,
            visibility=story.visibility,
            body=story.body,
            path=str(story.relative_path()),
        )


@app.post("/stories", response_model=StoryInfo)
def create_story(req: StoryCreateRequest) -> StoryInfo:
    vault = _open(req.archive)
    try:
        story = story_mod.new_story(
            title=req.title,
            date=req.date,
            body=req.body,
            people=req.people,
            places=req.places,
            tags=req.tags,
            visibility=req.visibility,
        )
        story_mod.save_story(vault.root, story)
    except story_mod.StoryError as e:
        raise HTTPException(status_code=400, detail=str(e)) from e
    return StoryInfo.from_story(story, vault.root)


@app.get("/stories", response_model=list[StoryInfo])
def list_stories(
    archive: str, year: int | None = None, person: str | None = None, tag: str | None = None
) -> list[StoryInfo]:
    vault = _open(archive)
    results = []
    for story in story_mod.iter_stories(vault.root):
        if year is not None and story.fuzzy_date.year != year:
            continue
        if person is not None and person not in story.people:
            continue
        if tag is not None and tag not in story.tags:
            continue
        results.append(StoryInfo.from_story(story, vault.root))
    return results


# -------------------------------------------------------------- timeline --


class ArchiveRequest(BaseModel):
    archive: str


class RebuildResultInfo(BaseModel):
    story_count: int
    timeline_path: str
    index_path: str
    manifest_path: str
    readme_path: str


@app.post("/timeline/build", response_model=RebuildResultInfo)
def timeline_build(req: ArchiveRequest) -> RebuildResultInfo:
    vault = _open(req.archive)
    result = timeline_mod.rebuild_derived_state(vault)
    return RebuildResultInfo(
        story_count=result.story_count,
        timeline_path=str(result.timeline_path),
        index_path=str(result.index_path),
        manifest_path=str(result.manifest_path),
        readme_path=str(result.readme_path),
    )


class VerifyProblem(BaseModel):
    path: str
    reason: str


class VerifyResult(BaseModel):
    ok: bool
    problems: list[VerifyProblem]


@app.get("/verify", response_model=VerifyResult)
def verify_archive(archive: str) -> VerifyResult:
    vault = _open(archive)
    problems = manifest_mod.verify_manifest(vault.root)
    return VerifyResult(
        ok=not problems,
        problems=[VerifyProblem(path=str(p), reason=r) for p, r in problems],
    )


# ----------------------------------------------------------------- media --


class MediaIngestRequest(BaseModel):
    archive: str
    source: str
    date_from_exif: bool = False
    date: str | None = None
    visibility: str = "family"


class MediaInfo(BaseModel):
    source: str
    dest: str
    sidecar_path: str
    sha256: str
    date: str | None
    visibility: str


@app.post("/media/ingest", response_model=list[MediaInfo])
def media_ingest(req: MediaIngestRequest) -> list[MediaInfo]:
    vault = _open(req.archive)
    try:
        results = media_mod.ingest_directory(
            Path(req.source),
            vault.media_dir,
            date_from_exif=req.date_from_exif,
            manual_date=req.date,
            visibility=req.visibility,
        )
    except media_mod.MediaError as e:
        raise HTTPException(status_code=400, detail=str(e)) from e
    return [
        MediaInfo(
            source=str(r.source),
            dest=str(r.dest),
            sidecar_path=str(r.sidecar_path),
            sha256=r.sidecar.sha256,
            date=r.sidecar.date,
            visibility=r.sidecar.visibility,
        )
        for r in results
    ]


# ------------------------------------------------------------- interview --


class InterviewStartRequest(BaseModel):
    archive: str
    bank: str
    mode: str = "text"


class QuestionInfo(BaseModel):
    id: str
    prompt: str
    followups: list[str]
    tags: list[str]


class SessionInfo(BaseModel):
    session_id: str
    bank: str
    mode: str
    status: str
    answered: list[str]
    skipped: list[str]
    stories: dict[str, str]
    next_question: QuestionInfo | None


def _session_info(
    session: interview_mod.InterviewSession, bank: interview_mod.QuestionBank
) -> SessionInfo:
    q = interview_mod.next_question(bank, session)
    return SessionInfo(
        session_id=session.session_id,
        bank=session.bank,
        mode=session.mode,
        status=session.status,
        answered=session.answered,
        skipped=session.skipped,
        stories=session.stories,
        next_question=(
            QuestionInfo(id=q.id, prompt=q.prompt, followups=q.followups, tags=q.tags)
            if q is not None
            else None
        ),
    )


@app.post("/interviews", response_model=SessionInfo)
def interview_start(req: InterviewStartRequest) -> SessionInfo:
    vault = _open(req.archive)
    if req.mode == "voice":
        raise HTTPException(status_code=400, detail="voice mode is not implemented yet")
    try:
        bank = interview_mod.load_bank(req.bank)
        session = interview_mod.new_session(vault.root, req.bank, mode=req.mode)
    except interview_mod.InterviewError as e:
        raise HTTPException(status_code=400, detail=str(e)) from e
    interview_mod.save_session(vault.root, session)
    return _session_info(session, bank)


@app.get("/interviews/{session_id}", response_model=SessionInfo)
def interview_get(session_id: str, archive: str) -> SessionInfo:
    vault = _open(archive)
    try:
        session = interview_mod.load_session(vault.root, session_id)
        bank = interview_mod.load_bank(session.bank)
    except interview_mod.InterviewError as e:
        raise HTTPException(status_code=404, detail=str(e)) from e
    return _session_info(session, bank)


class InterviewAnswerRequest(BaseModel):
    archive: str
    answer: str
    visibility: str = "family"


@app.post("/interviews/{session_id}/answer", response_model=SessionInfo)
def interview_answer(session_id: str, req: InterviewAnswerRequest) -> SessionInfo:
    vault = _open(req.archive)
    try:
        session = interview_mod.load_session(vault.root, session_id)
        bank = interview_mod.load_bank(session.bank)
        question = interview_mod.next_question(bank, session)
        if question is None:
            raise HTTPException(status_code=400, detail="session already complete")
        interview_mod.record_answer(
            vault.root, session, bank, question, req.answer, visibility=req.visibility
        )
    except interview_mod.InterviewError as e:
        raise HTTPException(status_code=400, detail=str(e)) from e
    if interview_mod.next_question(bank, session) is None:
        interview_mod.mark_complete(vault.root, session)
    return _session_info(session, bank)


@app.post("/interviews/{session_id}/skip", response_model=SessionInfo)
def interview_skip(session_id: str, req: ArchiveRequest) -> SessionInfo:
    vault = _open(req.archive)
    try:
        session = interview_mod.load_session(vault.root, session_id)
        bank = interview_mod.load_bank(session.bank)
        question = interview_mod.next_question(bank, session)
        if question is None:
            raise HTTPException(status_code=400, detail="session already complete")
        interview_mod.skip_question(vault.root, session, question)
    except interview_mod.InterviewError as e:
        raise HTTPException(status_code=400, detail=str(e)) from e
    return _session_info(session, bank)


# -------------------------------------------------------------------- llm --


class TagRequest(BaseModel):
    archive: str
    apply: bool = False


class TagSuggestionInfo(BaseModel):
    story_id: str
    current_tags: list[str]
    suggested_tags: list[str]
    new_tags: list[str]
    applied: bool


@app.post("/tag", response_model=list[TagSuggestionInfo])
def tag_stories(req: TagRequest) -> list[TagSuggestionInfo]:
    vault = _open(req.archive)
    settings = llm_mod.LLMSettings.from_env()
    try:
        suggestions = llm_mod.suggest_tags_for_archive(settings, vault.root)
    except llm_mod.LLMError as e:
        raise HTTPException(status_code=400, detail=str(e)) from e

    results = []
    for suggestion in suggestions:
        applied = False
        if req.apply and suggestion.new_tags:
            story = next(
                s for s in story_mod.iter_stories(vault.root) if s.id == suggestion.story_id
            )
            llm_mod.apply_tag_suggestion(vault.root, story, suggestion, settings.model)
            applied = True
        results.append(
            TagSuggestionInfo(
                story_id=suggestion.story_id,
                current_tags=suggestion.current_tags,
                suggested_tags=suggestion.suggested_tags,
                new_tags=suggestion.new_tags,
                applied=applied,
            )
        )
    return results


class AskRequest(BaseModel):
    archive: str
    question: str


class AskResponse(BaseModel):
    answer: str
    cited_story_ids: list[str]


@app.post("/ask", response_model=AskResponse)
def ask_replica(req: AskRequest) -> AskResponse:
    vault = _open(req.archive)
    config = vault.load_config()
    settings = llm_mod.LLMSettings.from_env()
    try:
        result = llm_mod.ask(
            settings,
            vault.root,
            vault.index_db_path,
            req.question,
            replica_sunset=config.replica_sunset,
        )
    except llm_mod.LLMError as e:
        raise HTTPException(status_code=400, detail=str(e)) from e
    return AskResponse(answer=result.answer, cited_story_ids=result.cited_story_ids)


# ------------------------------------------------------------------ seal --


class SealRequest(BaseModel):
    archive: str
    tiers: list[str] = ["family", "executor-only"]
    plaintext_escrow: bool = False


class SealedTierInfo(BaseModel):
    tier: str
    plaintext_escrow: bool
    story_count: int
    output_path: str
    par2_created: bool
    identity_sha256: str | None
    shares: list[str] | None


@app.post("/seal", response_model=list[SealedTierInfo])
def seal_archive(req: SealRequest) -> list[SealedTierInfo]:
    """WARNING: shares appear in this response body and are never stored
    anywhere else. This endpoint is only safe on localhost, as documented
    for the whole API."""
    vault = _open(req.archive)
    config = vault.load_config()
    results = []
    for tier in req.tiers:
        escrow = req.plaintext_escrow and tier == "family"
        try:
            result = seal_mod.seal_tier(vault, config, tier, plaintext_escrow=escrow)
        except seal_mod.SealError as e:
            raise HTTPException(status_code=400, detail=str(e)) from e
        results.append(
            SealedTierInfo(
                tier=result.tier,
                plaintext_escrow=result.plaintext_escrow,
                story_count=result.story_count,
                output_path=str(result.output_path),
                par2_created=result.par2_path is not None,
                identity_sha256=result.identity_sha256,
                shares=result.shares,
            )
        )
    vault.save_config(config)
    timeline_mod.rebuild_derived_state(vault)
    return results


class UnsealRequest(BaseModel):
    archive: str
    shares: list[str]
    output: str | None = None


class UnsealResultInfo(BaseModel):
    tier: str
    output_dir: str
    file_count: int


@app.post("/unseal", response_model=UnsealResultInfo)
def unseal_archive(req: UnsealRequest) -> UnsealResultInfo:
    vault = _open(req.archive)
    config = vault.load_config()
    output_base = Path(req.output) if req.output else vault.root / "unsealed"
    try:
        result = seal_mod.unseal(vault, config, req.shares, output_base)
    except seal_mod.SealError as e:
        raise HTTPException(status_code=400, detail=str(e)) from e
    return UnsealResultInfo(
        tier=result.tier, output_dir=str(result.output_dir), file_count=result.file_count
    )
