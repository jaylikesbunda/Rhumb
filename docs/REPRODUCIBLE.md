# Verifying a download and rebuilding it

Every release publishes a `SHA256SUMS` file next to the installers. Download it
with the build you want and check the hashes:

    sha256sum --check SHA256SUMS

On Windows, `Get-FileHash` does the same one file at a time:

    Get-FileHash rhumb-0.2.0-windows-portable.zip -Algorithm SHA256

The hash of the file you downloaded must match the line for it in
`SHA256SUMS`.

## Rebuilding

The release workflow builds from the tagged commit with the Rust toolchain the
runner calls `stable`, and sets:

- `SOURCE_DATE_EPOCH` to the commit's timestamp, instead of the build clock.
- `CARGO_INCREMENTAL=0`.
- `Cargo.lock` is committed, so dependency versions are exact.
- `[profile.release]` uses `codegen-units = 1`, `lto = "thin"`,
  `panic = "abort"` and `strip = true`.

To rebuild the executable:

    git checkout v0.2.0
    cargo build --release

## How reproducible this is

The executable is close to reproducible: with the same toolchain, the same
locked dependencies and the same source, the bytes match on repeated builds.
Three things are not yet fully nailed down, and are worth knowing before you
treat a hash mismatch as a problem:

- **The toolchain is not pinned.** `stable` moves; a rebuild months later will
  differ. Pinning an exact version is the next step.
- **Absolute paths are not remapped.** Paths from the build machine can reach
  the binary. This does not change with the source but does vary between
  machines.
- **The installers are not reproducible.** WiX and NSIS embed timestamps and
  their own generated identifiers, so the `.msi` and `setup.exe` will not match
  a rebuild even when the `.exe` inside them does.

So: the checksums are for verifying that a download is the one that was
published, and `cargo build --release` is for reproducing the program. Bit-for-bit
reproduction of the installers is not claimed.
