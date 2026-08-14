//! Seal and unseal: one encrypted tar per visibility tier, with the key
//! split by SLIP-0039, or that process run backwards given enough shares.
//!
//! Sealing never touches or deletes the plaintext archive — it is purely
//! additive. `sealed/<tier>.tar.age` is a distributable snapshot for
//! keyholders; the author's own working copy stays exactly as it was, so
//! there is no moment where the only copy of a story is one nobody can yet
//! decrypt.
//!
//! Tier membership:
//!   - `public` stories are never sealed. They stay plain in `timeline/`
//!     permanently, since they are meant to be readable by anyone.
//!   - `family` and `executor-only` stories go into the matching tier's
//!     bundle, along with any `media/` files they reference.
//!   - `people/`, `places/`, and `interviews/` go into the **family**
//!     bundle only. They are shared reference material and raw session
//!     recordings, which default to family-level sensitivity; duplicating
//!     them into the executor bundle would widen their exposure for no
//!     gain.

use std::fmt;
use std::path::{Path, PathBuf};

use crate::cap;
use crate::core::crypto::{self, CryptoError};
use crate::core::story::{iter_stories, Story, Visibility};
use crate::core::vault::{ArchiveConfig, Vault};

/// Directories bundled into the family tier only.
pub const FAMILY_EXTRA_DIRS: [&str; 3] = ["people", "places", "interviews"];

#[derive(Debug)]
pub struct SealError(pub String);

impl fmt::Display for SealError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.0)
    }
}

impl std::error::Error for SealError {}

impl From<CryptoError> for SealError {
    fn from(error: CryptoError) -> SealError {
        SealError(error.0)
    }
}

/// What sealing one tier produced.
#[derive(Debug, Clone)]
pub struct SealedTier {
    pub tier: String,
    pub plaintext_escrow: bool,
    pub story_count: usize,
    /// The `.tar.age` file, or the plaintext directory when escrowed.
    pub output_path: PathBuf,
    pub par2_path: Option<PathBuf>,
    /// `None` when escrowed — there is no key to share.
    pub shares: Option<Vec<String>>,
    pub identity_sha256: Option<String>,
}

#[derive(Debug, Clone)]
pub struct Unsealed {
    pub tier: String,
    pub output_dir: PathBuf,
    pub file_count: usize,
}

fn stories_for_tier(archive_root: &Path, tier: &str) -> Result<Vec<Story>, SealError> {
    let wanted = Visibility::parse(tier).map_err(|error| SealError(error.0))?;
    let stories = iter_stories(archive_root).map_err(|error| SealError(error.0))?;
    Ok(stories
        .into_iter()
        .filter(|story| story.visibility == wanted)
        .collect())
}

/// Copy this tier's files into `staging`, preserving relative paths.
fn stage_tier(archive_root: &Path, tier: &str, staging: &Path) -> Result<usize, SealError> {
    let stories = stories_for_tier(archive_root, tier)?;
    let mut media_paths: Vec<String> = Vec::new();

    for story in &stories {
        let relative = story.relative_path();
        let source = archive_root.join(&relative);
        let destination = staging.join(&relative);
        let bytes = cap::fs::read(&source)
            .map_err(|error| SealError(format!("failed to read {}: {error}", source.display())))?;
        cap::fs::write_bytes(&destination, &bytes).map_err(|error| {
            SealError(format!("failed to stage {}: {error}", relative.display()))
        })?;

        for media in &story.media {
            if !media_paths.contains(media) {
                media_paths.push(media.clone());
            }
        }
    }

    // Referenced media travels with the stories, so an unsealed bundle is
    // self-contained rather than full of dangling links.
    for media in &media_paths {
        let source = archive_root.join(media);
        if !cap::fs::is_file(&source) {
            continue;
        }
        copy_into_staging(&source, &staging.join(media))?;

        let sidecar = crate::core::media::sidecar_path_for(&source);
        if cap::fs::is_file(&sidecar) {
            let sidecar_destination = crate::core::media::sidecar_path_for(&staging.join(media));
            copy_into_staging(&sidecar, &sidecar_destination)?;
        }
    }

    if tier == "family" {
        for directory in FAMILY_EXTRA_DIRS {
            let source_dir = archive_root.join(directory);
            if !cap::fs::is_dir(&source_dir) {
                continue;
            }
            let files = cap::fs::walk_files(&source_dir)
                .map_err(|error| SealError(format!("failed to walk {directory}: {error}")))?;
            for source in files {
                let Ok(relative) = source.strip_prefix(archive_root) else {
                    continue;
                };
                copy_into_staging(&source, &staging.join(relative))?;
            }
        }
    }

    Ok(stories.len())
}

