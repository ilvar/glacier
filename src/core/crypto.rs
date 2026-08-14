//! Thin wrappers around `age`, `age-keygen`, and SLIP-0039 secret sharing.
//!
//! No cryptographic primitive is implemented here. `age` does the
//! encryption and the `sssmc39` crate does the secret splitting. The only
//! thing this module adds is the glue that turns an age identity into its
//! 32 raw bytes and back — a Bech32 text encoding, not a cipher — so that
//! those bytes can be split with SLIP-0039 and reassembled later.

use std::fmt;
use std::io::Read;
use std::path::{Path, PathBuf};

use bech32::primitives::decode::CheckedHrpstring;
use bech32::{Bech32, Hrp};
use sha2::Digest;

use crate::cap;

/// The human-readable prefix age uses for private keys. Note the trailing
/// hyphen: the Bech32 separator `1` follows it directly.
pub const IDENTITY_HRP: &str = "age-secret-key-";

/// Redundancy for `par2`, as a percentage of each sealed blob.
pub const PAR2_REDUNDANCY: u32 = 15;

#[derive(Debug)]
pub struct CryptoError(pub String);

impl fmt::Display for CryptoError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.0)
    }
}

impl std::error::Error for CryptoError {}

fn require(binary: &str) -> Result<(), CryptoError> {
    if cap::process::which(binary) {
        return Ok(());
    }
    Err(CryptoError(format!(
        "required tool {binary:?} was not found on PATH. Install it before running this command."
    )))
}

/// An age key pair. The identity is the secret; the recipient is public.
#[derive(Debug, Clone)]
pub struct Identity {
    pub identity: String,
    pub recipient: String,
}

impl Identity {
    pub fn sha256(&self) -> String {
        identity_sha256(&self.identity)
    }
}

pub fn sha256_hex(bytes: &[u8]) -> String {
    let digest = sha2::Sha256::digest(bytes);
    digest.iter().map(|byte| format!("{byte:02x}")).collect()
}

/// SHA-256 of the identity string, recorded in `archive.yaml` so a
/// reconstructed key can be verified before anyone concludes the archive
/// is corrupt.
pub fn identity_sha256(identity: &str) -> String {
    sha256_hex(identity.trim().as_bytes())
}

/// Stream a file through SHA-256 so arbitrarily large media can be hashed.
pub fn sha256_file(path: &Path) -> Result<String, CryptoError> {
    let mut file = cap::fs::open(path)
        .map_err(|error| CryptoError(format!("failed to open {}: {error}", path.display())))?;
    let mut hasher = sha2::Sha256::new();
    let mut buffer = vec![0_u8; 1024 * 1024];
    loop {
        let read = file
            .read(&mut buffer)
            .map_err(|error| CryptoError(format!("failed to read {}: {error}", path.display())))?;
        if read == 0 {
            break;
        }
        match buffer.get(..read) {
            Some(chunk) => hasher.update(chunk),
            None => break,
        }
    }
    Ok(hasher
        .finalize()
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect())
}

/// Generate a fresh age identity by running `age-keygen`.
pub fn generate_identity() -> Result<Identity, CryptoError> {
    require("age-keygen")?;
    let output = cap::process::run("age-keygen", &[])
        .map_err(|error| CryptoError(format!("failed to run age-keygen: {error}")))?;

    let mut identity = None;
    let mut recipient = None;
    // age-keygen splits its output across stdout and stderr, so scan both.
    for line in output.stdout.lines().chain(output.stderr.lines()) {
        let line = line.trim();
        if let Some(value) = line.strip_prefix("Public key:") {
            recipient = Some(value.trim().to_owned());
        } else if line.starts_with("AGE-SECRET-KEY-") {
            identity = Some(line.to_owned());
        }
    }

    match (identity, recipient) {
        (Some(identity), Some(recipient)) => Ok(Identity {
            identity,
            recipient,
        }),
        _incomplete => Err(CryptoError(format!(
            "could not parse age-keygen output:\n{}\n{}",
            output.stdout, output.stderr
        ))),
    }
}

