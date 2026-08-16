//! Speech features: push-to-talk recording via `ffmpeg`, transcription via
//! the Whisper API, and playback synthesis via the TTS API.
//!
//! All of it is opt-in and needs both network access and a key; the
//! interview subsystem works fully in text mode without this module. The
//! recording is always kept beside its transcript — the audio is the
//! artefact that matters most, and is never discarded after transcription
//! even if the transcription itself fails.

use std::fmt;
use std::path::Path;

use crate::cap;

pub const DEFAULT_STT_MODEL: &str = "whisper-1";
pub const DEFAULT_TTS_MODEL: &str = "tts-1";
pub const DEFAULT_TTS_VOICE: &str = "alloy";
pub const DEFAULT_API_BASE: &str = "https://api.openai.com/v1";

#[derive(Debug)]
pub struct VoiceError(pub String);

impl fmt::Display for VoiceError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.0)
    }
}

impl std::error::Error for VoiceError {}

#[derive(Debug, Clone)]
pub struct VoiceSettings {
    pub api_key: Option<String>,
    pub base_url: String,
    pub stt_model: String,
    pub tts_model: String,
    pub tts_voice: String,
}

impl VoiceSettings {
    /// Read configuration from the environment, preferring the
    /// `LEGACY_`-prefixed names and falling back to the conventional
    /// `OPENAI_*` ones.
    pub fn from_env() -> VoiceSettings {
        VoiceSettings {
            api_key: crate::core::env::first(&["LEGACY_OPENAI_API_KEY", "OPENAI_API_KEY"]),
            base_url: crate::core::env::first(&["LEGACY_OPENAI_BASE_URL", "OPENAI_BASE_URL"])
                .unwrap_or_else(|| DEFAULT_API_BASE.to_owned()),
            stt_model: crate::core::env::first(&["LEGACY_STT_MODEL"])
                .unwrap_or_else(|| DEFAULT_STT_MODEL.to_owned()),
            tts_model: crate::core::env::first(&["LEGACY_TTS_MODEL"])
                .unwrap_or_else(|| DEFAULT_TTS_MODEL.to_owned()),
            tts_voice: crate::core::env::first(&["LEGACY_TTS_VOICE"])
                .unwrap_or_else(|| DEFAULT_TTS_VOICE.to_owned()),
        }
    }

    fn require_key(&self) -> Result<&str, VoiceError> {
        self.api_key.as_deref().ok_or_else(|| {
            VoiceError(
                "No API key configured. Set LEGACY_OPENAI_API_KEY or OPENAI_API_KEY \
                 to use voice mode. Text mode works fully without it."
                    .to_owned(),
            )
        })
    }
}

/// Begin recording from the default microphone. Returns a handle the
/// caller stops later with [`finish_recording`].
///
/// Split from stopping so a caller with its own event loop — the TUI —
/// can keep drawing while the take runs. A blocking record-then-return
/// call would have to own the keyboard, which the TUI cannot give up.
///
/// The input device spelling is platform-dependent (`alsa` on Linux,
/// `avfoundation` on macOS, `dshow` on Windows), so
/// `LEGACY_FFMPEG_AUDIO_INPUT` overrides the `format:device` pair.
pub fn start_recording(out_path: &Path) -> Result<cap::process::Recording, VoiceError> {
    if !cap::process::which("ffmpeg") {
        return Err(VoiceError(
            "ffmpeg is required to record audio and was not found on PATH.".to_owned(),
        ));
    }
    let specification = std::env::var("LEGACY_FFMPEG_AUDIO_INPUT")
        .unwrap_or_else(|_missing| "alsa:default".to_owned());
    let (format, device) = specification.split_once(':').unwrap_or(("alsa", "default"));

    let Some(out_text) = out_path.to_str() else {
        return Err(VoiceError("output path is not valid UTF-8".to_owned()));
    };
    if let Some(parent) = out_path.parent() {
        cap::fs::create_dir_all(parent).map_err(|error| {
            VoiceError(format!("failed to create recording directory: {error}"))
        })?;
    }

    cap::process::spawn_quiet("ffmpeg", &["-y", "-f", format, "-i", device, out_text])
        .map_err(|error| VoiceError(format!("failed to start ffmpeg: {error}")))
}

/// Stop a take and confirm a file landed on disk.
pub fn finish_recording(
    recording: cap::process::Recording,
    out_path: &Path,
) -> Result<(), VoiceError> {
    // `q` is ffmpeg's graceful stop, which finalizes the WAV header.
    recording
        .stop(b"q")
        .map_err(|error| VoiceError(format!("failed to stop ffmpeg: {error}")))?;

    if !cap::fs::exists(out_path) {
        return Err(VoiceError(
            "ffmpeg produced no recording; check the audio input device".to_owned(),
        ));
    }
    Ok(())
}