fn copy_into_staging(source: &Path, destination: &Path) -> Result<(), SealError> {
    cap::fs::copy(source, destination)
        .map_err(|error| SealError(format!("failed to stage {}: {error}", source.display())))?;
    Ok(())
}

fn copy_tree(source: &Path, destination: &Path) -> Result<(), SealError> {
    let files = cap::fs::walk_files(source)
        .map_err(|error| SealError(format!("failed to walk staging area: {error}")))?;
    for path in files {
        let Ok(relative) = path.strip_prefix(source) else {
            continue;
        };
        copy_into_staging(&path, &destination.join(relative))?;
    }
    Ok(())
}

/// Seal one tier. Updates `config` in place with the key hash and escrow
/// flag; the caller is responsible for saving it.
pub fn seal_tier(
    vault: &Vault,
    config: &mut ArchiveConfig,
    tier: &str,
    plaintext_escrow: bool,
) -> Result<SealedTier, SealError> {
    let tier_config = config
        .tier(tier)
        .cloned()
        .ok_or_else(|| SealError(format!("no tier config for {tier:?} in archive.yaml")))?;

    cap::fs::create_dir_all(&vault.sealed_dir())
        .map_err(|error| SealError(format!("failed to create sealed/: {error}")))?;

    let scratch = cap::fs::temp_dir()
        .map_err(|error| SealError(format!("failed to create scratch directory: {error}")))?;
    let staging = scratch.path().join("staging");
    cap::fs::create_dir_all(&staging)
        .map_err(|error| SealError(format!("failed to create staging area: {error}")))?;

    let story_count = stage_tier(&vault.root, tier, &staging)?;

    if plaintext_escrow {
        let output_dir = vault.sealed_dir().join(tier);
        if cap::fs::exists(&output_dir) {
            cap::fs::remove_dir_all(&output_dir).map_err(|error| {
                SealError(format!(
                    "failed to replace {}: {error}",
                    output_dir.display()
                ))
            })?;
        }
        cap::fs::create_dir_all(&output_dir)
            .map_err(|error| SealError(format!("failed to create escrow directory: {error}")))?;
        copy_tree(&staging, &output_dir)?;

        if let Some(entry) = config.tier_mut(tier) {
            entry.plaintext_escrow = true;
            entry.identity_sha256 = None;
        }
        return Ok(SealedTier {
            tier: tier.to_owned(),
            plaintext_escrow: true,
            story_count,
            output_path: output_dir,
            par2_path: None,
            shares: None,
            identity_sha256: None,
        });
    }

    let tar_path = scratch.path().join(format!("{tier}.tar"));
    write_tar(&staging, &tar_path)?;

    // A fresh identity per tier, used once and never stored: after this
    // function returns it exists only inside the shares we print.
    let identity = crypto::generate_identity()?;
    let output_path = vault.sealed_dir().join(format!("{tier}.tar.age"));
    crypto::age_encrypt(&tar_path, &identity.recipient, &output_path)?;

    let shares = crypto::split_identity(
        &identity.identity,
        tier_config.threshold,
        tier_config.shares,
    )?;
    let par2_path = crypto::par2_create(&output_path)?;
    let digest = identity.sha256();

    if let Some(entry) = config.tier_mut(tier) {
        entry.plaintext_escrow = false;
        entry.identity_sha256 = Some(digest.clone());
    }

    Ok(SealedTier {
        tier: tier.to_owned(),
        plaintext_escrow: false,
        story_count,
        output_path,
        par2_path,
        shares: Some(shares),
        identity_sha256: Some(digest),
    })
}

