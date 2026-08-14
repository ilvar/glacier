import pytest

from legacy.core import voice as voice_mod


def test_settings_from_env_prefers_legacy_key(monkeypatch):
    monkeypatch.setenv("LEGACY_OPENAI_API_KEY", "sk-legacy")
    monkeypatch.setenv("OPENAI_API_KEY", "sk-generic")
    settings = voice_mod.VoiceSettings.from_env()
    assert settings.api_key == "sk-legacy"


def test_settings_from_env_falls_back_to_openai_key(monkeypatch):
    monkeypatch.delenv("LEGACY_OPENAI_API_KEY", raising=False)
    monkeypatch.setenv("OPENAI_API_KEY", "sk-generic")
    settings = voice_mod.VoiceSettings.from_env()
    assert settings.api_key == "sk-generic"


def test_get_client_requires_api_key():
    with pytest.raises(voice_mod.VoiceError):
        voice_mod.get_client(voice_mod.VoiceSettings(api_key=None))


def test_record_to_wav_requires_ffmpeg(tmp_path, monkeypatch):
    monkeypatch.setattr(voice_mod.shutil, "which", lambda name: None)
    with pytest.raises(voice_mod.VoiceError):
        voice_mod.record_to_wav(tmp_path / "out.wav")


def test_transcribe_uses_client(tmp_path, monkeypatch):
    wav = tmp_path / "answer.wav"
    wav.write_bytes(b"not-really-audio")

    class _FakeTranscript:
        text = "I remember the garden."

    class _FakeTranscriptions:
        def create(self, model, file):
            return _FakeTranscript()

    class _FakeAudio:
        transcriptions = _FakeTranscriptions()

    class _FakeClient:
        audio = _FakeAudio()

    monkeypatch.setattr(voice_mod, "get_client", lambda settings: _FakeClient())
    text = voice_mod.transcribe(voice_mod.VoiceSettings(api_key="x"), wav)
    assert text == "I remember the garden."


def test_synthesize_writes_via_streaming_response(tmp_path, monkeypatch):
    out_path = tmp_path / "out.mp3"

    class _FakeStreamingResponse:
        def __enter__(self):
            return self

        def __exit__(self, *exc):
            return False

        def stream_to_file(self, path):
            path.write_bytes(b"fake-audio-bytes")

    class _FakeStreamingSpeech:
        def create(self, model, voice, input):
            return _FakeStreamingResponse()

    class _FakeSpeech:
        with_streaming_response = _FakeStreamingSpeech()

    class _FakeAudio:
        speech = _FakeSpeech()

    class _FakeClient:
        audio = _FakeAudio()

    monkeypatch.setattr(voice_mod, "get_client", lambda settings: _FakeClient())
    result = voice_mod.synthesize(voice_mod.VoiceSettings(api_key="x"), "hello", out_path)
    assert result == out_path
    assert out_path.read_bytes() == b"fake-audio-bytes"
