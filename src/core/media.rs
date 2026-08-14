//! Media ingestion: copies original files into `media/`, verbatim, with a
//! YAML sidecar describing each one.
//!
//! Nothing here ever transcodes, resizes, or rewrites a media file. The
//! bytes that arrive are the bytes that are stored; the sidecar is purely
//! descriptive metadata living beside the original. A photo whose format
//! becomes unfashionable is still the photo.

use std::fmt;
use std::path::{Path, PathBuf};

use crate::cap;
use crate::core::clock;
use crate::core::crypto::sha256_file;
use crate::core::dates::{parse_fuzzy_date, FuzzyDate, Precision};
use crate::core::story::{slugify, Visibility};
use crate::core::yaml::{self, Emitter};

/// Extensions we recognize as media. Anything else in a source directory
/// is left alone rather than guessed at.
const MEDIA_EXTENSIONS: [&str; 19] = [
    "jpg", "jpeg", "png", "heic", "tif", "tiff", "gif", "bmp", "webp", "wav", "mp3", "m4a", "flac",
    "ogg", "mp4", "mov", "avi", "mkv", "webm",
];

/// Where a media file's date came from. Recorded so a reader can tell a
/// camera's own timestamp from a human's later guess.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DateSource {
    Exif,
    Filesystem,
    Manual,
    Unknown,
}

impl DateSource {
    pub fn as_str(self) -> &'static str {
        match self {
            DateSource::Exif => "exif",
            DateSource::Filesystem => "filesystem",
            DateSource::Manual => "manual",
            DateSource::Unknown => "unknown",
        }
    }

    pub fn from_str_lossy(raw: &str) -> DateSource {
        match raw {
            "exif" => DateSource::Exif,
            "filesystem" => DateSource::Filesystem,
            "manual" => DateSource::Manual,
            "unknown" => DateSource::Unknown,
            _unrecognized => DateSource::Unknown,
        }
    }
}

#[derive(Debug)]
pub struct MediaError(pub String);

impl fmt::Display for MediaError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.0)
    }
}

impl std::error::Error for MediaError {}

#[derive(Debug, Clone)]
pub struct Sidecar {
    pub original_filename: String,
    pub ingested_at: String,
    pub sha256: String,
    pub size_bytes: u64,
    pub mime_type: Option<String>,
    pub date: Option<String>,
    pub date_precision: Precision,
    pub date_source: DateSource,
    pub width: Option<u64>,
    pub height: Option<u64>,
    pub duration_seconds: Option<f64>,
    pub visibility: Visibility,
    pub caption: Option<String>,
}

impl Sidecar {
    pub fn render(&self) -> String {
        let mut emitter = Emitter::new();
        emitter
            .string("original_filename", &self.original_filename)
            .string("ingested_at", &self.ingested_at)
            .string("sha256", &self.sha256)
            .unsigned("size_bytes", self.size_bytes)
            .optional_string("mime_type", self.mime_type.as_deref())
            .optional_string("date", self.date.as_deref())
            .string("date_precision", self.date_precision.as_str())
            .string("date_source", self.date_source.as_str())
            .optional_unsigned("width", self.width)
            .optional_unsigned("height", self.height)
            .optional_float("duration_seconds", self.duration_seconds)
            .string("visibility", self.visibility.as_str())
            .optional_string("caption", self.caption.as_deref());
        emitter.finish()
    }

