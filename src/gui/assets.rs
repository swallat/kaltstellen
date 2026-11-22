//! The web page of the main window, embedded at compile time from `ui/`.
//! `ui/js/mock.js` (the browser-only mock backend) is not embedded.

const FILES: &[(&str, &[u8], &str)] = &[
    (
        "index.html",
        include_bytes!("../../ui/index.html"),
        "text/html; charset=utf-8",
    ),
    (
        "css/tokens.css",
        include_bytes!("../../ui/css/tokens.css"),
        "text/css; charset=utf-8",
    ),
    (
        "css/app.css",
        include_bytes!("../../ui/css/app.css"),
        "text/css; charset=utf-8",
    ),
    (
        "js/app.js",
        include_bytes!("../../ui/js/app.js"),
        "text/javascript; charset=utf-8",
    ),
    (
        "js/bridge.js",
        include_bytes!("../../ui/js/bridge.js"),
        "text/javascript; charset=utf-8",
    ),
    // The ice cube logo (branding/logos/09-eiswuerfel-warm-avatar.svg).
    (
        "img/logo.svg",
        include_bytes!("../../ui/img/logo.svg"),
        "image/svg+xml",
    ),
];

/// The file at `path` below the page's origin, with its MIME type.
pub fn get(path: &str) -> Option<(&'static [u8], &'static str)> {
    FILES
        .iter()
        .find(|(name, _, _)| *name == path)
        .map(|(_, bytes, mime)| (*bytes, *mime))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn text(path: &str) -> &'static str {
        std::str::from_utf8(get(path).unwrap().0).unwrap()
    }

    #[test]
    fn every_reference_of_the_page_is_embedded_and_nothing_is_remote() {
        let html = text("index.html");
        for reference in ["css/tokens.css", "css/app.css", "js/app.js", "img/logo.svg"] {
            assert!(html.contains(reference), "{reference}");
            assert!(get(reference).is_some(), "{reference} not embedded");
        }
        assert!(text("js/app.js").contains("from './bridge.js'"));
        for (name, bytes, _) in FILES {
            // The SVG namespace is a name, not something loaded.
            let content = std::str::from_utf8(bytes)
                .unwrap()
                .replace("xmlns=\"http://www.w3.org/2000/svg\"", "");
            for remote in ["http://", "https://", "//cdn", "<script>"] {
                assert!(!content.contains(remote), "{name} contains {remote}");
            }
            // No inline handlers or styles: the CSP forbids them.
            assert!(
                !content.contains(" onclick=") && !content.contains(" style="),
                "{name}"
            );
        }
        assert!(get("js/mock.js").is_none(), "the mock is for browsers only");
        assert!(get("../Cargo.toml").is_none());
    }
}
