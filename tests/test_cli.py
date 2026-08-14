from typer.testing import CliRunner

from legacy.cli import app

runner = CliRunner()


def test_init_story_add_list_timeline_verify(tmp_path):
    archive = tmp_path / "arch"

    result = runner.invoke(app, ["init", str(archive), "--subject", "Jane Doe"])
    assert result.exit_code == 0, result.output

    result = runner.invoke(
        app,
        [
            "story",
            "add",
            "--archive",
            str(archive),
            "--title",
            "Starting university",
            "--date",
            "1994-09-15",
            "--body",
            "I packed one suitcase.",
        ],
    )
    assert result.exit_code == 0, result.output

    result = runner.invoke(app, ["story", "list", "--archive", str(archive)])
    assert result.exit_code == 0, result.output
    assert "1994-09-15-starting-university" in result.output

    result = runner.invoke(app, ["timeline", "build", "--archive", str(archive)])
    assert result.exit_code == 0, result.output
    assert "Indexed 1 stories." in result.output

    result = runner.invoke(app, ["verify", "--archive", str(archive)])
    assert result.exit_code == 0, result.output
    assert "OK" in result.output


def test_init_refuses_existing_nonempty_dir(tmp_path):
    archive = tmp_path / "arch"
    archive.mkdir()
    (archive / "file.txt").write_text("hi")
    result = runner.invoke(app, ["init", str(archive), "--subject", "Jane Doe"])
    assert result.exit_code != 0
