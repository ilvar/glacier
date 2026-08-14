//! Archive open/create and top-level path resolution.
//!
//! `archive.yaml` is the only piece of archive-wide configuration.
//! Everything else the program needs is either derived from it or
//! rebuildable from the plain files on disk (see `index`, `timeline`).

use std::fmt;
use std::path::{Path, PathBuf};

use crate::cap;
use crate::core::clock;
use crate::core::yaml;

pub const SCHEMA_VERSION: u64 = 1;

/// Directories every archive starts with. `timeline/undated` is created
/// eagerly so the layout is legible before any story exists.
pub const ARCHIVE_DIRS: [&str; 6] = [
    "timeline/undated",
    "people",
    "places",
    "media",
    "interviews",
    "sealed",
];

/// The tiers that can be sealed. `public` is deliberately absent: public
/// stories are never encrypted, so they have no tier configuration.
pub const DEFAULT_TIERS: [&str; 2] = ["family", "executor-only"];

#[derive(Debug)]
pub struct VaultError(pub String);

impl fmt::Display for VaultError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.0)
    }
}

impl std::error::Error for VaultError {}

#[derive(Debug, Clone)]
pub struct TierConfig {
    pub threshold: u64,
    pub shares: u64,
    /// SHA-256 of the identity that encrypted this tier, so a reconstructed
    /// key can be checked before anyone concludes the archive is corrupt.
    pub identity_sha256: Option<String>,
    pub plaintext_escrow: bool,
}

impl TierConfig {
    pub fn new(threshold: u64, shares: u64) -> TierConfig {
        TierConfig {
            threshold,
            shares,
            identity_sha256: None,
            plaintext_escrow: false,
        }
    }
}

#[derive(Debug, Clone)]
pub struct ArchiveConfig {
    pub subject_name: String,
    pub schema_version: u64,
    pub created_at: String,
    /// Ordered so `archive.yaml` round-trips with stable field order.
    pub tiers: Vec<(String, TierConfig)>,
    pub keyholders: Vec<String>,
    pub replica_sunset: Option<String>,
}

impl ArchiveConfig {
    pub fn new(subject_name: &str, threshold: u64, shares: u64) -> ArchiveConfig {
        ArchiveConfig {
            subject_name: subject_name.to_owned(),
            schema_version: SCHEMA_VERSION,
            created_at: clock::now_timestamp(),
            tiers: DEFAULT_TIERS
                .iter()
                .map(|name| ((*name).to_owned(), TierConfig::new(threshold, shares)))
                .collect(),
            keyholders: Vec::new(),
            replica_sunset: None,
        }
    }

    pub fn tier(&self, name: &str) -> Option<&TierConfig> {
        self.tiers
            .iter()
            .find(|(tier_name, _)| tier_name == name)
            .map(|(_, config)| config)
    }

    pub fn tier_mut(&mut self, name: &str) -> Option<&mut TierConfig> {
        self.tiers
            .iter_mut()
            .find(|(tier_name, _)| tier_name == name)
            .map(|(_, config)| config)
    }

    pub fn render(&self) -> String {
        let mut emitter = yaml::Emitter::new();
        emitter
            .unsigned("schema_version", self.schema_version)
            .open_map("subject")
            .raw(&format!("  name: {}", yaml::scalar(&self.subject_name)))
            .string("created_at", &self.created_at)
            .open_map("tiers");

        for (name, tier) in &self.tiers {
            emitter
                .raw(&format!("  {}:", yaml::scalar(name)))
                .raw(&format!("    threshold: {}", tier.threshold))
                .raw(&format!("    shares: {}", tier.shares))
                .raw(&match tier.identity_sha256.as_deref() {
                    Some(digest) => format!("    identity_sha256: {}", yaml::scalar(digest)),
                    None => "    identity_sha256: null".to_owned(),
                })
                .raw(&format!("    plaintext_escrow: {}", tier.plaintext_escrow));
        }

        emitter
            .list("keyholders", &self.keyholders)
            .open_map("replica");
        emitter.raw(&match self.replica_sunset.as_deref() {
            Some(sunset) => format!("  sunset: {}", yaml::scalar(sunset)),
            None => "  sunset: null".to_owned(),
        });
        emitter.finish()
    }