/// Derive the public recipient for an identity via `age-keygen -y`.
pub fn recipient_for_identity(identity: &str) -> Result<String, CryptoError> {
    require("age-keygen")?;
    let scratch = cap::fs::temp_dir()
        .map_err(|error| CryptoError(format!("failed to create scratch directory: {error}")))?;
    let identity_path = scratch.path().join("identity.txt");
    write_identity_file(&identity_path, identity)?;

    let path_text = path_to_str(&identity_path)?;
    let output = cap::process::run("age-keygen", &["-y", path_text])
        .map_err(|error| CryptoError(format!("failed to run age-keygen -y: {error}")))?;
    if !output.success {
        return Err(CryptoError(format!(
            "age-keygen -y failed: {}",
            output.stderr.trim()
        )));
    }
    Ok(output.stdout.trim().to_owned())
}

fn path_to_str(path: &Path) -> Result<&str, CryptoError> {
    path.to_str()
        .ok_or_else(|| CryptoError(format!("path is not valid UTF-8: {}", path.display())))
}

/// Write an identity to a file readable only by its owner.
fn write_identity_file(path: &Path, identity: &str) -> Result<(), CryptoError> {
    cap::fs::write(path, &format!("{}\n", identity.trim()))
        .map_err(|error| CryptoError(format!("failed to write identity file: {error}")))
}

/// The 32 raw key bytes inside an age identity string.
pub fn identity_to_bytes(identity: &str) -> Result<Vec<u8>, CryptoError> {
    let trimmed = identity.trim();
    let checked = CheckedHrpstring::new::<Bech32>(trimmed)
        .map_err(|error| CryptoError(format!("not a valid age identity: {error}")))?;
    let hrp = checked.hrp();
    if !hrp.as_str().eq_ignore_ascii_case(IDENTITY_HRP) {
        return Err(CryptoError(format!(
            "not an age identity (unexpected prefix {:?})",
            hrp.as_str()
        )));
    }
    let payload: Vec<u8> = checked.byte_iter().collect();
    if payload.len() != 32 {
        return Err(CryptoError(format!(
            "unexpected age identity length: {} bytes",
            payload.len()
        )));
    }
    Ok(payload)
}

/// Rebuild the age identity string from its 32 raw key bytes.
pub fn bytes_to_identity(data: &[u8]) -> Result<String, CryptoError> {
    if data.len() != 32 {
        return Err(CryptoError(format!(
            "expected 32 bytes, got {}",
            data.len()
        )));
    }
    let hrp = Hrp::parse(IDENTITY_HRP)
        .map_err(|error| CryptoError(format!("invalid identity prefix: {error}")))?;
    let encoded = bech32::encode_upper::<Bech32>(hrp, data)
        .map_err(|error| CryptoError(format!("failed to encode identity: {error}")))?;
    Ok(encoded)
}

/// Split an age identity into `shares` SLIP-0039 mnemonics, of which
/// `threshold` are needed to reconstruct it.
pub fn split_identity(
    identity: &str,
    threshold: u64,
    shares: u64,
) -> Result<Vec<String>, CryptoError> {
    let threshold = u8::try_from(threshold)
        .map_err(|_| CryptoError("threshold must be between 1 and 255".to_owned()))?;
    let shares = u8::try_from(shares)
        .map_err(|_| CryptoError("share count must be between 1 and 255".to_owned()))?;
    if threshold == 0 || shares == 0 || threshold > shares {
        return Err(CryptoError(format!(
            "invalid threshold {threshold} of {shares}: need 1 <= threshold <= shares"
        )));
    }

    let secret = identity_to_bytes(identity)?;
    let groups = sssmc39::generate_mnemonics(1, &[(threshold, shares)], &secret, "", 0)
        .map_err(|error| CryptoError(format!("failed to split key: {error}")))?;
    let group = groups
        .first()
        .ok_or_else(|| CryptoError("secret sharing produced no group".to_owned()))?;
    group
        .mnemonic_list_flat()
        .map_err(|error| CryptoError(format!("failed to render shares: {error}")))
}

/// Reconstruct an age identity from SLIP-0039 mnemonic shares.
pub fn combine_shares(mnemonics: &[String]) -> Result<String, CryptoError> {
    if mnemonics.is_empty() {
        return Err(CryptoError("no shares were supplied".to_owned()));
    }
    let words: Vec<Vec<String>> = mnemonics
        .iter()
        .map(|mnemonic| {
            mnemonic
                .split_whitespace()
                .map(str::to_owned)
                .collect::<Vec<String>>()
        })
        .collect();

    let secret = sssmc39::combine_mnemonics(&words, "")
        .map_err(|error| CryptoError(format!("could not reconstruct key from shares: {error}")))?;
    bytes_to_identity(&secret)
}

