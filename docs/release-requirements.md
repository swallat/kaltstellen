# What a release must contain for the updater

Input for the later CI/CD work; nothing here exists yet. The client side is
described in [spec.md](spec.md) UP1 and implemented in `src/update/`. A
release that follows this page is picked up by every client that has the
updater, without changing the client.

## Tag and version

- Tag `v<version>`, for example `v1.2.0`. `<version>` is
  `MAJOR.MINOR.PATCH`, optionally `-<pre-release>` (`v1.2.0-beta.1`).
- The version in `Cargo.toml` must equal the tag without `v`: the updater
  starts the downloaded file with `--version` and refuses it unless it
  prints exactly `kaltstellen <version>`. The workflow should check this
  before building.
- A tag with `-` is published as a GitHub *pre-release*. `releases/latest`
  leaves pre-releases out, so only clients that run a pre-release see them.
- Drafts are ignored by the updater; publish the release only when all files
  are uploaded (an empty published release looks like "no file for this
  system", `KS802`).

## Build

- Built with the environment variable `KALTSTELLEN_RELEASE=1`. Without it
  the binary is a development build: it never checks by itself and refuses
  to install.
- Release profile, Windows MSVC targets `x86_64-pc-windows-msvc` and, if
  wanted, `aarch64-pc-windows-msvc`.

## Files (release assets)

| File | Content |
| --- | --- |
| `kaltstellen-<version>-windows-x86_64.exe` | The executable itself, not packed. |
| `kaltstellen-<version>-windows-aarch64.exe` | The same for ARM64, if built. |
| `checksums.txt` | One line per file: `<sha256, 64 lower-case hex digits>  <file name>` (two spaces, `sha256sum` format). Must list every executable above. |

The architecture part is Rust's `std::env::consts::ARCH`. Other files (a zip
with licence and readme for people, release notes, signatures, an SBOM,
build attestations) may be added freely; the updater ignores them.

Limits the client applies: the executable at most 64 MiB, `checksums.txt`
at most 64 KiB, metadata at most 4 MiB.

## Compatibility between versions

- A new version must read the `state.json`, `seen.json`, `settings.json` and
  `config.toml` of the previous one: the update keeps them as they are.
- It must keep the pipe request `quit` (protocol version 1), because the
  next update asks the running tray to quit with it.
- It must keep `--version` printing `kaltstellen <version>` on the first
  line, and `tray` answering `status` on its pipe within 15 s of starting.

## Optional, needs the owner

- **Signing** (spec UP1 "Integrity and authenticity"): either an
  Authenticode certificate to sign the executable, or an own key pair whose
  private half (an Actions secret) signs `checksums.txt` as
  `checksums.txt.sig`. The client does not check either yet; adding the
  file now changes nothing for existing clients.
- foresprint additionally publishes a cosign bundle of `checksums.txt` and a
  GitHub build attestation per file; both can be added here the same way.

## Example

```text
v1.2.0
  kaltstellen-1.2.0-windows-x86_64.exe
  kaltstellen-1.2.0-windows-aarch64.exe
  checksums.txt
  kaltstellen-1.2.0-windows-x86_64.zip   (optional, for people)
```

```text
3f1c…e9a0  kaltstellen-1.2.0-windows-x86_64.exe
7b22…41dd  kaltstellen-1.2.0-windows-aarch64.exe
```
