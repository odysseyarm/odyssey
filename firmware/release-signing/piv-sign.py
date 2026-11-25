#!/usr/bin/env python3
"""ODYSSEY_SIGNER for an Ed25519 key in YubiKey PIV slot 9c.

Reads the 67-byte release message on stdin and writes the 64 raw signature bytes
to stdout. The PIN is read from the terminal, never from arguments or the
environment. Needs `yubikey-manager` (yubikit) and YubiKey firmware 5.7+.
"""

import getpass
import sys

from yubikit.core.smartcard import SmartCardConnection
from yubikit.piv import KEY_TYPE, SLOT, PivSession
from ykman.device import list_all_devices

MESSAGE_LEN = 67


def main() -> int:
    message = sys.stdin.buffer.read()
    if len(message) != MESSAGE_LEN:
        print(
            f"expected {MESSAGE_LEN} message bytes, got {len(message)}", file=sys.stderr
        )
        return 1
    devices = list_all_devices()
    if len(devices) != 1:
        print(f"connect exactly one YubiKey; found {len(devices)}", file=sys.stderr)
        return 1
    device, info = devices[0]
    with device.open_connection(SmartCardConnection) as connection:
        piv = PivSession(connection)
        piv.verify_pin(getpass.getpass(f"PIV PIN for YubiKey {info.serial}: "))
        print("Touch the YubiKey if it blinks.", file=sys.stderr)
        signature = piv.sign(SLOT.SIGNATURE, KEY_TYPE.ED25519, message, None)
    if len(signature) != 64:
        print(f"unexpected signature length {len(signature)}", file=sys.stderr)
        return 1
    sys.stdout.buffer.write(signature)
    return 0


if __name__ == "__main__":
    sys.exit(main())
