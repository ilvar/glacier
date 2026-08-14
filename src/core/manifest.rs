//! `MANIFEST.sha256`: a plain-text checksum list, one line per file.
//!
//! The format matches `sha256sum` output exactly — `<hex>  <path>` — so a
//! reader in thirty years can verify the whole archive with
//! `sha256sum -c MANIFEST.sha256` and nothing else.

use std::fmt;
use std::path::{Path, PathBuf};

use crate::cap;
use crate::core::crypto::sha256_file;

/// Never listed: the manifest cannot contain its own hash, and the search
/// index is rebuildable scratch state rather than part of the archive.
const EXCLUDED_DIRS: [&str; 2] = [".legacy", ".git"];
const EXCLUDED_NAMES: [&str; 1] = ["MANIFEST.sha256"];

#[derive(Debug)]
pub struct ManifestError(pub String);

impl fmt::Display for ManifestError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.0)
    }
}

impl std::error::Error for ManifestError {}

/// Why a file failed verification.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Problem {
    ChecksumMismatch,
    MissingFromManifest,
    MissingFromDisk,
    ManifestAbsent,
}

impl Problem {
    pub fn as_str(self) -> &'static str {
        match self {
            Problem::ChecksumMismatch => "checksum mismatch",
            Problem::MissingFromManifest => "present on disk but not in manifest",
            Problem::MissingFromDisk => "listed in manifest but missing on disk",
            Problem::ManifestAbsent => "manifest file is missing",
        }
    }
}

impl fmt::Display for Problem {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(self.as_str())
    }
}

#[derive(Debug, Clone)]
pub struct Finding {
    pub path: PathBuf,
    pub problem: Problem,
}

fn is_excluded(relative: &Path) -> bool {
    if relative
        .file_name()
        .and_then(|name| name.to_str())
        .is_some_and(|name| EXCLUDED_NAMES.contains(&name))
    {
        return true;
    }
    relative.components().any(|component| {
        component
            .as_os_str()
            .to_str()
            .is_some_and(|part| EXCLUDED_DIRS.contains(&part))
    })
}

/// Archive-relative paths of every file the manifest covers, sorted.
pub fn manifest_files(archive_root: &Path) -> Result<Vec<PathBuf>, ManifestError> {
    let files = cap::fs::walk_files(archive_root).map_err(|error| {
        ManifestError(format!(
            "failed to walk {}: {error}",
            archive_root.display()
        ))
    })?;

    let mut relative_paths = Vec::new();
    for path in files {
        let Ok(relative) = path.strip_prefix(archive_root) else {
            continue;
        };
        if !is_excluded(relative) {
            relative_paths.push(relative.to_path_buf());
        }
    }
    relative_paths.sort();
    Ok(relative_paths)
}

/// Render paths with forward slashes so the manifest reads the same
/// everywhere, regardless of the platform that wrote it.
fn to_posix(path: &Path) -> String {
    path.components()
        .filter_map(|component| component.as_os_str().to_str())
        .collect::<Vec<&str>>()
        .join("/")
}

pub fn build_manifest(archive_root: &Path) -> Result<String, ManifestError> {
    let mut lines = String::new();
    for relative in manifest_files(archive_root)? {
        let digest = sha256_file(&archive_root.join(&relative)).map_err(|error| {
            ManifestError(format!("failed to hash {}: {error}", relative.display()))
        })?;
        lines.push_str(&format!("{digest}  {}\n", to_posix(&relative)));
    }
    Ok(lines)
}

pub fn write_manifest(archive_root: &Path) -> Result<PathBuf, ManifestError> {
    let path = archive_root.join("MANIFEST.sha256");
    let contents = build_manifest(archive_root)?;
    cap::fs::write(&path, &contents)
        .map_err(|error| ManifestError(format!("failed to write manifest: {error}")))?;
    Ok(path)
}

