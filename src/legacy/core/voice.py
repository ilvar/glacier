"""Speech features: push-to-talk recording via `ffmpeg`, transcription via
the OpenAI Whisper API, and playback synthesis via the OpenAI TTS API.

Both are opt-in and require network access and an API key; nothing else in
this program needs either. The interview subsystem works fully in text
mode without this module. The recorded audio is always kept alongside its
transcript -- never discarded.
"""

from __future__ import annotations

import os
import shutil
import subprocess
from dataclasses import dataclass
from pathlib import Path

DEFAULT_STT_MODEL = "whisper-1"
DEFAULT_TTS_MODEL = "tts-1"
DEFAULT_TTS_VOICE = "alloy"


class VoiceError(RuntimeError):
    pass


@dataclass
class VoiceSettings:
    api_key: str | None = None
    stt_model: str = DEFAULT_STT_MODEL
    tts_model: str = DEFAULT_TTS_MODEL
    tts_voice: str = DEFAULT_TTS_VOICE

    @classmethod
    def from_env(cls) -> VoiceSettings:
        return cls(
            api_key=os.environ.get("LEGACY_OPENAI_API_KEY") or os.environ.get("OPENAI_API_KEY"),
            stt_model=os.environ.get("LEGACY_STT_MODEL", DEFAULT_STT_MODEL),
            tts_model=os.environ.get("LEGACY_TTS_MODEL", DEFAULT_TTS_MODEL),
            tts_voice=os.environ.get("LEGACY_TTS_VOICE", DEFAULT_TTS_VOICE),
        )


def get_client(settings: VoiceSettings):
    if not settings.api_key:
        raise VoiceError(
            "No OpenAI API key configured. Set LEGACY_OPENAI_API_KEY (or OPENAI_API_KEY) "
            "to use --voice. Text mode works fully without it."
        )
    try:
        from openai import OpenAI
    except ImportError as e:
        raise VoiceError(
            "The 'openai' package is required for voice features. Install with: "
            "uv pip install -e '.[llm]'"
        ) from e
    return OpenAI(api_key=settings.api_key)


def record_to_wav(out_path: Path) -> Path:
    """Push-to-talk: starts recording immediately, stops on Enter.

    Shells out to `ffmpeg` capturing the system default microphone. The
    exact input device format is platform-dependent (alsa on Linux,
    avfoundation on macOS, dshow on Windows); LEGACY_FFMPEG_AUDIO_INPUT can
    override the "-f <format> -i <device>" pair if the default doesn't
    match your system, e.g. "avfoundation:0".
    """
    if shutil.which("ffmpeg") is None:
        raise VoiceError("ffmpeg is required to record audio and was not found on PATH.")

    input_spec = os.environ.get("LEGACY_FFMPEG_AUDIO_INPUT", "alsa:default")
    fmt, _, device = input_spec.partition(":")
    out_path.parent.mkdir(parents=True, exist_ok=True)

    proc = subprocess.Popen(
        ["ffmpeg", "-y", "-f", fmt, "-i", device, str(out_path)],
        stdin=subprocess.PIPE,
        stdout=subprocess.DEVNULL,
        stderr=subprocess.DEVNULL,
    )
    input("Recording... press Enter to stop.")
    proc.communicate(input=b"q")  # ffmpeg's interactive "stop gracefully" key
    if proc.returncode not in (0, None) and not out_path.exists():
        raise VoiceError("ffmpeg failed to record audio.")
    return out_path


def transcribe(settings: VoiceSettings, wav_path: Path) -> str:
    client = get_client(settings)
    with open(wav_path, "rb") as f:
        response = client.audio.transcriptions.create(model=settings.stt_model, file=f)
    return response.text


def synthesize(settings: VoiceSettings, text: str, out_path: Path) -> Path:
    client = get_client(settings)
    out_path.parent.mkdir(parents=True, exist_ok=True)
    with client.audio.speech.with_streaming_response.create(
        model=settings.tts_model, voice=settings.tts_voice, input=text
    ) as response:
        response.stream_to_file(out_path)
    return out_path