    pub fn parse(text: &str) -> Result<ArchiveConfig, VaultError> {
        let document = yaml::parse(text).map_err(VaultError)?;

        let subject = yaml::as_mapping(&document, "subject")
            .ok_or_else(|| VaultError("archive.yaml has no 'subject' section".to_owned()))?;
        let subject_name = yaml::get_string(&subject, "name")
            .ok_or_else(|| VaultError("archive.yaml has no subject name".to_owned()))?;

        let mut tiers = Vec::new();
        if let Some(serde_yaml_ng::Value::Mapping(mapping)) = document.get("tiers") {
            for (name, value) in mapping {
                if let serde_yaml_ng::Value::String(name) = name {
                    tiers.push((
                        name.clone(),
                        TierConfig {
                            threshold: yaml::get_u64(value, "threshold").unwrap_or(3),
                            shares: yaml::get_u64(value, "shares").unwrap_or(5),
                            identity_sha256: yaml::get_string(value, "identity_sha256"),
                            plaintext_escrow: yaml::get_bool(value, "plaintext_escrow", false),
                        },
                    ));
                }
            }
        }

        let replica_sunset = yaml::as_mapping(&document, "replica")
            .and_then(|replica| yaml::get_string(&replica, "sunset"));

        Ok(ArchiveConfig {
            subject_name,
            schema_version: yaml::get_u64(&document, "schema_version").unwrap_or(SCHEMA_VERSION),
            created_at: yaml::get_string(&document, "created_at")
                .unwrap_or_else(clock::now_timestamp),
            tiers,
            keyholders: yaml::get_list(&document, "keyholders"),
            replica_sunset,
        })
    }
}

/// A single archive on disk.
#[derive(Debug, Clone)]
pub struct Vault {
    pub root: PathBuf,
}

impl Vault {
    pub fn at(root: &Path) -> Vault {
        Vault {
            root: root.to_path_buf(),
        }
    }

    pub fn config_path(&self) -> PathBuf {
        self.root.join("archive.yaml")
    }

    pub fn manifest_path(&self) -> PathBuf {
        self.root.join("MANIFEST.sha256")
    }

    pub fn readme_path(&self) -> PathBuf {
        self.root.join("README.txt")
    }

    pub fn timeline_dir(&self) -> PathBuf {
        self.root.join("timeline")
    }

    pub fn people_dir(&self) -> PathBuf {
        self.root.join("people")
    }

    pub fn places_dir(&self) -> PathBuf {
        self.root.join("places")
    }

    pub fn media_dir(&self) -> PathBuf {
        self.root.join("media")
    }

    pub fn interviews_dir(&self) -> PathBuf {
        self.root.join("interviews")
    }

    pub fn sealed_dir(&self) -> PathBuf {
        self.root.join("sealed")
    }

    /// The search index lives in a dotted directory so it is obviously not
    /// part of the archive proper, and is excluded from MANIFEST.sha256.
    pub fn index_db_path(&self) -> PathBuf {
        self.root.join(".legacy").join("index.sqlite3")
    }

    pub fn exists(&self) -> bool {
        cap::fs::exists(&self.config_path())
    }

    pub fn load_config(&self) -> Result<ArchiveConfig, VaultError> {
        if !self.exists() {
            return Err(VaultError(format!(
                "{} is not a legacy archive (no archive.yaml)",
                self.root.display()
            )));
        }
        let text = cap::fs::read_to_string(&self.config_path())
            .map_err(|error| VaultError(format!("failed to read archive.yaml: {error}")))?;
        ArchiveConfig::parse(&text)
    }

    pub fn save_config(&self, config: &ArchiveConfig) -> Result<(), VaultError> {
        cap::fs::write(&self.config_path(), &config.render())
            .map_err(|error| VaultError(format!("failed to write archive.yaml: {error}")))
    }

    /// Open an existing archive, failing if there is not one here.
    pub fn open(root: &Path) -> Result<Vault, VaultError> {
        let vault = Vault::at(root);
        if !vault.exists() {
            return Err(VaultError(format!(
                "{} is not a legacy archive (no archive.yaml found)",
                root.display()
            )));
        }
        Ok(vault)
    }

