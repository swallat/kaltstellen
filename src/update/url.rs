//! Which URLs the updater may fetch (spec UP1): `https://` anywhere, plain
//! `http://` only on a loopback host (the tests' local server).

/// A URL split into what WinHTTP needs.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Url {
    pub secure: bool,
    pub host: String,
    pub port: u16,
    /// Path and query, starting with `/`.
    pub path: String,
}

impl Url {
    /// Parses `scheme://host[:port][/path]`. User info, fragments and
    /// anything but `http`/`https` are refused, as is `http` to a host that
    /// is not loopback.
    pub fn parse(text: &str) -> Result<Url, String> {
        let bad = |why: &str| format!("{text}: {why}");
        let (secure, rest) = if let Some(rest) = text.strip_prefix("https://") {
            (true, rest)
        } else if let Some(rest) = text.strip_prefix("http://") {
            (false, rest)
        } else {
            return Err(bad("only https:// addresses are used"));
        };
        let (authority, path) = match rest.find('/') {
            Some(i) => (&rest[..i], &rest[i..]),
            None => (rest, "/"),
        };
        if authority.contains('@') || path.contains('#') {
            return Err(bad("user names and fragments are not allowed"));
        }
        if path.chars().any(|c| c.is_whitespace() || c.is_control()) {
            return Err(bad("spaces or control characters in the path"));
        }
        let (host, port) = match authority.rsplit_once(':') {
            Some((host, port)) if !host.ends_with(']') || authority.starts_with('[') => {
                let port: u16 = port.parse().map_err(|_| bad("invalid port"))?;
                (host, port)
            }
            _ => (authority, if secure { 443 } else { 80 }),
        };
        if host.is_empty() || port == 0 {
            return Err(bad("no host"));
        }
        if !host
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '-' | '[' | ']' | ':'))
        {
            return Err(bad("invalid host name"));
        }
        if !secure && !is_loopback(host) {
            return Err(bad(
                "plain http is allowed only on this computer (loopback)",
            ));
        }
        Ok(Url {
            secure,
            host: host.to_owned(),
            port,
            path: path.to_owned(),
        })
    }
}

fn is_loopback(host: &str) -> bool {
    host.eq_ignore_ascii_case("localhost")
        || host == "[::1]"
        // 127.0.0.0/8 as four numeric octets; `127.example.com.x` is a
        // host name, not loopback.
        || host
            .parse::<std::net::Ipv4Addr>()
            .is_ok_and(|ip| ip.is_loopback())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn up1_https_anywhere_http_only_on_loopback() {
        let url = Url::parse("https://api.github.com/repos/a/b/releases/latest").unwrap();
        assert_eq!(
            url,
            Url {
                secure: true,
                host: "api.github.com".into(),
                port: 443,
                path: "/repos/a/b/releases/latest".into()
            }
        );
        let local = Url::parse("http://127.0.0.1:50123/x?y=1").unwrap();
        assert_eq!((local.secure, local.port), (false, 50123));
        assert_eq!(local.path, "/x?y=1");
        assert!(Url::parse("http://localhost/").is_ok());
        assert!(Url::parse("http://[::1]:8080/").is_ok());
        assert_eq!(Url::parse("https://example.org").unwrap().path, "/");
        for bad in [
            "http://example.org/x",
            "http://127.0.0.1.example.org/x",
            "http://127.example.com.x/x",
            "http://127.a.b.c/x",
            "ftp://example.org/x",
            "file:///C:/x",
            "https://user@example.org/",
            "https://example.org:0/",
            "https://example.org:99999/",
            "https:///x",
            "https://exa mple.org/",
            "https://example.org/a b",
            "https://example.org/#f",
        ] {
            assert!(Url::parse(bad).is_err(), "{bad}");
        }
    }
}