    pub fn parse(text: &str) -> Result<Sidecar, MediaError> {
        let document = yaml::parse(text).map_err(MediaError)?;
        let visibility = match yaml::get_string(&document, "visibility") {
            Some(raw) => Visibility::parse(&raw).map_err(|error| MediaError(error.0))?,
            None => Visibility::Family,
        };
        Ok(Sidecar {
            original_filename: yaml::get_string(&document, "original_filename")
                .ok_or_else(|| MediaError("sidecar has no original_filename".to_owned()))?,
            ingested_at: yaml::get_string(&document, "ingested_at")
                .unwrap_or_else(clock::now_timestamp),
            sha256: yaml::get_string(&document, "sha256")
                .ok_or_else(|| MediaError("sidecar has no sha256".to_owned()))?,
            size_bytes: yaml::get_u64(&document, "size_bytes").unwrap_or(0),
            mime_type: yaml::get_string(&document, "mime_type"),
            date: yaml::get_string(&document, "date"),
            date_precision: yaml::get_string(&document, "date_precision")
                .map_or(Precision::Unknown, |raw| Precision::from_str_lossy(&raw)),
            date_source: yaml::get_string(&document, "date_source")
                .map_or(DateSource::Unknown, |raw| DateSource::from_str_lossy(&raw)),
            width: yaml::get_u64(&document, "width"),
            height: yaml::get_u64(&document, "height"),
            duration_seconds: yaml::get_f64(&document, "duration_seconds"),
            visibility,
            caption: yaml::get_string(&document, "caption"),
        })
    }
}

#[derive(Debug, Clone)]
pub struct Ingested {
    pub source: PathBuf,
    pub destination: PathBuf,
    pub sidecar_path: PathBuf,
    pub sidecar: Sidecar,
}

/// Options for one ingest run.
#[derive(Debug, Clone, Default)]
pub struct IngestOptions {
    /// Try to read a capture date from embedded metadata via ffprobe.
    pub date_from_exif: bool,
    /// Apply this date to everything ingested in this run.
    pub manual_date: Option<String>,
    pub visibility: Option<Visibility>,
}

fn extension_of(path: &Path) -> Option<String> {
    path.extension()
        .and_then(|extension| extension.to_str())
        .map(str::to_lowercase)
}

fn is_media(path: &Path) -> bool {
    extension_of(path).is_some_and(|extension| MEDIA_EXTENSIONS.contains(&extension.as_str()))
}

/// A small, explicit extension-to-type table. We only claim a type for the
/// formats we accept, and say nothing rather than guess for the rest.
fn mime_for(extension: &str) -> Option<&'static str> {
    match extension {
        "jpg" | "jpeg" => Some("image/jpeg"),
        "png" => Some("image/png"),
        "heic" => Some("image/heic"),
        "tif" | "tiff" => Some("image/tiff"),
        "gif" => Some("image/gif"),
        "bmp" => Some("image/bmp"),
        "webp" => Some("image/webp"),
        "wav" => Some("audio/wav"),
        "mp3" => Some("audio/mpeg"),
        "m4a" => Some("audio/mp4"),
        "flac" => Some("audio/flac"),
        "ogg" => Some("audio/ogg"),
        "mp4" => Some("video/mp4"),
        "mov" => Some("video/quicktime"),
        "avi" => Some("video/x-msvideo"),
        "mkv" => Some("video/x-matroska"),
        "webm" => Some("video/webm"),
        _unknown => None,
    }
}

/// Every recognized media file under `source_dir`, sorted.
pub fn iter_media_files(source_dir: &Path) -> Result<Vec<PathBuf>, MediaError> {
    let files = cap::fs::walk_files(source_dir)
        .map_err(|error| MediaError(format!("failed to read {}: {error}", source_dir.display())))?;
    Ok(files.into_iter().filter(|path| is_media(path)).collect())
}

/// Metadata ffprobe reported, if it was available and could read the file.
#[derive(Debug, Clone, Default)]
struct Probe {
    width: Option<u64>,
    height: Option<u64>,
    duration_seconds: Option<f64>,
    creation_date: Option<FuzzyDate>,
}

