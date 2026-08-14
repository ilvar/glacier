import pytest

from legacy.core import interview as interview_mod
from legacy.core.story import load_story
from legacy.core.vault import Vault


def test_list_banks_includes_required_set():
    banks = set(interview_mod.list_banks())
    required = {
        "childhood",
        "family-and-origins",
        "school",
        "work",
        "love-and-partnership",
        "parenthood",
        "beliefs-and-values",
        "turning-points",
        "advice-and-messages",
        "objects-and-places",
    }
    assert required.issubset(banks)


def test_load_bank_parses_questions():
    bank = interview_mod.load_bank("childhood")
    assert bank.name == "childhood"
    assert len(bank.questions) >= 3
    q = bank.question("earliest-memory")
    assert q is not None
    assert q.prompt
    assert isinstance(q.followups, list)


def test_load_bank_unknown_name_raises():
    with pytest.raises(interview_mod.InterviewError):
        interview_mod.load_bank("does-not-exist")


def test_new_session_and_resume_cycle(tmp_path):
    vault = Vault.init(tmp_path / "arch", subject_name="Jane Doe")
    bank = interview_mod.load_bank("childhood")
    session = interview_mod.new_session(vault.root, "childhood")
    assert session.bank == "childhood"
    assert session.status == "in_progress"
    interview_mod.save_session(vault.root, session)

    q1 = interview_mod.next_question(bank, session)
    assert q1 is not None

    story = interview_mod.record_answer(vault.root, session, bank, q1, "I remember the garden.")
    assert story.source == f"interview:{session.session_id}"
    assert story.body.strip() == "I remember the garden."
    assert q1.id in session.answered

    # Reload from disk to simulate a crash-and-resume.
    reloaded = interview_mod.load_session(vault.root, session.session_id)
    assert reloaded.answered == session.answered
    assert reloaded.stories[q1.id] == story.id

    q2 = interview_mod.next_question(bank, reloaded)
    assert q2 is not None
    assert q2.id != q1.id


def test_record_answer_writes_a_readable_story_file(tmp_path):
    vault = Vault.init(tmp_path / "arch", subject_name="Jane Doe")
    bank = interview_mod.load_bank("childhood")
    session = interview_mod.new_session(vault.root, "childhood")
    q1 = bank.questions[0]

    story = interview_mod.record_answer(vault.root, session, bank, q1, "Answer text here.")
    path = vault.root / story.relative_path()
    assert path.exists()
    loaded = load_story(path)
    assert loaded.body.strip() == "Answer text here."
    assert loaded.source == f"interview:{session.session_id}"
    assert loaded.tags == q1.tags


def test_record_answer_rejects_empty(tmp_path):
    vault = Vault.init(tmp_path / "arch", subject_name="Jane Doe")
    bank = interview_mod.load_bank("childhood")
    session = interview_mod.new_session(vault.root, "childhood")
    q1 = bank.questions[0]

    with pytest.raises(interview_mod.InterviewError):
        interview_mod.record_answer(vault.root, session, bank, q1, "   ")


def test_skip_question_does_not_create_story(tmp_path):
    vault = Vault.init(tmp_path / "arch", subject_name="Jane Doe")
    bank = interview_mod.load_bank("childhood")
    session = interview_mod.new_session(vault.root, "childhood")
    q1 = bank.questions[0]

    interview_mod.skip_question(vault.root, session, q1)
    assert q1.id in session.skipped
    assert q1.id not in session.stories

    reloaded = interview_mod.load_session(vault.root, session.session_id)
    assert q1.id in reloaded.skipped


def test_mark_complete_sets_status(tmp_path):
    vault = Vault.init(tmp_path / "arch", subject_name="Jane Doe")
    session = interview_mod.new_session(vault.root, "childhood")
    interview_mod.save_session(vault.root, session)
    interview_mod.mark_complete(vault.root, session)

    reloaded = interview_mod.load_session(vault.root, session.session_id)
    assert reloaded.status == "complete"


def test_new_session_id_avoids_collisions(tmp_path):
    vault = Vault.init(tmp_path / "arch", subject_name="Jane Doe")
    s1 = interview_mod.new_session(vault.root, "childhood")
    interview_mod.save_session(vault.root, s1)
    s2 = interview_mod.new_session(vault.root, "childhood")
    assert s1.session_id != s2.session_id
