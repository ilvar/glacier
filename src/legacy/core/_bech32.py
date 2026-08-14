"""Minimal Bech32 encode/decode (BIP-173 reference algorithm, public domain).

This is a plain data encoding, not a cryptographic primitive -- it is the
text format `age` uses for its identity strings. We need it to extract and
reinsert the raw 32-byte secret when splitting/recombining identities with
Shamir's Secret Sharing; we do not use it for anything cryptographic.
"""

from __future__ import annotations

CHARSET = "qpzry9x8gf2tvdw0s3jn54khce6mua7l"


def _polymod(values: list[int]) -> int:
    generator = [0x3B6A57B2, 0x26508E6D, 0x1EA119FA, 0x3D4233DD, 0x2A1462B3]
    chk = 1
    for v in values:
        top = chk >> 25
        chk = (chk & 0x1FFFFFF) << 5 ^ v
        for i in range(5):
            chk ^= generator[i] if ((top >> i) & 1) else 0
    return chk


def _hrp_expand(hrp: str) -> list[int]:
    return [ord(x) >> 5 for x in hrp] + [0] + [ord(x) & 31 for x in hrp]


def _create_checksum(hrp: str, data: list[int]) -> list[int]:
    values = _hrp_expand(hrp) + data
    polymod = _polymod(values + [0, 0, 0, 0, 0, 0]) ^ 1
    return [(polymod >> 5 * (5 - i)) & 31 for i in range(6)]


def _verify_checksum(hrp: str, data: list[int]) -> bool:
    return _polymod(_hrp_expand(hrp) + data) == 1


def convertbits(data: bytes, frombits: int, tobits: int, pad: bool = True) -> list[int]:
    acc = 0
    bits = 0
    ret = []
    maxv = (1 << tobits) - 1
    max_acc = (1 << (frombits + tobits - 1)) - 1
    for value in data:
        if value < 0 or (value >> frombits):
            raise ValueError("invalid data for base conversion")
        acc = ((acc << frombits) | value) & max_acc
        bits += frombits
        while bits >= tobits:
            bits -= tobits
            ret.append((acc >> bits) & maxv)
    if pad:
        if bits:
            ret.append((acc << (tobits - bits)) & maxv)
    elif bits >= frombits or ((acc << (tobits - bits)) & maxv):
        raise ValueError("invalid padding in base conversion")
    return ret


def bech32_encode(hrp: str, data: bytes) -> str:
    values = convertbits(data, 8, 5)
    combined = values + _create_checksum(hrp, values)
    return hrp + "1" + "".join(CHARSET[d] for d in combined)


def bech32_decode(bech: str) -> tuple[str, bytes]:
    if bech != bech.lower() and bech != bech.upper():
        raise ValueError("mixed case string")
    bech = bech.lower()
    pos = bech.rfind("1")
    if pos < 1 or pos + 7 > len(bech):
        raise ValueError("invalid bech32 separator position")
    hrp = bech[:pos]
    data_part = bech[pos + 1 :]
    if not all(c in CHARSET for c in data_part):
        raise ValueError("invalid bech32 character")
    data = [CHARSET.index(c) for c in data_part]
    if not _verify_checksum(hrp, data):
        raise ValueError("invalid bech32 checksum")
    payload = convertbits(data[:-6], 5, 8, pad=False)
    return hrp, bytes(payload)