/// Best-effort probe. A missing ffprobe or an unreadable file is not an
/// error: the media is still ingested, just with less metadata.
fn probe_media(path: &Path) -> Probe {
    if !cap::process::which("ffprobe") {
        return Probe::default();
    }
    let Some(path_text) = path.to_str() else {
        return Probe::default();
    };
    let Ok(output) = cap::process::run(
        "ffprobe",
        &[
            "-v",
            "quiet",
            "-print_format",
            "json",
            "-show_format",
            "-show_streams",
            path_text,
        ],
    ) else {
        return Probe::default();
    };
    if !output.success {
        return Probe::default();
    }
    let Ok(document) = serde_json::from_str::<serde_json::Value>(&output.stdout) else {
        return Probe::default();
    };

    let mut probe = Probe::default();
    if let Some(streams) = document.get("streams").and_then(|value| value.as_array()) {
        for stream in streams {
            let codec_type = stream.get("codec_type").and_then(|value| value.as_str());
            if codec_type == Some("video") || codec_type == Some("image") {
                probe.width = stream.get("width").and_then(|value| value.as_u64());
                probe.height = stream.get("height").and_then(|value| value.as_u64());
            }
        }
    }
    if let Some(format) = document.get("format") {
        probe.duration_seconds = format
            .get("duration")
            .and_then(|value| value.as_str())
            .and_then(|value| value.parse::<f64>().ok());
        probe.creation_date = format.get("tags").and_then(creation_date_from_tags);
    }
    probe
}

/// Pull a capture date out of ffprobe's tag bag, trying the spellings
/// different cameras and containers use.
fn creation_date_from_tags(tags: &serde_json::Value) -> Option<FuzzyDate> {
    const KEYS: [&str; 4] = [
        "creation_time",
        "date",
        "DateTimeOriginal",
        "com.apple.quicktime.creationdate",
    ];
    for key in KEYS {
        let Some(raw) = tags.get(key).and_then(|value| value.as_str()) else {
            continue;
        };
        // Both "1994:09:15 10:30:00" and "1994-09-15T10:30:00Z" appear in
        // the wild; take the first ten characters and normalize.
        let head: String = raw.chars().take(10).collect();
        let normalized = head.replace(':', "-");
        if let Ok(date) = parse_fuzzy_date(Some(&normalized)) {
            if date.value.is_some() {
                return Some(date);
            }
        }
    }
    None
}

/// Pick a destination that does not collide with an existing file.
fn unique_destination(
    media_dir: &Path,
    year_bucket: &str,
    base_slug: &str,
    extension: &str,
) -> PathBuf {
    let bucket = media_dir.join(year_bucket);
    let mut counter = 1_u32;
    loop {
        let candidate = bucket.join(format!("{base_slug}-{counter:03}.{extension}"));
        if !cap::fs::exists(&candidate) {
            return candidate;
        }
        counter = counter.saturating_add(1);
    }
}

/// Copy one media file into the archive and write its sidecar.
pub fn ingest_file(
    source: &Path,
    media_dir: &Path,
    options: &IngestOptions,
) -> Result<Ingested, MediaError> {
    if !cap::fs::is_file(source) {
        return Err(MediaError(format!("{} is not a file", source.display())));
    }
    let visibility = options.visibility.unwrap_or(Visibility::Family);

    let digest = sha256_file(source).map_err(|error| MediaError(error.0))?;
    let size_bytes = cap::fs::file_size(source)
        .map_err(|error| MediaError(format!("failed to stat {}: {error}", source.display())))?;
    let extension = extension_of(source).unwrap_or_else(|| "bin".to_owned());
    let mime_type = mime_for(&extension).map(str::to_owned);

    let probe = probe_media(source);

    // A date given on the command line always wins over an embedded one:
    // the person doing the ingest knows more than the camera did.
    let (date, date_source) = match options.manual_date.as_deref() {
        Some(manual) => (
            parse_fuzzy_date(Some(manual)).map_err(|error| MediaError(error.0))?,
            DateSource::Manual,
        ),
        None => match probe
            .creation_date
            .clone()
            .filter(|_| options.date_from_exif)
        {
            Some(embedded) => (embedded, DateSource::Exif),
            None => (FuzzyDate::unknown(), DateSource::Unknown),
        },
    };

    let year_bucket = match date.year() {
        Some(year) => year.to_string(),
        None => "undated".to_owned(),
    };
    let stem = source
        .file_stem()
        .and_then(|stem| stem.to_str())
        .unwrap_or("media");
    let slug = {
        let slug = slugify(stem);
        if slug.is_empty() {
            "media".to_owned()
        } else {
            slug
        }
    };
    let base_slug = match date.value.as_deref() {
        Some(value) => format!("{value}-{slug}"),
        None => slug,
    };

    let destination = unique_destination(media_dir, &year_bucket, &base_slug, &extension);
    cap::fs::copy(source, &destination).map_err(|error| {
        MediaError(format!(
            "failed to copy {} to {}: {error}",
            source.display(),
            destination.display()
        ))
    })?;

    let sidecar = Sidecar {
        original_filename: source
            .file_name()
            .and_then(|name| name.to_str())
            .unwrap_or("unknown")
            .to_owned(),
        ingested_at: clock::now_timestamp(),
        sha256: digest,
        size_bytes,
        mime_type,
        date: date.value.clone(),
        date_precision: date.precision,
        date_source,
        width: probe.width,
        height: probe.height,
        duration_seconds: probe.duration_seconds,
        visibility,
        caption: None,
    };

    let sidecar_path = sidecar_path_for(&destination);
    cap::fs::write(&sidecar_path, &sidecar.render())
        .map_err(|error| MediaError(format!("failed to write sidecar: {error}")))?;

    Ok(Ingested {
        source: source.to_path_buf(),
        destination,
        sidecar_path,
        sidecar,
    })
}