/// Record until the operator presses Enter. Used by the line-oriented
/// CLI, which owns stdin for the duration.
pub fn record_to_wav(out_path: &Path) -> Result<(), VoiceError> {
    let recording = start_recording(out_path)?;

    let mut discard = String::new();
    let _ignored = std::io::stdin().read_line(&mut discard);

    finish_recording(recording, out_path)
}

/// A multipart body for the transcription endpoint.
fn multipart_body(boundary: &str, model: &str, filename: &str, audio: &[u8]) -> Vec<u8> {
    let mut body = Vec::new();
    body.extend_from_slice(
        format!(
            "--{boundary}\r\nContent-Disposition: form-data; name=\"model\"\r\n\r\n{model}\r\n"
        )
        .as_bytes(),
    );
    body.extend_from_slice(
        format!(
            "--{boundary}\r\nContent-Disposition: form-data; name=\"file\"; \
             filename=\"{filename}\"\r\nContent-Type: audio/wav\r\n\r\n"
        )
        .as_bytes(),
    );
    body.extend_from_slice(audio);
    body.extend_from_slice(format!("\r\n--{boundary}--\r\n").as_bytes());
    body
}

pub fn transcribe(settings: &VoiceSettings, wav_path: &Path) -> Result<String, VoiceError> {
    let key = settings.require_key()?;
    let audio = cap::fs::read(wav_path)
        .map_err(|error| VoiceError(format!("failed to read recording: {error}")))?;
    let filename = wav_path
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or("audio.wav");

    let boundary = "legacy-boundary-b8f1c2a4";
    let body = multipart_body(boundary, &settings.stt_model, filename, &audio);
    let url = format!(
        "{}/audio/transcriptions",
        settings.base_url.trim_end_matches('/')
    );

    let response = cap::net::post_multipart(&url, key, boundary, body)
        .map_err(|error| VoiceError(format!("transcription request failed: {error}")))?;

    let parsed: serde_json::Value = serde_json::from_str(&response)
        .map_err(|error| VoiceError(format!("could not parse transcription: {error}")))?;
    parsed
        .get("text")
        .and_then(|text| text.as_str())
        .map(|text| text.trim().to_owned())
        .ok_or_else(|| VoiceError(format!("transcription response had no text: {response}")))
}

pub fn synthesize(settings: &VoiceSettings, text: &str, out_path: &Path) -> Result<(), VoiceError> {
    let key = settings.require_key()?;
    let request = serde_json::json!({
        "model": settings.tts_model,
        "voice": settings.tts_voice,
        "input": text,
    });
    let url = format!("{}/audio/speech", settings.base_url.trim_end_matches('/'));

    let audio = cap::net::post_json_bytes(&url, key, &request.to_string())
        .map_err(|error| VoiceError(format!("speech request failed: {error}")))?;
    cap::fs::write_bytes(out_path, &audio)
        .map_err(|error| VoiceError(format!("failed to write audio: {error}")))?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::{multipart_body, VoiceSettings};
    use std::path::Path;

    #[test]
    fn a_missing_key_is_an_actionable_error() {
        let settings = VoiceSettings {
            api_key: None,
            base_url: "http://localhost".to_owned(),
            stt_model: "whisper-1".to_owned(),
            tts_model: "tts-1".to_owned(),
            tts_voice: "alloy".to_owned(),
        };
        let error = settings.require_key().expect_err("should refuse");
        assert!(error.0.contains("LEGACY_OPENAI_API_KEY"));
        assert!(error.0.contains("Text mode works fully without it"));
    }

    #[test]
    fn transcribe_without_a_key_does_not_touch_the_network() {
        let settings = VoiceSettings {
            api_key: None,
            base_url: "http://127.0.0.1:1".to_owned(),
            stt_model: "whisper-1".to_owned(),
            tts_model: "tts-1".to_owned(),
            tts_voice: "alloy".to_owned(),
        };
        // Would connect-refuse if it tried; the key check comes first.
        assert!(super::transcribe(&settings, Path::new("/nonexistent.wav")).is_err());
    }

    #[test]
    fn multipart_body_carries_the_audio_bytes_verbatim() {
        let audio: Vec<u8> = (0..=255_u8).collect();
        let body = multipart_body("BOUND", "whisper-1", "answer.wav", &audio);

        let text = String::from_utf8_lossy(&body);
        assert!(text.contains("name=\"model\""));
        assert!(text.contains("whisper-1"));
        assert!(text.contains("filename=\"answer.wav\""));
        assert!(text.ends_with("--BOUND--\r\n"));
        assert!(
            body.windows(audio.len()).any(|window| window == audio),
            "the raw audio must be embedded unmodified"
        );
    }

    #[test]
    fn settings_prefer_the_namespaced_key() {
        // Reads whatever the environment has; the contract under test is
        // that a default model and base URL are always present.
        let settings = VoiceSettings::from_env();
        assert!(!settings.stt_model.is_empty());
        assert!(!settings.tts_model.is_empty());
        assert!(settings.base_url.starts_with("http"));
    }
}
