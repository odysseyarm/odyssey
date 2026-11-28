# Supplementary license notices

`collect-cv-notices.py` reads license files from the exact sources resolved by
Cargo. This directory supplies text for crates whose resolved source omits a
license file.

`eskf-MIT.txt` supplies the MIT permission text and credits Jørgen Nordmoen and
contributors. The fork's manifest and the
[upstream manifest](https://github.com/nordmoen/eskf-rs/blob/master/Cargo.toml)
declare `MIT OR Apache-2.0` and name Jørgen Nordmoen as the author. The original
source contains no copyright year, so this notice does not invent one. Odyssey
uses the MIT option for redistribution. The fork also carries this notice in
`third_party/eskf/LICENSE-MIT` in the private repository.

`convert_case-MIT.txt` supplies the standard MIT permission text and credits
David Purdum, the author named in the
[locked 0.4.0 manifest](https://github.com/rutrum/convert-case/blob/300d9eef9e8c970e15f324afbc3df8b6a4ebbf71/Cargo.toml).
That source declares MIT but supplies neither a license file nor a copyright
year. The notice therefore uses the declared author's name without a year.
It does not copy the dated notice from a different upstream revision.

## Update the bundled notices

Save the output of `cargo metadata --locked --format-version 1` from the public
workspace, `firmware/lite1`, `firmware/legacy-atslite`, and the private
`odyssey-cv` workspace. Run `tools/collect-cv-notices.py` with those four JSON
paths and redirect its output to `THIRD_PARTY_NOTICES.txt`. Keep the public and
private copies of the generated notice identical. Cargo's source cache must
contain the resolved dependencies.

The collector follows normal dependency edges, including target-specific edges
and procedural macros. It excludes development and build-script-only edges.
It preserves supplementary copyright and NOTICE files, libm's per-file
copyright notices, and the additional Unicode license. It stops when a license
expression or a missing notice needs review.

`lambda-twist` uses MPL-2.0. Its notice identifies the public source download for
the exact version. The resolved source must match that download. Any local
changes to MPL-covered files require publishing those changed files under the
MPL and updating the source location before distributing the binary. This does
not require publishing unrelated private Odyssey code.

This notice collection covers the `odyssey-cv` dependency tree. Other Odyssey
dependencies and separately packaged native libraries have their own notice
requirements.
