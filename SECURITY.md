# Security policy

## Reporting a vulnerability

Please do not open a public issue. Use GitHub's private vulnerability reporting
("Security" tab > "Report a vulnerability") for this repository.

Kaltstellen suspends and terminates processes, so reports about unintended
process termination, privilege handling, or unsafe code in `src/process.rs`
are especially welcome.

## Automated monitoring

- Renovate (`renovate.json`) opens pull requests for dependency and
  GitHub Actions updates; security fixes are raised immediately.
- `cargo-deny` runs on every dependency change and daily against the RustSec
  advisory database (`.github/workflows/security.yml`, policy in `deny.toml`).
- CodeQL scans the Rust code and the workflows (`.github/workflows/codeql.yml`).
- `scripts/check-unsafe.sh` keeps `unsafe` confined to `src/process.rs`.