/// Bundle a directory into an ordinary tar. Plain `tar` and nothing else,
/// so `tar -xf` works on any machine that ever existed.
fn write_tar(staging: &Path, tar_path: &Path) -> Result<(), SealError> {
    let file = cap::fs::create(tar_path)
        .map_err(|error| SealError(format!("failed to create tar: {error}")))?;
    let mut builder = tar::Builder::new(file);
    builder
        .append_dir_all(".", staging)
        .map_err(|error| SealError(format!("failed to build tar: {error}")))?;
    builder
        .finish()
        .map_err(|error| SealError(format!("failed to finish tar: {error}")))?;
    Ok(())
}

/// Reconstruct the identity from `shares`, work out which tier it belongs
/// to by comparing hashes against `archive.yaml`, and decrypt that tier.
///
/// The caller does not name the tier: the key itself identifies it, so a
/// keyholder with a handful of word lists does not need to be told which
/// door they open.
pub fn unseal(
    vault: &Vault,
    config: &ArchiveConfig,
    shares: &[String],
    output_base: &Path,
) -> Result<Unsealed, SealError> {
    let identity = crypto::combine_shares(shares)?;
    let digest = crypto::identity_sha256(&identity);

    let matching = config
        .tiers
        .iter()
        .find(|(_, tier)| tier.identity_sha256.as_deref() == Some(digest.as_str()))
        .map(|(name, _)| name.clone());

    let Some(tier) = matching else {
        return Err(SealError(
            "reconstructed a key, but its SHA-256 does not match any tier in archive.yaml. \
             Check that you have enough correct shares for the same tier."
                .to_owned(),
        ));
    };

    let sealed_path = vault.sealed_dir().join(format!("{tier}.tar.age"));
    if !cap::fs::exists(&sealed_path) {
        return Err(SealError(format!(
            "key matches tier {tier:?} but {} does not exist",
            sealed_path.display()
        )));
    }

    let output_dir = output_base.join(&tier);
    let scratch = cap::fs::temp_dir()
        .map_err(|error| SealError(format!("failed to create scratch directory: {error}")))?;
    let tar_path = scratch.path().join("archive.tar");

    crypto::age_decrypt(&sealed_path, &identity, &tar_path)?;
    cap::fs::create_dir_all(&output_dir)
        .map_err(|error| SealError(format!("failed to create output directory: {error}")))?;

    let file = cap::fs::open(&tar_path)
        .map_err(|error| SealError(format!("failed to open decrypted tar: {error}")))?;
    let mut archive = tar::Archive::new(file);
    archive
        .unpack(&output_dir)
        .map_err(|error| SealError(format!("failed to extract tar: {error}")))?;

    let file_count = cap::fs::walk_files(&output_dir)
        .map_err(|error| SealError(format!("failed to count extracted files: {error}")))?
        .len();

    Ok(Unsealed {
        tier,
        output_dir,
        file_count,
    })
}

#[cfg(test)]
mod tests {
    use super::{seal_tier, unseal};
    use crate::cap;
    use crate::core::story::{new_story, save_story, NewStory, Visibility};
    use crate::core::vault::Vault;

    fn age_available() -> bool {
        cap::process::which("age") && cap::process::which("age-keygen")
    }

    fn seeded() -> (tempfile::TempDir, Vault) {
        let temp = tempfile::TempDir::new().expect("temp dir");
        let vault = Vault::init(&temp.path().join("arch"), "Jane Doe", 2, 3).expect("init");
        for (title, date, body, visibility) in [
            (
                "Public fact",
                "1990",
                "Anyone can read this.",
                Visibility::Public,
            ),
            (
                "Family story",
                "1994",
                "For the family.",
                Visibility::Family,
            ),
            (
                "Executor secret",
                "2000",
                "Only the executor should read this.",
                Visibility::ExecutorOnly,
            ),
        ] {
            let story = new_story(NewStory {
                title: title.to_owned(),
                date: Some(date.to_owned()),
                body: body.to_owned(),
                visibility: Some(visibility),
                ..NewStory::default()
            })
            .expect("story");
            save_story(&vault.root, &story, false).expect("save");
        }
        (temp, vault)
    }

