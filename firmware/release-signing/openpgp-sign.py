#!/usr/bin/env python3
"""Sign Odyssey messages with OpenPGP slot 1 over Ledger HID or PC/SC."""

import argparse
from collections.abc import Callable
import getpass
import os
import select
import struct
import sys
import time
import warnings

from cryptography.hazmat.primitives.asymmetric.ed25519 import Ed25519PublicKey

CHANNEL = 0x0101
TAG = 0x05
MESSAGE_LEN = 67


def frames(apdu: bytes) -> list[bytes]:
    if len(apdu) > 65535:
        raise ValueError("APDU too large")
    remaining = len(apdu).to_bytes(2, "big") + apdu
    result: list[bytes] = []
    while remaining:
        sequence = len(result)
        part, remaining = remaining[:59], remaining[59:]
        result.append(
            struct.pack(">HBH", CHANNEL, TAG, sequence) + part.ljust(59, b"\0")
        )
    return result


def parse_tlv(data: bytes) -> dict[int, bytes]:
    result = {}
    pos = 0
    while pos < len(data):
        tag = data[pos]
        pos += 1
        if tag & 0x1F == 0x1F:
            if pos == len(data):
                raise ValueError("truncated TLV tag")
            tag = tag << 8 | data[pos]
            pos += 1
        if pos == len(data):
            raise ValueError("missing TLV length")
        length = data[pos]
        pos += 1
        if length & 0x80:
            count = length & 0x7F
            if count == 0 or count > 2 or pos + count > len(data):
                raise ValueError("invalid TLV length")
            length = int.from_bytes(data[pos : pos + count], "big")
            pos += count
        if pos + length > len(data) or tag in result:
            raise ValueError("truncated value or duplicate TLV")
        result[tag] = data[pos : pos + length]
        pos += length
    return result


def extract_public_key(data: bytes) -> bytes:
    algorithm = parse_tlv(data).get(0x7F49)
    if algorithm is None:
        raise ValueError("OpenPGP response lacks 7F49")
    public_key = parse_tlv(algorithm).get(0x86)
    if public_key is None or len(public_key) != 32:
        raise ValueError("OpenPGP 7F49/86 public key is not 32 bytes")
    return public_key


def response(fd: int, timeout: float = 30) -> tuple[bytes, int]:
    deadline = time.monotonic() + timeout
    output = bytearray()
    sequence = 0
    total = None
    while total is None or len(output) < total:
        remaining = deadline - time.monotonic()
        if remaining <= 0 or not select.select([fd], [], [], remaining)[0]:
            raise TimeoutError(
                "Ledger response timeout; inspect device before retrying"
            )
        packet = os.read(fd, 64)
        if len(packet) < (7 if sequence == 0 else 5):
            raise ValueError("short Ledger frame")
        channel, tag, got_sequence = struct.unpack(">HBH", packet[:5])
        if (channel, tag, got_sequence) != (CHANNEL, TAG, sequence):
            raise ValueError("unexpected Ledger frame")
        if sequence == 0:
            total = int.from_bytes(packet[5:7], "big")
            if total < 2:
                raise ValueError("missing APDU status")
            output.extend(packet[7:])
        else:
            output.extend(packet[5:])
        sequence += 1
    result = bytes(output[:total])
    return result[:-2], int.from_bytes(result[-2:], "big")


def exchange(fd: int, apdu: bytes, timeout: float = 30) -> bytes:
    for frame in frames(apdu):
        if os.write(fd, b"\0" + frame) != 65:
            raise OSError("short HID write")
    data, status = response(fd, timeout)
    return checked_response(data, status)


def checked_response(data: bytes, status: int) -> bytes:
    if status != 0x9000:
        raise RuntimeError(
            f"OpenPGP status {status:04x}; stop, do not automatically retry"
        )
    return data


def pcsc_exchange(
    transmit: Callable[[list[int]], tuple[list[int], int, int]], apdu: bytes
) -> bytes:
    data, sw1, sw2 = transmit(list(apdu))
    return checked_response(bytes(data), sw1 << 8 | sw2)


def pin_apdu(pin: str) -> bytes:
    encoded = pin.encode("utf-8")
    if not 6 <= len(encoded) <= 127:
        raise ValueError("OpenPGP user PIN must be 6 to 127 UTF-8 bytes")
    return bytes.fromhex("00200081") + bytes([len(encoded)]) + encoded


def verify_pcsc_pin(exchange: Callable[[bytes], bytes]) -> None:
    with warnings.catch_warnings():
        warnings.simplefilter("error", getpass.GetPassWarning)
        pin = getpass.getpass("OpenPGP signing PIN: ")
    exchange(pin_apdu(pin))
    print("Touch the key if required by its configured policy.", file=sys.stderr)


