#!/usr/bin/env python3
"""Sign Odyssey messages with OpenPGP signature slot 1 on Ledger."""

import argparse
import os
import select
import struct
import sys
import time

CHANNEL = 0x0101
TAG = 0x05
MESSAGE_LEN = 67


def frames(apdu: bytes) -> list[bytes]:
    if len(apdu) > 65535:
        raise ValueError("APDU too large")
    remaining = len(apdu).to_bytes(2, "big") + apdu
    result = []
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
        raise ValueError("Ledger response lacks 7F49")
    public_key = parse_tlv(algorithm).get(0x86)
    if public_key is None or len(public_key) != 32:
        raise ValueError("Ledger 7F49/86 public key is not 32 bytes")
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
    if status != 0x9000:
        raise RuntimeError(
            f"Ledger status {status:04x}; stop, do not automatically retry"
        )
    return data


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


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--device")
    parser.add_argument("--self-test", action="store_true")
    args = parser.parse_args()
    if args.self_test:
        self_test()
        return 0
    if not args.device:
        parser.error("--device /dev/hidrawN required")
    message = sys.stdin.buffer.read(MESSAGE_LEN + 1)
    if len(message) != MESSAGE_LEN:
        print(
            f"expected {MESSAGE_LEN} message bytes, got {len(message)}", file=sys.stderr
        )
        return 1
    fd = os.open(args.device, os.O_RDWR | os.O_NONBLOCK)
    try:
        exchange(fd, bytes.fromhex("00A4040006D27600012401"))
        public_data = exchange(fd, bytes.fromhex("0047810002B60000"))
        print(
            f"OpenPGP slot 1 public key: {extract_public_key(public_data).hex()}",
            file=sys.stderr,
        )
        print("Confirm user PIN on Ledger device.", file=sys.stderr)
        exchange(fd, bytes.fromhex("EF200081"), timeout=180)
        apdu = bytes.fromhex("002A9E9A") + bytes([len(message)]) + message + b"\0"
        signature = exchange(fd, apdu, timeout=180)
    finally:
        os.close(fd)
    if len(signature) != 64:
        raise ValueError(f"expected 64 signature bytes, got {len(signature)}")
    sys.stdout.buffer.write(signature)
    return 0


if __name__ == "__main__":
    sys.exit(main())