/// The sidecar sits beside its media file with `.yaml` appended, so the
/// pairing survives any sort order and is obvious to a human browsing.
pub fn sidecar_path_for(media_path: &Path) -> PathBuf {
    let name = media_path
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or("media");
    media_path.with_file_name(format!("{name}.yaml"))
}

pub fn load_sidecar(path: &Path) -> Result<Sidecar, MediaError> {
    let text = cap::fs::read_to_string(path)
        .map_err(|error| MediaError(format!("failed to read {}: {error}", path.display())))?;
    Sidecar::parse(&text)
}

pub fn ingest_directory(
    source_dir: &Path,
    media_dir: &Path,
    options: &IngestOptions,
) -> Result<Vec<Ingested>, MediaError> {
    let mut ingested = Vec::new();
    for path in iter_media_files(source_dir)? {
        ingested.push(ingest_file(&path, media_dir, options)?);
    }
    Ok(ingested)
}

#[cfg(test)]
mod tests {
    use super::{ingest_directory, ingest_file, load_sidecar, DateSource, IngestOptions};
    use crate::cap;
    use crate::core::dates::Precision;
    use crate::core::story::Visibility;
    use crate::core::vault::Vault;
    use std::path::PathBuf;

    fn archive() -> (tempfile::TempDir, Vault) {
        let temp = tempfile::TempDir::new().expect("temp dir");
        let vault = Vault::init(&temp.path().join("arch"), "Jane Doe", 2, 3).expect("init");
        (temp, vault)
    }

    fn source_file(temp: &tempfile::TempDir, name: &str, bytes: &[u8]) -> PathBuf {
        let directory = temp.path().join("incoming");
        cap::fs::create_dir_all(&directory).expect("mkdir");
        let path = directory.join(name);
        cap::fs::write_bytes(&path, bytes).expect("write");
        path
    }

    #[test]
    fn copies_the_file_and_writes_a_sidecar() {
        let (temp, vault) = archive();
        let source = source_file(&temp, "IMG_0001.JPG", b"fake-jpeg-bytes");

        let options = IngestOptions {
            manual_date: Some("1994-09-15".to_owned()),
            ..IngestOptions::default()
        };
        let result = ingest_file(&source, &vault.media_dir(), &options).expect("ingest");

        assert!(cap::fs::exists(&result.destination));
        assert_eq!(
            result.destination.parent().and_then(|p| p.file_name()),
            Some(std::ffi::OsStr::new("1994"))
        );
        assert_eq!(
            result.destination.file_name().and_then(|n| n.to_str()),
            Some("1994-09-15-img-0001-001.jpg")
        );

        let sidecar = load_sidecar(&result.sidecar_path).expect("sidecar");
        assert_eq!(sidecar.original_filename, "IMG_0001.JPG");
        assert_eq!(sidecar.date.as_deref(), Some("1994-09-15"));
        assert_eq!(sidecar.date_precision, Precision::Day);
        assert_eq!(sidecar.date_source, DateSource::Manual);
        assert_eq!(sidecar.visibility, Visibility::Family);
        assert_eq!(sidecar.mime_type.as_deref(), Some("image/jpeg"));
        assert_eq!(sidecar.size_bytes, 15);
    }