pub fn age_encrypt(plaintext: &Path, recipient: &str, output: &Path) -> Result<(), CryptoError> {
    require("age")?;
    let result = cap::process::run(
        "age",
        &[
            "-r",
            recipient,
            "-o",
            path_to_str(output)?,
            path_to_str(plaintext)?,
        ],
    )
    .map_err(|error| CryptoError(format!("failed to run age: {error}")))?;
    if !result.success {
        return Err(CryptoError(format!(
            "age encryption failed: {}",
            result.stderr.trim()
        )));
    }
    Ok(())
}

pub fn age_decrypt(ciphertext: &Path, identity: &str, output: &Path) -> Result<(), CryptoError> {
    require("age")?;
    let scratch = cap::fs::temp_dir()
        .map_err(|error| CryptoError(format!("failed to create scratch directory: {error}")))?;
    let identity_path = scratch.path().join("identity.txt");
    write_identity_file(&identity_path, identity)?;

    let result = cap::process::run(
        "age",
        &[
            "-d",
            "-i",
            path_to_str(&identity_path)?,
            "-o",
            path_to_str(output)?,
            path_to_str(ciphertext)?,
        ],
    )
    .map_err(|error| CryptoError(format!("failed to run age: {error}")))?;
    if !result.success {
        return Err(CryptoError(format!(
            "age decryption failed: {}",
            result.stderr.trim()
        )));
    }
    Ok(())
}

/// Create par2 recovery files next to `path`. Returns `None` when par2 is
/// not installed — a missing par2 is a warning, not a failure, because the
/// archive itself is still complete without it.
pub fn par2_create(path: &Path) -> Result<Option<PathBuf>, CryptoError> {
    if !cap::process::which("par2") {
        return Ok(None);
    }
    let directory = path.parent().unwrap_or(Path::new("."));
    let name = path
        .file_name()
        .and_then(|name| name.to_str())
        .ok_or_else(|| CryptoError(format!("unusable filename: {}", path.display())))?;

    let redundancy = format!("-r{PAR2_REDUNDANCY}");
    let result = cap::process::run_in(directory, "par2", &["create", &redundancy, "-q", name])
        .map_err(|error| CryptoError(format!("failed to run par2: {error}")))?;
    if !result.success {
        return Err(CryptoError(format!(
            "par2 create failed: {}",
            result.stderr.trim()
        )));
    }
    Ok(Some(path.with_file_name(format!("{name}.par2"))))
}

/// Verify a par2 set. Errors only if par2 is missing; a failed check is
/// reported as `false` so the caller can present it alongside other
/// verification findings.
pub fn par2_verify(index: &Path) -> Result<bool, CryptoError> {
    if !cap::process::which("par2") {
        return Err(CryptoError(
            "par2 is not installed; cannot verify recovery data".to_owned(),
        ));
    }
    let directory = index.parent().unwrap_or(Path::new("."));
    let name = index
        .file_name()
        .and_then(|name| name.to_str())
        .ok_or_else(|| CryptoError(format!("unusable filename: {}", index.display())))?;
    let result = cap::process::run_in(directory, "par2", &["verify", "-q", name])
        .map_err(|error| CryptoError(format!("failed to run par2: {error}")))?;
    Ok(result.success)
}

#[cfg(test)]
mod tests {
    use super::{
        age_decrypt, age_encrypt, bytes_to_identity, combine_shares, generate_identity,
        identity_to_bytes, recipient_for_identity, sha256_file, sha256_hex, split_identity,
    };
    use crate::cap;

    fn age_available() -> bool {
        cap::process::which("age") && cap::process::which("age-keygen")
    }

    #[test]
    fn sha256_matches_known_vectors() {
        assert_eq!(
            sha256_hex(b""),
            "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855"
        );
        assert_eq!(
            sha256_hex(b"abc"),
            "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad"
        );
    }

