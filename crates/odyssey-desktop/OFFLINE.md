# Odyssey Desktop in offline mode

For machines that must not reach the internet. With offline mode on, Odyssey Desktop opens
**no** connection to the internet: firmware comes from a folder on disk, and the camera
software is not downloaded. Its own traffic between the app and the hub is on this machine
(localhost) only.

## What Odyssey connects to when offline mode is off

| What | Where | When |
| --- | --- | --- |
| Firmware manifest | `raw.githubusercontent.com/odysseyarm/odyssey/master/firmware/manifests/` | at start, and again when the firmware channel changes |
| Firmware images | `github.com/odysseyarm/odyssey/releases/download/` | when you update a device |
| Orbbec camera software (online installer only) | `github.com/orbbec/OrbbecSDK_v2/releases/download/` and `raw.githubusercontent.com/orbbec/OrbbecSDK_v2/` | only when you press the download button |

Links in the app (for example to a newer Odyssey) open in your browser and are not shown in
offline mode. The web view Odyssey uses (Microsoft Edge WebView2) is a separate Windows
component with its own update behaviour; manage it with your usual Windows policies.

## Turning offline mode on

Any one of these turns it on, and **nothing turns it off**: a stale file cannot undo a registry
setting, and the other way round. A policy file that cannot be read or understood also turns it
on (and the home screen says why).

* **At install time (Windows).** `msiexec /i Odyssey-<version>-win-x64.msi OFFLINE_MODE=1`.
  The offline installer (`...-win-x64-offline.msi`) has it on by default and also carries Orbbec's
  camera software, so nothing needs downloading. `OFFLINE_MODE=0` leaves it off.
* **Registry (Windows; Group Policy or Intune).** `HKLM\SOFTWARE\Policies\OdysseyArm\Odyssey`
  or `HKLM\SOFTWARE\OdysseyArm\Odyssey`: `OfflineMode` (DWORD) = `1`.
* **Policy file (every platform).** `policy.json` in the machine-wide folder; it may contain
  comments:

  ```json
  { "offline": true, "firmware_pack_dir": "D:\\odyssey-firmware-pack" }
  ```

  | OS | Path |
  | --- | --- |
  | Windows | `%ProgramData%\OdysseyArm\Odyssey\policy.json` |
  | macOS | `/Library/Application Support/OdysseyArm/Odyssey/policy.json` |
  | Linux | `/etc/odyssey/policy.json` |

* **Environment.** `ODYSSEY_OFFLINE=1`. The environment can only turn offline mode on.

Changes take effect the next time Odyssey starts.

## The firmware pack

In offline mode Odyssey reads firmware from a **firmware pack**: a folder laid out exactly like
GitHub, so nothing has to be named or placed by hand.

```
releases.stable.json                          the manifest (and releases.alpha.json)
atslite1-v0.1.22/app-a.bin                    an application release carries both slots
atslite1-v0.1.22/app-b.bin
atslite1-netcore-v1/net-core.bin              net core and bootloader releases a manifest
atslite1-bootloader-v4/stage2-bootloader.bin  entry refers to
```

Odyssey offers a firmware release only when **every file it lists is in the folder at its listed
size, both application slots included** (an update writes whichever slot the device is not
running). A missing or half-copied file means that release is left out, and the log says which
file; it never starts an update that cannot finish. Each file is also checked against its
hash when it is used.

Where the folder is, in order: `ODYSSEY_FIRMWARE_PACK_DIR`, the registry value `FirmwarePackDir`,
`firmware_pack_dir` in the policy file, then the default:

| OS | Default folder |
| --- | --- |
| Windows | `%ProgramData%\OdysseyArm\Odyssey\firmware-pack` (the offline installer creates it) |
| macOS | `/Library/Application Support/OdysseyArm/Odyssey/firmware-pack` |
| Linux | `/var/lib/odyssey/firmware-pack` |

Build a pack on a machine with internet access and carry it over:

```
python firmware/manifests/build-pack.py D:\odyssey-firmware-pack --channel stable --latest 2
```

It downloads the newest releases of each device, checks every file's size and hash, and writes
`releases.<channel>.json`, `SHA256SUMS` and a short README. Use `--device` to limit it, and
`--channel stable --channel alpha` for both channels.