    #[test]
    fn stores_the_original_bytes_unchanged() {
        let (temp, vault) = archive();
        // Every byte value, so any transcoding or text mangling shows up.
        let content: Vec<u8> = (0..=255_u8).cycle().take(4096).collect();
        let source = source_file(&temp, "raw.wav", &content);

        let options = IngestOptions {
            manual_date: Some("2005-01".to_owned()),
            ..IngestOptions::default()
        };
        let result = ingest_file(&source, &vault.media_dir(), &options).expect("ingest");
        assert_eq!(
            cap::fs::read(&result.destination).expect("read"),
            content,
            "media must be stored byte-for-byte"
        );
    }

    #[test]
    fn recorded_checksum_matches_the_stored_file() {
        let (temp, vault) = archive();
        let source = source_file(&temp, "scan.png", b"some bytes here");
        let result =
            ingest_file(&source, &vault.media_dir(), &IngestOptions::default()).expect("ingest");

        let actual = crate::core::crypto::sha256_file(&result.destination).expect("hash");
        assert_eq!(actual, result.sidecar.sha256);
    }

    #[test]
    fn an_undated_file_lands_in_the_undated_bucket() {
        let (temp, vault) = archive();
        let source = source_file(&temp, "scan.png", b"bytes");
        let result =
            ingest_file(&source, &vault.media_dir(), &IngestOptions::default()).expect("ingest");

        assert_eq!(
            result.destination.parent().and_then(|p| p.file_name()),
            Some(std::ffi::OsStr::new("undated"))
        );
        assert_eq!(result.sidecar.date, None);
        assert_eq!(result.sidecar.date_precision, Precision::Unknown);
        assert_eq!(result.sidecar.date_source, DateSource::Unknown);
    }

    #[test]
    fn never_invents_a_date_when_exif_is_absent() {
        let (temp, vault) = archive();
        let source = source_file(&temp, "nodate.jpg", b"no metadata in here");
        let options = IngestOptions {
            date_from_exif: true,
            ..IngestOptions::default()
        };
        let result = ingest_file(&source, &vault.media_dir(), &options).expect("ingest");
        assert_eq!(result.sidecar.date, None);
        assert_eq!(result.sidecar.date_source, DateSource::Unknown);
    }

    #[test]
    fn ingests_a_directory_and_skips_non_media() {
        let (temp, vault) = archive();
        source_file(&temp, "a.jpg", b"one");
        source_file(&temp, "b.jpg", b"two");
        source_file(&temp, "notes.txt", b"not media");

        let options = IngestOptions {
            manual_date: Some("2000".to_owned()),
            ..IngestOptions::default()
        };
        let results = ingest_directory(&temp.path().join("incoming"), &vault.media_dir(), &options)
            .expect("ingest");

        assert_eq!(results.len(), 2);
        let names: Vec<String> = results
            .iter()
            .filter_map(|r| r.destination.file_name().and_then(|n| n.to_str()))
            .map(str::to_owned)
            .collect();
        assert_eq!(names.len(), 2);
        assert_ne!(
            names.first(),
            names.get(1),
            "identically named sources must not collide"
        );
    }

    #[test]
    fn two_files_with_the_same_name_both_survive() {
        let (temp, vault) = archive();
        let first = source_file(&temp, "photo.jpg", b"first");
        let options = IngestOptions {
            manual_date: Some("1999".to_owned()),
            ..IngestOptions::default()
        };
        let one = ingest_file(&first, &vault.media_dir(), &options).expect("first");
        let two = ingest_file(&first, &vault.media_dir(), &options).expect("second");

        assert_ne!(one.destination, two.destination);
        assert!(cap::fs::exists(&one.destination));
        assert!(cap::fs::exists(&two.destination));
    }

    #[test]
    fn rejects_a_visibility_that_is_not_a_tier() {
        // Visibility is a closed enum, so an invalid value cannot be
        // constructed; the parse path is what a hand-edited sidecar hits.
        assert!(Visibility::parse("top-secret").is_err());
    }
}
