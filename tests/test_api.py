import shutil

import pytest
from fastapi.testclient import TestClient

from legacy.api import app

client = TestClient(app)


def test_init_and_story_lifecycle(tmp_path):
    archive = str(tmp_path / "arch")

    r = client.post("/init", json={"path": archive, "subject": "Jane Doe"})
    assert r.status_code == 200, r.text
    assert r.json()["subject"] == "Jane Doe"

    r = client.post(
        "/stories",
        json={
            "archive": archive,
            "title": "Starting university",
            "date": "1994-09-15",
            "body": "I packed one suitcase.",
            "tags": ["education"],
        },
    )
    assert r.status_code == 200, r.text
    story = r.json()
    assert story["id"] == "1994-09-15-starting-university"
    assert story["date_precision"] == "day"

    r = client.get("/stories", params={"archive": archive})
    assert r.status_code == 200
    assert len(r.json()) == 1

    r = client.get("/stories", params={"archive": archive, "year": 1993})
    assert r.json() == []


def test_timeline_build_and_verify(tmp_path):
    archive = str(tmp_path / "arch")
    client.post("/init", json={"path": archive, "subject": "Jane Doe"})
    client.post("/stories", json={"archive": archive, "title": "A story", "date": "2000"})

    r = client.post("/timeline/build", json={"archive": archive})
    assert r.status_code == 200
    assert r.json()["story_count"] == 1

    r = client.get("/verify", params={"archive": archive})
    assert r.status_code == 200
    assert r.json()["ok"] is True


def test_verify_reports_problems(tmp_path):
    archive_path = tmp_path / "arch"
    archive = str(archive_path)
    client.post("/init", json={"path": archive, "subject": "Jane Doe"})
    client.post("/stories", json={"archive": archive, "title": "A story", "date": "2000"})
    client.post("/timeline/build", json={"archive": archive})

    story_file = next((archive_path / "timeline").glob("*/*.md"))
    story_file.write_text(story_file.read_text() + "\ntampered\n")

    r = client.get("/verify", params={"archive": archive})
    assert r.json()["ok"] is False
    assert len(r.json()["problems"]) >= 1


def test_interview_flow(tmp_path):
    archive = str(tmp_path / "arch")
    client.post("/init", json={"path": archive, "subject": "Jane Doe"})

    r = client.post("/interviews", json={"archive": archive, "bank": "childhood"})
    assert r.status_code == 200, r.text
    session = r.json()
    session_id = session["session_id"]
    assert session["next_question"] is not None
    first_question_id = session["next_question"]["id"]

    r = client.post(
        f"/interviews/{session_id}/answer",
        json={"archive": archive, "answer": "I remember the garden."},
    )
    assert r.status_code == 200, r.text
    session = r.json()
    assert first_question_id in session["answered"]

    r = client.post(f"/interviews/{session_id}/skip", json={"archive": archive})
    assert r.status_code == 200
    session = r.json()
    assert len(session["skipped"]) == 1

    r = client.get(f"/interviews/{session_id}", params={"archive": archive})
    assert r.status_code == 200
    assert r.json()["session_id"] == session_id


def test_interview_unknown_bank_returns_400(tmp_path):
    archive = str(tmp_path / "arch")
    client.post("/init", json={"path": archive, "subject": "Jane Doe"})
    r = client.post("/interviews", json={"archive": archive, "bank": "does-not-exist"})
    assert r.status_code == 400


@pytest.mark.skipif(
    shutil.which("age") is None or shutil.which("age-keygen") is None,
    reason="requires age and age-keygen on PATH",
)
def test_seal_and_unseal(tmp_path):
    archive = str(tmp_path / "arch")
    client.post("/init", json={"path": archive, "subject": "Jane Doe", "threshold": 2, "shares": 3})
    client.post(
        "/stories",
        json={"archive": archive, "title": "Family story", "date": "1994", "visibility": "family"},
    )

    r = client.post("/seal", json={"archive": archive, "tiers": ["family"]})
    assert r.status_code == 200, r.text
    tiers = r.json()
    assert len(tiers) == 1
    assert tiers[0]["tier"] == "family"
    assert len(tiers[0]["shares"]) == 3

    r = client.post(
        "/unseal", json={"archive": archive, "shares": tiers[0]["shares"][:2]}
    )
    assert r.status_code == 200, r.text
    result = r.json()
    assert result["tier"] == "family"
    assert result["file_count"] == 1


def test_verify_unknown_archive_returns_404():
    r = client.get("/verify", params={"archive": "/does/not/exist"})
    assert r.status_code == 404