def verified_signature(public_key: bytes, message: bytes, signature: bytes) -> bytes:
    if len(signature) != 64:
        raise ValueError(f"expected 64 signature bytes, got {len(signature)}")
    Ed25519PublicKey.from_public_bytes(public_key).verify(signature, message)
    return signature


def sign(
    message: bytes,
    exchange: Callable[[bytes], bytes],
    verify_pin: Callable[[], None],
) -> bytes:
    exchange(bytes.fromhex("00A4040006D27600012401"))
    public_key = extract_public_key(exchange(bytes.fromhex("0047810002B60000")))
    print(f"OpenPGP slot 1 public key: {public_key.hex()}", file=sys.stderr)
    verify_pin()
    apdu = bytes.fromhex("002A9E9A") + bytes([len(message)]) + message + b"\0"
    return verified_signature(public_key, message, exchange(apdu))


def self_test() -> None:
    apdu = bytes(range(MESSAGE_LEN))
    encoded = frames(apdu)
    assert all(len(frame) == 64 for frame in encoded)
    assert encoded[0][:5] == struct.pack(">HBH", CHANNEL, TAG, 0)
    assert int.from_bytes(encoded[0][5:7], "big") == len(apdu)
    payload = b"".join(frame[5:] for frame in encoded)
    assert payload[2 : 2 + len(apdu)] == apdu
    pubkey = bytes(range(32))
    tlv = bytes([0x7F, 0x49, 0x22, 0x86, 0x20]) + pubkey
    assert extract_public_key(tlv) == pubkey
    for malformed in (b"\x7f", b"\x7f\x49", b"\x7f\x49\x80", b"\x7f\x49\x01"):
        try:
            parse_tlv(malformed)
        except ValueError:
            pass
        else:
            raise AssertionError("malformed TLV accepted")
    assert pin_apdu("123456") == bytes.fromhex("0020008106") + b"123456"
    for invalid_pin in ("", "12345", "x" * 128):
        try:
            pin_apdu(invalid_pin)
        except ValueError:
            pass
        else:
            raise AssertionError("invalid PIN length accepted")
    for status in (0x63C2, 0x6983, 0x6985, 0x6A82):
        try:
            checked_response(b"", status)
        except RuntimeError:
            pass
        else:
            raise AssertionError("failed APDU accepted")
    from cryptography.exceptions import InvalidSignature
    from cryptography.hazmat.primitives.asymmetric.ed25519 import Ed25519PrivateKey

    key = Ed25519PrivateKey.from_private_bytes(bytes(range(32)))
    public_key = key.public_key().public_bytes_raw()
    signature = key.sign(apdu)
    assert verified_signature(public_key, apdu, signature) == signature
    try:
        verified_signature(public_key, apdu + b"\0", signature)
    except InvalidSignature:
        pass
    else:
        raise AssertionError("signature for different message accepted")


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    transport = parser.add_mutually_exclusive_group()
    transport.add_argument("--device", help="Ledger /dev/hidrawN")
    transport.add_argument(
        "--reader", type=int, help="PC/SC reader index (e.g. Token2)"
    )
    parser.add_argument("--self-test", action="store_true")
    args = parser.parse_args()
    if args.self_test:
        self_test()
        return 0
    if not args.device and args.reader is None:
        parser.error("--device /dev/hidrawN or --reader INDEX required")
    message = sys.stdin.buffer.read(MESSAGE_LEN + 1)
    if len(message) != MESSAGE_LEN:
        print(
            f"expected {MESSAGE_LEN} message bytes, got {len(message)}", file=sys.stderr
        )
        return 1
    if args.device:
        fd = os.open(args.device, os.O_RDWR | os.O_NONBLOCK)
        try:

            def ledger_pin() -> None:
                print("Confirm user PIN on Ledger device.", file=sys.stderr)
                exchange(fd, bytes.fromhex("EF200081"), timeout=180)

            signature = sign(message, lambda apdu: exchange(fd, apdu, 180), ledger_pin)
        finally:
            os.close(fd)
    else:
        from smartcard.System import readers
        from smartcard.scard import SCARD_RESET_CARD, SCARD_SHARE_EXCLUSIVE

        available = readers()
        if not 0 <= args.reader < len(available):
            raise ValueError(f"PC/SC reader index out of range: {args.reader}")
        with available[args.reader].createConnection() as connection:
            connection.connect(mode=SCARD_SHARE_EXCLUSIVE, disposition=SCARD_RESET_CARD)

            def card_exchange(apdu: bytes) -> bytes:
                return pcsc_exchange(connection.transmit, apdu)

            signature = sign(
                message, card_exchange, lambda: verify_pcsc_pin(card_exchange)
            )
    sys.stdout.buffer.write(signature)
    return 0


if __name__ == "__main__":
    sys.exit(main())
