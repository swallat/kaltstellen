//! Release metadata from the GitHub API and the files a release must have
//! (spec UP1 "Source", "Release files"; `docs/release-requirements.md`).

use std::collections::BTreeMap;

use serde::Deserialize;

use super::version::Version;

/// The file with the SHA-256 sums in a release.
pub const CHECKSUMS: &str = "checksums.txt";

/// A published version.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Release {
    /// Without the leading `v`.
    pub version: String,
    /// The release page.
    pub page: String,
    /// Download URL by file name.
    pub assets: BTreeMap<String, String>,
}

/// A release as the GitHub API lists it; unknown fields are ignored.
#[derive(Debug, Deserialize)]
struct Raw {
    tag_name: String,
    #[serde(default)]
    html_url: String,
    #[serde(default)]
    draft: bool,
    #[serde(default)]
    assets: Vec<RawAsset>,
}

#[derive(Debug, Deserialize)]
struct RawAsset {
    name: String,
    browser_download_url: String,
}

impl Raw {
    fn release(self) -> Result<Release, String> {
        let version = Version::parse(&self.tag_name)
            .ok_or_else(|| format!("release tag \"{}\" is not a version", self.tag_name))?;
        Ok(Release {
            version: version.to_string(),
            page: self.html_url,
            assets: self
                .assets
                .into_iter()
                .map(|a| (a.name, a.browser_download_url))
                .collect(),
        })
    }
}

/// The answer of `/releases/latest`: one release.
pub fn parse_latest(json: &[u8]) -> Result<Release, String> {
    let raw: Raw = serde_json::from_slice(json).map_err(|e| format!("release information: {e}"))?;
    if raw.draft {
        return Err("the newest release is a draft".to_owned());
    }
    raw.release()
}

/// The answer of `/releases`: the highest version that is not a draft
/// (used while a pre-release runs, so a beta sees the next beta).
pub fn parse_list(json: &[u8]) -> Result<Release, String> {
    let raws: Vec<Raw> =
        serde_json::from_slice(json).map_err(|e| format!("release information: {e}"))?;
    raws.into_iter()
        .filter(|r| !r.draft)
        .filter_map(|r| r.release().ok())
        .max_by(|a, b| {
            let (x, y) = (Version::parse(&a.version), Version::parse(&b.version));
            x.cmp(&y)
        })
        .ok_or_else(|| "no release found".to_owned())
}

/// The executable of `version` for this architecture
/// (`std::env::consts::ARCH`: `x86_64`, `aarch64`).
pub fn asset_name(version: &str, arch: &str) -> String {
    format!("kaltstellen-{version}-windows-{arch}.exe")
}

/// The sum of `name` in the text of `checksums.txt` (`<hex>  <name>` or
/// `<hex> *<name>`), lower case.
pub fn checksum_of(sums: &str, name: &str) -> Result<String, String> {
    for line in sums.lines() {
        let mut fields = line.split_whitespace();
        if let (Some(sum), Some(file), None) = (fields.next(), fields.next(), fields.next())
            && file.strip_prefix('*').unwrap_or(file) == name
        {
            if sum.len() == 64 && sum.bytes().all(|b| b.is_ascii_hexdigit()) {
                return Ok(sum.to_ascii_lowercase());
            }
            return Err(format!("{CHECKSUMS}: the sum of {name} is not SHA-256"));
        }
    }
    Err(format!("{CHECKSUMS} has no sum for {name}"))
}

#[cfg(test)]
mod tests {
    use super::*;

    const LATEST: &str = r#"{
        "tag_name": "v1.2.0", "html_url": "https://github.com/o/r/releases/tag/v1.2.0",
        "draft": false, "prerelease": false, "name": "ignored", "body": "notes",
        "assets": [
            {"name": "kaltstellen-1.2.0-windows-x86_64.exe", "browser_download_url": "https://x/a.exe", "size": 3},
            {"name": "checksums.txt", "browser_download_url": "https://x/checksums.txt"}
        ]
    }"#;

    #[test]
    fn up1_parses_the_github_answer() {
        let release = parse_latest(LATEST.as_bytes()).unwrap();
        assert_eq!(release.version, "1.2.0");
        assert_eq!(release.page, "https://github.com/o/r/releases/tag/v1.2.0");
        assert_eq!(release.assets[CHECKSUMS], "https://x/checksums.txt");
        assert_eq!(
            release.assets[&asset_name("1.2.0", "x86_64")],
            "https://x/a.exe"
        );
        assert!(parse_latest(b"{").is_err());
        assert!(parse_latest(br#"{"tag_name":"nightly"}"#).is_err());
        assert!(parse_latest(br#"{"tag_name":"v1.0.0","draft":true}"#).is_err());
    }

    #[test]
    fn up1_pre_release_list_takes_the_highest_non_draft() {
        let list = r#"[
            {"tag_name": "v1.0.0-beta.9", "draft": false},
            {"tag_name": "v1.0.0-beta.11", "draft": true},
            {"tag_name": "v1.0.0-beta.10", "draft": false},
            {"tag_name": "junk", "draft": false},
            {"tag_name": "v0.9.0", "draft": false}
        ]"#;
        assert_eq!(
            parse_list(list.as_bytes()).unwrap().version,
            "1.0.0-beta.10"
        );
        assert!(parse_list(b"[]").is_err());
    }

    #[test]
    fn up1_checksums_in_sha256sum_format() {
        let a = "a".repeat(64);
        let b = "B".repeat(64);
        let sums =
            format!("{a}  kaltstellen-1.2.0-windows-x86_64.exe\n{b} *checksums-of-b\nnoise\n");
        assert_eq!(
            checksum_of(&sums, "kaltstellen-1.2.0-windows-x86_64.exe").unwrap(),
            a
        );
        assert_eq!(
            checksum_of(&sums, "checksums-of-b").unwrap(),
            "b".repeat(64)
        );
        assert!(checksum_of(&sums, "other.exe").is_err());
        assert!(checksum_of("abc  x.exe", "x.exe").is_err());
    }
}
