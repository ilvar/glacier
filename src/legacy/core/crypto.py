"""Thin wrappers around `age`, `age-keygen`, and SLIP-0039 secret sharing.

No cryptographic primitive is implemented here. `age` does the encryption,
`shamir_mnemonic` does the secret splitting. The only thing this module adds
is the glue to turn an age identity into 32 raw bytes and back (a Bech32
encoding, not a cipher) so that those bytes can be split with SLIP-0039 and
reassembled later.
"""

from __future__ import annotations

import hashlib
import shutil
import subprocess
import tempfile
from dataclasses import dataclass
from pathlib import Path

import shamir_mnemonic as sm

from legacy.core._bech32 import bech32_decode, bech32_encode

IDENTITY_HRP = "age-secret-key-"


class CryptoError(RuntimeError):
    pass


def _require(binary: str) -> str:
    path = shutil.which(binary)
    if path is None:
        raise CryptoError(
            f"required tool {binary!r} was not found on PATH. "
            f"Install it before running this command."
        )
    return path


@dataclass(frozen=True)
class Identity:
    identity: str  # "AGE-SECRET-KEY-1..."
    recipient: str  # "age1..."

    @property
    def sha256(self) -> str:
        return hashlib.sha256(self.identity.encode("ascii")).hexdigest()


def generate_identity() -> Identity:
    """Generate a fresh age identity via `age-keygen`."""
    age_keygen = _require("age-keygen")
    result = subprocess.run(
        [age_keygen], capture_output=True, text=True, check=True
    )
    recipient = None
    identity = None
    for line in result.stdout.splitlines() + result.stderr.splitlines():
        line = line.strip()
        if line.startswith("Public key:"):
            recipient = line.split(":", 1)[1].strip()
        elif line.startswith("AGE-SECRET-KEY-"):
            identity = line
    if identity is None or recipient is None:
        raise CryptoError(f"could not parse age-keygen output:\n{result.stdout}\n{result.stderr}")
    return Identity(identity=identity, recipient=recipient)


def recipient_for_identity(identity: str) -> str:
    """Derive the recipient (public key) for an identity string via age-keygen -y."""
    age_keygen = _require("age-keygen")
    with tempfile.NamedTemporaryFile("w", suffix=".txt", delete=False) as f:
        f.write(identity.strip() + "\n")
        identity_path = f.name
    try:
        result = subprocess.run(
            [age_keygen, "-y", identity_path], capture_output=True, text=True, check=True
        )
        return result.stdout.strip()
    finally:
        Path(identity_path).unlink(missing_ok=True)


def identity_to_bytes(identity: str) -> bytes:
    hrp, payload = bech32_decode(identity.strip())
    if hrp != IDENTITY_HRP:
        raise CryptoError(f"not an age identity (unexpected prefix {hrp!r})")
    if len(payload) != 32:
        raise CryptoError(f"unexpected age identity length: {len(payload)} bytes")
    return payload


def bytes_to_identity(data: bytes) -> str:
    if len(data) != 32:
        raise CryptoError(f"expected 32 bytes, got {len(data)}")
    return bech32_encode(IDENTITY_HRP, data).upper()


def split_identity(identity: str, threshold: int, shares: int) -> list[str]:
    """Split an age identity into `shares` SLIP-0039 mnemonics, `threshold` needed."""
    secret = identity_to_bytes(identity)
    groups = sm.generate_mnemonics(1, [(threshold, shares)], secret)
    return groups[0]


def combine_shares(mnemonics: list[str]) -> str:
    """Reconstruct an age identity string from SLIP-0039 mnemonic shares."""
    try:
        secret = sm.combine_mnemonics(mnemonics)
    except Exception as e:  # shamir_mnemonic raises its own MnemonicError
        raise CryptoError(f"could not reconstruct key from shares: {e}") from e
    return bytes_to_identity(secret)


def age_encrypt(plaintext_path: Path, recipient: str, out_path: Path) -> None:
    age = _require("age")
    subprocess.run(
        [age, "-r", recipient, "-o", str(out_path), str(plaintext_path)],
        check=True,
        capture_output=True,
    )


def age_decrypt(ciphertext_path: Path, identity: str, out_path: Path) -> None:
    age = _require("age")
    with tempfile.NamedTemporaryFile("w", suffix=".txt", delete=False) as f:
        f.write(identity.strip() + "\n")
        identity_path = f.name
    try:
        subprocess.run(
            [age, "-d", "-i", identity_path, "-o", str(out_path), str(ciphertext_path)],
            check=True,
            capture_output=True,
        )
    finally:
        Path(identity_path).unlink(missing_ok=True)


def sha256_file(path: Path) -> str:
    h = hashlib.sha256()
    with open(path, "rb") as f:
        for chunk in iter(lambda: f.read(1024 * 1024), b""):
            h.update(chunk)
    return h.hexdigest()


def par2_create(path: Path, redundancy_percent: int = 15) -> Path | None:
    """Create par2 recovery files next to `path`. Returns the .par2 index path, or
    None if par2 is not installed (caller should warn, not fail)."""
    par2 = shutil.which("par2")
    if par2 is None:
        return None
    subprocess.run(
        [par2, "create", f"-r{redundancy_percent}", "-q", str(path)],
        check=True,
        capture_output=True,
        cwd=str(path.parent),
    )
    return path.with_suffix(path.suffix + ".par2")


def par2_verify(par2_index: Path) -> bool:
    par2 = shutil.which("par2")
    if par2 is None:
        raise CryptoError("par2 is not installed; cannot verify recovery data")
    result = subprocess.run(
        ["par2", "verify", "-q", str(par2_index)],
        capture_output=True,
        text=True,
        cwd=str(par2_index.parent),
    )
    return result.returncode == 0