    #[test]
    fn hashes_a_file_by_streaming_it() {
        let temp = tempfile::TempDir::new().expect("temp dir");
        let path = temp.path().join("data.bin");
        cap::fs::write_bytes(&path, b"abc").expect("write");
        assert_eq!(
            sha256_file(&path).expect("hash"),
            "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad"
        );
    }

    #[test]
    fn identity_bytes_round_trip() {
        // A real age identity, used here purely as an encoding fixture.
        let identity = "AGE-SECRET-KEY-1ZMKGL8XP7KFN4L9U2Z8V7PMW5EH009E5HFWX7XZSJTSZN0GXMHCSSCHF28";
        let bytes = identity_to_bytes(identity).expect("decodes");
        assert_eq!(bytes.len(), 32);
        assert_eq!(bytes_to_identity(&bytes).expect("re-encodes"), identity);
    }

    #[test]
    fn rejects_a_non_identity_string() {
        assert!(identity_to_bytes("not-a-key").is_err());
        assert!(bytes_to_identity(&[0_u8; 8]).is_err());
    }

    #[test]
    fn splitting_and_recombining_restores_the_identity() {
        if !age_available() {
            return;
        }
        let identity = generate_identity().expect("keygen");
        let shares = split_identity(&identity.identity, 3, 5).expect("split");
        assert_eq!(shares.len(), 5);

        let recovered = combine_shares(&shares[..3]).expect("combine");
        assert_eq!(recovered, identity.identity);

        // Any threshold-sized subset works, not just the first one.
        let alternative = vec![
            shares.get(1).cloned().unwrap_or_default(),
            shares.get(3).cloned().unwrap_or_default(),
            shares.get(4).cloned().unwrap_or_default(),
        ];
        assert_eq!(
            combine_shares(&alternative).expect("combine subset"),
            identity.identity
        );
    }

    #[test]
    fn too_few_shares_cannot_reconstruct() {
        if !age_available() {
            return;
        }
        let identity = generate_identity().expect("keygen");
        let shares = split_identity(&identity.identity, 3, 5).expect("split");
        assert!(combine_shares(&shares[..2]).is_err());
    }

    #[test]
    fn rejects_an_impossible_threshold() {
        if !age_available() {
            return;
        }
        let identity = generate_identity().expect("keygen");
        assert!(split_identity(&identity.identity, 5, 3).is_err());
        assert!(split_identity(&identity.identity, 0, 3).is_err());
    }

    #[test]
    fn derives_the_same_recipient_as_keygen() {
        if !age_available() {
            return;
        }
        let identity = generate_identity().expect("keygen");
        assert_eq!(
            recipient_for_identity(&identity.identity).expect("derive"),
            identity.recipient
        );
    }

    #[test]
    fn encrypts_and_decrypts_a_round_trip() {
        if !age_available() {
            return;
        }
        let temp = tempfile::TempDir::new().expect("temp dir");
        let identity = generate_identity().expect("keygen");

        let plaintext = temp.path().join("hello.txt");
        cap::fs::write(&plaintext, "hello world").expect("write");
        let encrypted = temp.path().join("hello.txt.age");
        age_encrypt(&plaintext, &identity.recipient, &encrypted).expect("encrypt");

        // The ciphertext must not contain the plaintext.
        let raw = cap::fs::read(&encrypted).expect("read");
        assert!(!raw.windows(11).any(|window| window == b"hello world"));

        let decrypted = temp.path().join("out.txt");
        age_decrypt(&encrypted, &identity.identity, &decrypted).expect("decrypt");
        assert_eq!(
            cap::fs::read_to_string(&decrypted).expect("read"),
            "hello world"
        );
    }

    #[test]
    fn decryption_fails_with_the_wrong_key() {
        if !age_available() {
            return;
        }
        let temp = tempfile::TempDir::new().expect("temp dir");
        let right = generate_identity().expect("keygen");
        let wrong = generate_identity().expect("keygen");

        let plaintext = temp.path().join("secret.txt");
        cap::fs::write(&plaintext, "for your eyes only").expect("write");
        let encrypted = temp.path().join("secret.txt.age");
        age_encrypt(&plaintext, &right.recipient, &encrypted).expect("encrypt");

        let out = temp.path().join("out.txt");
        assert!(age_decrypt(&encrypted, &wrong.identity, &out).is_err());
    }
}
