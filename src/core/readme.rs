//! Renders the unencrypted `README.txt` that ships in every archive root.
//!
//! This file is the whole point of the "readable in 30 years" constraint:
//! it must let a technician with no knowledge of this program reconstruct
//! the key and decrypt the archive using only `age`, `par2`, and a
//! SLIP-0039 tool. It is regenerated from `archive.yaml` on every
//! `timeline build` and `seal`, so the thresholds and key hashes it quotes
//! are always current.

use crate::core::vault::ArchiveConfig;

/// The plain-bash verifier copied into every archive, so integrity can be
/// checked with no Rust toolchain and no `legacy` binary.
pub const VERIFY_SCRIPT: &str = include_str!("../templates/verify-archive.sh");

const HEADER: &str = r#"LEGACY ARCHIVE — READ THIS FIRST
=================================
"#;

fn preamble(config: &ArchiveConfig) -> String {
    format!(
        r#"
This directory is a "legacy" archive: a life story, in plain files, for
{subject}. It was created on {created} using a
program called "legacy" (https://github.com/ilvar/glacier), but you do not
need that program, or any program, to read most of what is here. Everything
in this file is plain prose. If you are a technician in the future who has
never heard of "legacy", this file alone should be enough.

Archive schema version: {version}
"#,
        subject = config.subject_name,
        created = config.created_at,
        version = config.schema_version,
    )
}

const SECTION_ONE: &str = r#"

1. WHAT IS SAFE TO READ RIGHT NOW, UNENCRYPTED
------------------------------------------------

Everything in this archive root is plain text or a common file format
(Markdown, JPEG, WAV, ...) EXCEPT the contents of the `sealed/` directory,
which are encrypted. You can read stories, look at photos, and browse
freely without any password or key, UNLESS a story's `visibility` field
is "family" or "executor-only" and it has already been moved into
`sealed/` by the `legacy seal` command. Stories marked "public" are never
encrypted and always remain in `timeline/` as plain files.


2. DIRECTORY LAYOUT
---------------------

  archive.yaml        Machine-readable configuration: subject name, schema
                       version, and the encryption/threshold settings for
                       each visibility tier. Plain YAML, human-readable.

  MANIFEST.sha256      One line per file in this archive: a SHA-256 hash and
                       its path. Regenerate and check with `sha256sum -c`
                       (see section 5). Used to detect corruption or
                       accidental edits.

  verify-archive.sh    A small bash script that runs the checks in section 5
                       for you. Plain text; read it before running it.

  timeline.md          A single, generated, human-readable index of every
                       story in chronological order. Purely a convenience —
                       delete it and it will be regenerated the next time
                       `legacy timeline build` runs; the files under
                       timeline/ remain the source of truth.

  timeline/            Story files, one per Markdown (.md) file, grouped
                       into folders by year (e.g. timeline/1994/), or under
                       timeline/undated/ if the story has no known date.
                       See section 3 for the file format.

  people/               One Markdown file per person mentioned in the
                       stories, e.g. people/mary-oconnor.md.

  places/               One Markdown file per place, e.g. places/dublin.md.

  media/                Original photos, scans, and recordings, organized
                       by year. Each media file has a sidecar YAML file
                       with the same name plus ".yaml" describing it, e.g.
                       media/1994/graduation.jpg.yaml next to
                       media/1994/graduation.jpg.

  interviews/            Recorded interview sessions: a Markdown transcript
                       and, if voice was used, the original .wav audio.
                       Both are kept — the audio is never discarded.

  sealed/               Encrypted tar archives, one per visibility tier
                       that has been sealed (see section 4). Each is named
                       <tier>.tar.age, alongside .par2 recovery files.


3. STORY FILE FORMAT
----------------------

Every file under timeline/ is plain UTF-8 Markdown with a YAML "frontmatter"
block at the top, delimited by lines containing only "---". For example:

  ---
  id: 1994-09-15-started-university
  title: Starting university
  date: '1994-09-15'
  date_precision: day
  people:
  - mary-oconnor
  places:
  - dublin
  tags:
  - education
  media: []
  source: interview:2026-08-14-childhood-session-01
  recorded_at: '2026-08-14T19:22:31Z'
  tags_generated_by: null
  visibility: family
  ---

  Free prose. This is the story as the subject told it, lightly cleaned up
  at most.

Any text editor can open these files. `date_precision` may be "day",
"month", "year", "decade", or "unknown" — life stories rarely come with
exact dates, and this archive never invents one. A date of "1970s" means
the decade. `tags_generated_by` is null when a tag was written by a human,
or the name of an AI model if a machine suggested it — this line is never
blurred. `visibility` is one of "public", "family", or "executor-only", and
controls which encrypted tier (if any) the story belongs to once sealed.


4. THE ENCRYPTION SCHEME
--------------------------

This archive uses two off-the-shelf, widely reviewed tools, not anything
custom:

  * age (https://age-encryption.org) for the actual file encryption.
  * SLIP-0039 (https://github.com/satoshilabs/slips/blob/master/slip-0039.md)
    "Shamir's Secret Sharing" for splitting the decryption key into
    several human-readable word lists ("shares"), so that no single
    keyholder can decrypt alone, but a subset of them together can.

For each visibility tier that was sealed ("family", "executor-only"), a
random age identity (a private key) was generated, used to encrypt that
tier's files into `sealed/<tier>.tar.age`, and then THROWN AWAY — it exists
nowhere except split into shares. The shares were printed to the terminal
once, at seal time, and given to trusted keyholders. This archive does not
store them.
"#;

fn tier_section(config: &ArchiveConfig) -> String {
    let mut text = String::new();
    for (name, tier) in &config.tiers {
        text.push_str(&format!(
            "\n  Tier \"{name}\": {} of {} shares are required\n  to reconstruct the key.\n",
            tier.threshold, tier.shares
        ));
        if tier.plaintext_escrow {
            text.push_str(&format!(
                "  This tier was sealed with plaintext escrow: its files are UNENCRYPTED,\n  \
                 plain Markdown/media, under sealed/{name}/ -- there is no key to\n  \
                 reconstruct for it.\n"
            ));
        } else if let Some(digest) = tier.identity_sha256.as_deref() {
            text.push_str(&format!(
                "  SHA-256 of the correct reconstructed identity: {digest}\n  \
                 (compare against this BEFORE concluding the archive is corrupt — if your\n  \
                 reconstructed key's hash matches this, the key is right and any decrypt\n  \
                 failure is a different problem, e.g. a damaged .tar.age file, which\n  \
                 `par2` may be able to repair; see section 5.)\n"
            ));
        } else {
            text.push_str(
                "  This tier has not been sealed yet; no encrypted blob exists for it.\n",
            );
        }
    }
    text
}

const RECONSTRUCTION: &str = r#"
TO RECONSTRUCT THE KEY AND DECRYPT (step by step, no special software
beyond `age` and a SLIP-0039 tool):

  a. Gather at least the threshold number of shares for the tier you want
     to open. Each share is a list of English words.

  b. Reconstruct the age identity from the shares. Using the Python
     "shamir-mnemonic" package (pip install shamir-mnemonic):

       python3 -c "
       import shamir_mnemonic as sm
       shares = [
           'share one words here...',
           'share two words here...',
           'share three words here...',
       ]
       secret = sm.combine_mnemonics(shares)
       print(secret.hex())
       "

     These shares use the original, non-extendable SLIP-0039 variant.
     Recovery tools detect that automatically, so no extra flag is needed
     to combine them; if you ever RE-SPLIT the key with the Python package
     and want shares interchangeable with these, pass extendable=False.

     Then convert those 32 raw bytes back into an age identity string by
     Bech32-encoding them with the human-readable prefix "age-secret-key-",
     and upper-casing the result. If you have the `legacy` program
     available, `legacy unseal --share "..." --share "..."` does steps
     (b) and (c) for you. Otherwise, any Bech32 implementation will do —
     it is a simple checksummed encoding, described at
     https://github.com/bitcoin/bips/blob/master/bip-0173.mediawiki

  c. Verify: SHA-256 the resulting identity string and compare it to the
     "SHA-256 of the correct reconstructed identity" line above for that
     tier. If it matches, you have the right key.

  d. Decrypt with age:

       age -d -i /path/to/identity.txt -o archive.tar sealed/<tier>.tar.age

     (put the reconstructed "AGE-SECRET-KEY-1..." string, and nothing
     else, in identity.txt)

  e. Untar: tar -xf archive.tar

  f. The extracted files are ordinary Markdown, images, and audio — open
     them with anything.


5. INTEGRITY: CHECKSUMS AND BIT-ROT REPAIR
---------------------------------------------

MANIFEST.sha256 lists a SHA-256 checksum for every file in this archive as
of the last time `legacy timeline build` or `legacy seal` ran. To check the
whole archive against it, with no tools beyond `sha256sum`:

  cd (this directory) && sha256sum -c MANIFEST.sha256

Or run the plain-bash script shipped in this archive:

  ./verify-archive.sh

which checks MANIFEST.sha256 and, if present, the par2 recovery data under
sealed/ -- no Rust toolchain or `legacy` install required. `legacy verify`
does the same thing if you have the program installed.

Each sealed/<tier>.tar.age file also has recovery data alongside it,
created with `par2` (15% redundancy), covering small amounts of bit rot or
media damage without needing a backup copy. To check and, if needed, repair:

  cd sealed && par2 verify <tier>.tar.age.par2
  cd sealed && par2 repair <tier>.tar.age.par2     # only if verify fails


6. IMPORTANT NOTE ON THE MOST LIKELY FAILURE MODE
-----------------------------------------------------

In practice, the greater risk to this archive is not a stranger reading it
— it is the family being unable to decrypt it at all: shares lost,
scattered, or a keyholder unreachable. If this archive was sealed with
"plaintext escrow" enabled for a tier, that tier's files were left
UNENCRYPTED in sealed/<tier>/ specifically to avoid that failure. Check
archive.yaml's "plaintext_escrow" field for each tier before assuming
sealed/ requires a key at all.
"#;

/// Build the complete `README.txt` for an archive.
pub fn render(config: &ArchiveConfig) -> String {
    let mut text = String::new();
    text.push_str(HEADER);
    text.push_str(&preamble(config));
    text.push_str(SECTION_ONE);
    text.push_str(&tier_section(config));
    text.push_str(RECONSTRUCTION);
    text
}

#[cfg(test)]
mod tests {
    use super::render;
    use crate::core::vault::ArchiveConfig;

    #[test]
    fn names_the_subject_and_explains_decryption() {
        let config = ArchiveConfig::new("Jane Doe", 3, 5);
        let text = render(&config);
        assert!(text.contains("Jane Doe"));
        assert!(text.contains("age -d -i"));
        assert!(text.contains("sha256sum -c MANIFEST.sha256"));
        assert!(text.contains("combine_mnemonics"));
    }

    #[test]
    fn states_each_tier_threshold() {
        let config = ArchiveConfig::new("Jane Doe", 2, 4);
        let text = render(&config);
        assert!(text.contains("Tier \"family\": 2 of 4 shares"));
        assert!(text.contains("Tier \"executor-only\": 2 of 4 shares"));
        assert!(text.contains("has not been sealed yet"));
    }

    #[test]
    fn quotes_the_key_hash_once_a_tier_is_sealed() {
        let mut config = ArchiveConfig::new("Jane Doe", 2, 3);
        if let Some(tier) = config.tier_mut("family") {
            tier.identity_sha256 = Some("f".repeat(64));
        }
        let text = render(&config);
        assert!(text.contains(&"f".repeat(64)));
        assert!(text.contains("BEFORE concluding the archive is corrupt"));
    }

    #[test]
    fn explains_plaintext_escrow_when_it_is_in_use() {
        let mut config = ArchiveConfig::new("Jane Doe", 2, 3);
        if let Some(tier) = config.tier_mut("family") {
            tier.plaintext_escrow = true;
        }
        let text = render(&config);
        assert!(text.contains("plaintext escrow"));
        assert!(text.contains("sealed/family/"));
    }

    #[test]
    fn documents_the_slip39_variant_for_a_future_reader() {
        // Shares from this program are the non-extendable variant; a future
        // reader re-splitting the key needs to know that to stay compatible.
        let text = render(&ArchiveConfig::new("Jane Doe", 2, 3));
        assert!(text.contains("extendable=False"));
    }
}