/// Check every file against the recorded checksums, in both directions:
/// changed files, new files, and files that have gone missing.
pub fn verify_manifest(archive_root: &Path) -> Result<Vec<Finding>, ManifestError> {
    let manifest_path = archive_root.join("MANIFEST.sha256");
    if !cap::fs::exists(&manifest_path) {
        return Ok(vec![Finding {
            path: PathBuf::from("MANIFEST.sha256"),
            problem: Problem::ManifestAbsent,
        }]);
    }

    let text = cap::fs::read_to_string(&manifest_path)
        .map_err(|error| ManifestError(format!("failed to read manifest: {error}")))?;

    let mut expected: Vec<(String, String)> = Vec::new();
    for line in text.lines() {
        if line.trim().is_empty() {
            continue;
        }
        if let Some((digest, relative)) = line.split_once("  ") {
            expected.push((relative.to_owned(), digest.to_owned()));
        }
    }

    let mut findings = Vec::new();
    let mut seen: Vec<String> = Vec::new();

    for relative in manifest_files(archive_root)? {
        let key = to_posix(&relative);
        seen.push(key.clone());

        let Some((_, recorded)) = expected.iter().find(|(path, _)| *path == key) else {
            findings.push(Finding {
                path: relative,
                problem: Problem::MissingFromManifest,
            });
            continue;
        };

        let actual = sha256_file(&archive_root.join(&relative)).map_err(|error| {
            ManifestError(format!("failed to hash {}: {error}", relative.display()))
        })?;
        if &actual != recorded {
            findings.push(Finding {
                path: relative,
                problem: Problem::ChecksumMismatch,
            });
        }
    }

    for (relative, _) in &expected {
        if !seen.contains(relative) {
            findings.push(Finding {
                path: PathBuf::from(relative),
                problem: Problem::MissingFromDisk,
            });
        }
    }

    Ok(findings)
}

#[cfg(test)]
mod tests {
    use super::{build_manifest, verify_manifest, write_manifest, Problem};
    use crate::cap;
    use crate::core::vault::Vault;

    fn seeded() -> tempfile::TempDir {
        let temp = tempfile::TempDir::new().expect("temp dir");
        let root = temp.path().join("arch");
        Vault::init(&root, "Jane Doe", 2, 3).expect("init");
        cap::fs::write(&root.join("timeline").join("note.md"), "hello").expect("write");
        temp
    }

    #[test]
    fn manifest_uses_sha256sum_format() {
        let temp = seeded();
        let root = temp.path().join("arch");
        write_manifest(&root).expect("write");
        let text = cap::fs::read_to_string(&root.join("MANIFEST.sha256")).expect("read");

        for line in text.lines() {
            let (digest, path) = line.split_once("  ").expect("two-space separator");
            assert_eq!(digest.len(), 64, "sha256 hex digest");
            assert!(!path.starts_with('/'), "paths are archive-relative");
        }
        // The manifest never lists itself.
        assert!(!text.contains("MANIFEST.sha256"));
    }

    #[test]
    fn a_fresh_manifest_verifies_clean() {
        let temp = seeded();
        let root = temp.path().join("arch");
        write_manifest(&root).expect("write");
        assert!(verify_manifest(&root).expect("verify").is_empty());
    }

    #[test]
    fn detects_a_tampered_file() {
        let temp = seeded();
        let root = temp.path().join("arch");
        write_manifest(&root).expect("write");

        let note = root.join("timeline").join("note.md");
        cap::fs::write(&note, "hello, tampered").expect("write");

        let findings = verify_manifest(&root).expect("verify");
        assert!(findings
            .iter()
            .any(|finding| finding.problem == Problem::ChecksumMismatch));
    }

    #[test]
    fn detects_a_deleted_file() {
        let temp = seeded();
        let root = temp.path().join("arch");
        write_manifest(&root).expect("write");
        cap::fs::remove_file(&root.join("timeline").join("note.md")).expect("remove");

        let findings = verify_manifest(&root).expect("verify");
        assert!(findings
            .iter()
            .any(|finding| finding.problem == Problem::MissingFromDisk));
    }

    #[test]
    fn detects_an_unlisted_file() {
        let temp = seeded();
        let root = temp.path().join("arch");
        write_manifest(&root).expect("write");
        cap::fs::write(&root.join("timeline").join("extra.md"), "new").expect("write");

        let findings = verify_manifest(&root).expect("verify");
        assert!(findings
            .iter()
            .any(|finding| finding.problem == Problem::MissingFromManifest));
    }

    #[test]
    fn excludes_the_rebuildable_index() {
        let temp = seeded();
        let root = temp.path().join("arch");
        cap::fs::create_dir_all(&root.join(".legacy")).expect("mkdir");
        cap::fs::write(&root.join(".legacy").join("index.sqlite3"), "scratch").expect("write");

        let text = build_manifest(&root).expect("build");
        assert!(!text.contains(".legacy"));
    }
}