    #[test]
    fn seals_only_the_requested_tier() {
        if !age_available() {
            return;
        }
        let (_temp, vault) = seeded();
        let mut config = vault.load_config().expect("config");
        let result = seal_tier(&vault, &mut config, "family", false).expect("seal");

        assert!(!result.plaintext_escrow);
        assert_eq!(result.story_count, 1, "only the one family story");
        assert!(cap::fs::exists(&result.output_path));
        assert_eq!(
            result.output_path.file_name().and_then(|n| n.to_str()),
            Some("family.tar.age")
        );
        assert_eq!(result.shares.as_ref().map(Vec::len), Some(3));
        assert_eq!(
            result.identity_sha256,
            config
                .tier("family")
                .and_then(|tier| tier.identity_sha256.clone())
        );
    }

    #[test]
    fn sealing_leaves_the_plaintext_archive_untouched() {
        if !age_available() {
            return;
        }
        let (_temp, vault) = seeded();
        let before = cap::fs::walk_files(&vault.timeline_dir()).expect("walk");
        let mut config = vault.load_config().expect("config");
        seal_tier(&vault, &mut config, "family", false).expect("seal");

        let after = cap::fs::walk_files(&vault.timeline_dir()).expect("walk");
        assert_eq!(before, after, "seal must never remove or move source files");
    }

    #[test]
    fn public_stories_are_never_encrypted() {
        if !age_available() {
            return;
        }
        let (_temp, vault) = seeded();
        let mut config = vault.load_config().expect("config");
        seal_tier(&vault, &mut config, "family", false).expect("seal");
        seal_tier(&vault, &mut config, "executor-only", false).expect("seal");

        // The public story is still readable with no key at all.
        let public = vault
            .timeline_dir()
            .join("1990")
            .join("1990-public-fact.md");
        assert!(cap::fs::exists(&public));
        let text = cap::fs::read_to_string(&public).expect("read");
        assert!(text.contains("Anyone can read this."));
    }

    #[test]
    fn seal_then_unseal_round_trips() {
        if !age_available() {
            return;
        }
        let (_temp, vault) = seeded();
        let mut config = vault.load_config().expect("config");
        let sealed = seal_tier(&vault, &mut config, "executor-only", false).expect("seal");
        vault.save_config(&config).expect("save config");

        let shares = sealed.shares.expect("shares");
        let reloaded = vault.load_config().expect("reload");
        let threshold_shares: Vec<String> = shares.iter().take(2).cloned().collect();

        let result = unseal(
            &vault,
            &reloaded,
            &threshold_shares,
            &vault.root.join("unsealed"),
        )
        .expect("unseal");

        assert_eq!(result.tier, "executor-only");
        let files = cap::fs::walk_files(&result.output_dir).expect("walk");
        let contents: String = files
            .iter()
            .filter_map(|path| cap::fs::read_to_string(path).ok())
            .collect();
        assert!(contents.contains("Only the executor should read this."));
    }

    #[test]
    fn unseal_identifies_the_tier_from_the_key_alone() {
        if !age_available() {
            return;
        }
        let (_temp, vault) = seeded();
        let mut config = vault.load_config().expect("config");
        let family = seal_tier(&vault, &mut config, "family", false).expect("seal");
        seal_tier(&vault, &mut config, "executor-only", false).expect("seal");
        vault.save_config(&config).expect("save");

        let shares: Vec<String> = family
            .shares
            .expect("shares")
            .iter()
            .take(2)
            .cloned()
            .collect();
        let reloaded = vault.load_config().expect("reload");
        let result =
            unseal(&vault, &reloaded, &shares, &vault.root.join("unsealed")).expect("unseal");
        assert_eq!(result.tier, "family", "the key alone names its own tier");
    }

