//! Version numbers of releases and their order (spec UP1 "Versions"),
//! carried over from foresprint's `selfupdate.Newer`.

use std::cmp::Ordering;

/// `MAJOR.MINOR.PATCH[-PRE][+BUILD]`, a leading `v` allowed; build
/// metadata is ignored.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Version {
    pub numbers: [u64; 3],
    /// Pre-release label without the `-`; empty for a release.
    pub pre: String,
}

impl Version {
    pub fn parse(text: &str) -> Option<Version> {
        let text = text.trim();
        let text = text.strip_prefix('v').unwrap_or(text);
        let text = text.split_once('+').map_or(text, |(core, _)| core);
        let (core, pre) = text.split_once('-').unwrap_or((text, ""));
        if text.contains('-') && pre.is_empty() {
            return None;
        }
        let mut numbers = [0u64; 3];
        let mut parts = core.split('.');
        for slot in &mut numbers {
            let part = parts.next()?;
            if part.is_empty() || !part.bytes().all(|b| b.is_ascii_digit()) {
                return None;
            }
            *slot = part.parse().ok()?;
        }
        if parts.next().is_some() {
            return None;
        }
        Some(Version {
            numbers,
            pre: pre.to_owned(),
        })
    }

    pub fn is_pre_release(&self) -> bool {
        !self.pre.is_empty()
    }
}

impl std::fmt::Display for Version {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let [a, b, c] = self.numbers;
        write!(f, "{a}.{b}.{c}")?;
        if !self.pre.is_empty() {
            write!(f, "-{}", self.pre)?;
        }
        Ok(())
    }
}

impl PartialOrd for Version {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

impl Ord for Version {
    fn cmp(&self, other: &Self) -> Ordering {
        self.numbers.cmp(&other.numbers).then_with(|| {
            match (self.pre.is_empty(), other.pre.is_empty()) {
                (true, true) => Ordering::Equal,
                // 1.0.0-beta.1 < 1.0.0
                (true, false) => Ordering::Greater,
                (false, true) => Ordering::Less,
                (false, false) => compare_pre(&self.pre, &other.pre),
            }
        })
    }
}

/// Dot by dot; numbers as numbers and below words, so `beta.10` is above
/// `beta.9` and `rc` above `beta`.
fn compare_pre(a: &str, b: &str) -> Ordering {
    let (mut x, mut y) = (a.split('.'), b.split('.'));
    loop {
        match (x.next(), y.next()) {
            (None, None) => return Ordering::Equal,
            (None, Some(_)) => return Ordering::Less,
            (Some(_), None) => return Ordering::Greater,
            (Some(p), Some(q)) => {
                let order = match (p.parse::<u64>(), q.parse::<u64>()) {
                    (Ok(m), Ok(n)) => m.cmp(&n),
                    (Ok(_), Err(_)) => Ordering::Less,
                    (Err(_), Ok(_)) => Ordering::Greater,
                    (Err(_), Err(_)) => p.cmp(q),
                };
                if order != Ordering::Equal {
                    return order;
                }
            }
        }
    }
}

/// Whether `latest` is a higher version than `current`. Anything that is
/// not a version is never newer and never older.
pub fn newer(current: &str, latest: &str) -> bool {
    match (Version::parse(current), Version::parse(latest)) {
        (Some(c), Some(l)) => l > c,
        _ => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn up1_order_follows_foresprint() {
        for (current, latest, want) in [
            ("1.0.0", "1.0.1", true),
            ("1.0.1", "1.0.0", false),
            ("1.0.0", "1.0.0", false),
            ("1.9.0", "1.10.0", true),
            ("1.0.0-beta.1", "1.0.0", true),
            ("1.0.0", "1.0.0-beta.2", false),
            ("v1.0.0", "v1.1.0", true),
            ("1.0.0-beta.9", "1.0.0-beta.10", true),
            ("1.0.0-beta.10", "1.0.0-beta.9", false),
            ("1.0.0-beta.2", "1.0.0-rc.1", true),
            ("1.0.0-beta", "1.0.0-beta.1", true),
            ("1.0.0-1", "1.0.0-alpha", true),
            ("0.1.0", "0.1.0+build.7", false),
            ("dev", "9.9.9", false),
            ("1.0.0", "weird", false),
            ("2.0.0", "10.0.0", true),
        ] {
            assert_eq!(newer(current, latest), want, "{current} -> {latest}");
        }
    }

    #[test]
    fn up1_parse_rejects_what_is_not_a_version() {
        for bad in [
            "", "1", "1.2", "1.2.3.4", "1.a.3", "1.2.3-", "-1.2.3", "1..3", "+1.2.3",
        ] {
            assert_eq!(Version::parse(bad), None, "{bad:?}");
        }
        let v = Version::parse(" v1.2.3-rc.1+abc ").unwrap();
        assert_eq!(v.numbers, [1, 2, 3]);
        assert_eq!(v.pre, "rc.1");
        assert!(v.is_pre_release());
        assert_eq!(v.to_string(), "1.2.3-rc.1");
    }
}