    /// Create a new, empty archive. Refuses to touch a non-empty directory.
    pub fn init(
        root: &Path,
        subject_name: &str,
        threshold: u64,
        shares: u64,
    ) -> Result<Vault, VaultError> {
        if cap::fs::exists(root) {
            let entries = cap::fs::read_dir_sorted(root).map_err(|error| {
                VaultError(format!("failed to inspect {}: {error}", root.display()))
            })?;
            if !entries.is_empty() {
                return Err(VaultError(format!(
                    "{} already exists and is not empty",
                    root.display()
                )));
            }
        }

        for relative in ARCHIVE_DIRS {
            cap::fs::create_dir_all(&root.join(relative))
                .map_err(|error| VaultError(format!("failed to create {relative}: {error}")))?;
        }

        let vault = Vault::at(root);
        let config = ArchiveConfig::new(subject_name, threshold, shares);
        vault.save_config(&config)?;

        cap::fs::write(&vault.readme_path(), &crate::core::readme::render(&config))
            .map_err(|error| VaultError(format!("failed to write README.txt: {error}")))?;

        // The bash verifier travels with the archive so integrity can be
        // checked without this program installed.
        let script = root.join("verify-archive.sh");
        cap::fs::write(&script, crate::core::readme::VERIFY_SCRIPT)
            .map_err(|error| VaultError(format!("failed to write verify-archive.sh: {error}")))?;
        cap::fs::set_executable(&script).map_err(|error| {
            VaultError(format!(
                "failed to mark verify-archive.sh executable: {error}"
            ))
        })?;

        crate::core::manifest::write_manifest(root)
            .map_err(|error| VaultError(format!("failed to write MANIFEST.sha256: {error}")))?;

        Ok(vault)
    }
}

#[cfg(test)]
mod tests {
    use super::{ArchiveConfig, Vault};
    use crate::cap;

    #[test]
    fn init_creates_the_layout() {
        let temp = tempfile::TempDir::new().expect("temp dir");
        let root = temp.path().join("my-archive");
        let vault = Vault::init(&root, "Jane Doe", 3, 5).expect("init");

        assert!(vault.config_path().exists());
        assert!(vault.readme_path().exists());
        assert!(vault.manifest_path().exists());
        assert!(vault.timeline_dir().is_dir());
        assert!(vault.people_dir().is_dir());
        assert!(vault.places_dir().is_dir());
        assert!(vault.media_dir().is_dir());
        assert!(vault.interviews_dir().is_dir());
        assert!(root.join("verify-archive.sh").exists());

        let config = vault.load_config().expect("config loads");
        assert_eq!(config.subject_name, "Jane Doe");
        assert_eq!(config.tier("family").expect("family tier").threshold, 3);
        assert_eq!(config.tier("family").expect("family tier").shares, 5);
        assert!(config.tier("executor-only").is_some());

        let readme = cap::fs::read_to_string(&vault.readme_path()).expect("readme");
        assert!(readme.contains("Jane Doe"));
        assert!(readme.contains("age -d -i"));
    }

    #[test]
    fn init_refuses_a_non_empty_directory() {
        let temp = tempfile::TempDir::new().expect("temp dir");
        let root = temp.path().join("existing");
        cap::fs::create_dir_all(&root).expect("mkdir");
        cap::fs::write(&root.join("somefile"), "hi").expect("write");
        assert!(Vault::init(&root, "Someone", 3, 5).is_err());
    }

    #[test]
    fn open_requires_an_existing_archive() {
        let temp = tempfile::TempDir::new().expect("temp dir");
        assert!(Vault::open(&temp.path().join("nope")).is_err());
    }

    #[test]
    fn config_round_trips() {
        let temp = tempfile::TempDir::new().expect("temp dir");
        let root = temp.path().join("arch");
        let vault = Vault::init(&root, "Jane Doe", 2, 4).expect("init");

        let mut config = vault.load_config().expect("load");
        if let Some(tier) = config.tier_mut("family") {
            tier.identity_sha256 = Some("a".repeat(64));
            tier.plaintext_escrow = true;
        }
        config.replica_sunset = Some("2050-01-01".to_owned());
        vault.save_config(&config).expect("save");

        let reloaded = vault.load_config().expect("reload");
        let family = reloaded.tier("family").expect("family tier");
        assert_eq!(family.identity_sha256.as_deref(), Some(&"a".repeat(64)[..]));
        assert!(family.plaintext_escrow);
        assert_eq!(reloaded.replica_sunset.as_deref(), Some("2050-01-01"));
        assert_eq!(reloaded.subject_name, "Jane Doe");
        assert_eq!(reloaded.tiers.len(), 2);
    }

    #[test]
    fn rendered_config_is_valid_yaml_with_quoted_timestamp() {
        let config = ArchiveConfig::new("Jane Doe", 3, 5);
        let rendered = config.render();
        assert!(rendered.contains("schema_version: 1"));
        assert!(rendered.contains("  name: Jane Doe"));
        assert!(rendered.contains("plaintext_escrow: false"));
        ArchiveConfig::parse(&rendered).expect("rendered config parses");
    }
}