    #[test]
    fn unseal_rejects_shares_that_match_no_tier() {
        if !age_available() {
            return;
        }
        let (_temp, vault) = seeded();
        let mut config = vault.load_config().expect("config");
        seal_tier(&vault, &mut config, "family", false).expect("seal");
        vault.save_config(&config).expect("save");

        // A valid, well-formed key for a different secret entirely.
        let stranger = crate::core::crypto::generate_identity().expect("keygen");
        let shares = crate::core::crypto::split_identity(&stranger.identity, 2, 3).expect("split");

        let reloaded = vault.load_config().expect("reload");
        let error = unseal(
            &vault,
            &reloaded,
            &shares[..2],
            &vault.root.join("unsealed"),
        )
        .expect_err("should refuse");
        assert!(error.0.contains("does not match any tier"));
    }

    #[test]
    fn plaintext_escrow_leaves_the_family_tier_readable() {
        let (_temp, vault) = seeded();
        let mut config = vault.load_config().expect("config");
        let result = seal_tier(&vault, &mut config, "family", true).expect("escrow");

        assert!(result.plaintext_escrow);
        assert!(result.shares.is_none());
        assert_eq!(
            config.tier("family").map(|tier| tier.plaintext_escrow),
            Some(true)
        );
        assert_eq!(
            config
                .tier("family")
                .and_then(|tier| tier.identity_sha256.clone()),
            None
        );

        let files = cap::fs::walk_files(&result.output_path).expect("walk");
        let contents: String = files
            .iter()
            .filter_map(|path| cap::fs::read_to_string(path).ok())
            .collect();
        assert!(contents.contains("For the family."));
    }

    #[test]
    fn the_family_bundle_carries_people_places_and_interviews() {
        let (_temp, vault) = seeded();
        cap::fs::write(
            &vault.people_dir().join("mary.md"),
            "---\nname: Mary\n---\n\nMary was kind.\n",
        )
        .expect("write");
        cap::fs::write(
            &vault.interviews_dir().join("session-01.md"),
            "---\nsession_id: x\n---\n\nTranscript.\n",
        )
        .expect("write");

        let mut config = vault.load_config().expect("config");
        let result = seal_tier(&vault, &mut config, "family", true).expect("escrow");

        assert!(cap::fs::exists(
            &result.output_path.join("people").join("mary.md")
        ));
        assert!(cap::fs::exists(
            &result.output_path.join("interviews").join("session-01.md")
        ));
    }

    #[test]
    fn the_executor_bundle_excludes_shared_reference_material() {
        let (_temp, vault) = seeded();
        cap::fs::write(
            &vault.people_dir().join("mary.md"),
            "---\nname: Mary\n---\n\nKind.\n",
        )
        .expect("write");

        let mut config = vault.load_config().expect("config");
        let result = seal_tier(&vault, &mut config, "executor-only", true).expect("escrow");
        assert!(!cap::fs::exists(
            &result.output_path.join("people").join("mary.md")
        ));
    }

    #[test]
    fn referenced_media_travels_with_its_story() {
        let (_temp, vault) = seeded();
        let media_relative = "media/1994/photo.jpg";
        cap::fs::write_bytes(&vault.root.join(media_relative), b"jpeg-bytes").expect("write");
        cap::fs::write(
            &vault.root.join(format!("{media_relative}.yaml")),
            "original_filename: photo.jpg\nsha256: abc\n",
        )
        .expect("write");

        let story = new_story(NewStory {
            title: "With a photo".to_owned(),
            date: Some("1994".to_owned()),
            body: "See the photo.".to_owned(),
            media: vec![media_relative.to_owned()],
            visibility: Some(Visibility::Family),
            ..NewStory::default()
        })
        .expect("story");
        save_story(&vault.root, &story, false).expect("save");

        let mut config = vault.load_config().expect("config");
        let result = seal_tier(&vault, &mut config, "family", true).expect("escrow");

        assert!(cap::fs::exists(&result.output_path.join(media_relative)));
        assert!(cap::fs::exists(
            &result.output_path.join(format!("{media_relative}.yaml"))
        ));
    }

    #[test]
    fn an_unknown_tier_is_refused() {
        let (_temp, vault) = seeded();
        let mut config = vault.load_config().expect("config");
        assert!(seal_tier(&vault, &mut config, "nonexistent", false).is_err());
    }
}
