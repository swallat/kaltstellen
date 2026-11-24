//! H3: a likely kind per program and the mildest action that fits it.
//! Local data only: a list of names shipped with the tool (like the warn
//! list, P6), the description of the file (M2) and the folder. The result is
//! a guess and is shown as one; a program that cannot be placed gets no
//! suggestion.

use crate::config::Action;
use crate::protection::{self, Protection};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    Browser,
    Mail,
    Chat,
    CloudSync,
    Updater,
    MediaPlayer,
    Launcher,
    Game,
    Editor,
    PasswordManager,
    /// Security, remote access, drivers, shells: the warn list (P6).
    Sensitive,
    /// Lives in a Windows folder.
    System,
    Unknown,
}

impl Kind {
    /// With its article, for `probably …`.
    pub fn label(self) -> &'static str {
        match self {
            Kind::Browser => "a browser",
            Kind::Mail => "a mail program",
            Kind::Chat => "a chat program",
            Kind::CloudSync => "a cloud sync program",
            Kind::Updater => "an updater or helper",
            Kind::MediaPlayer => "a media player",
            Kind::Launcher => "a launcher or store client",
            Kind::Game => "a game",
            Kind::Editor => "an editor, office or development tool",
            Kind::PasswordManager => "a password manager",
            Kind::Sensitive => "a security, remote-access or developer tool",
            Kind::System => "part of Windows",
            Kind::Unknown => "a program the tool cannot place",
        }
    }

    /// The mildest action that reaches the purpose, and why. `None`: the
    /// tool does not suggest anything.
    pub fn suggestion(self) -> Option<(Action, &'static str)> {
        match self {
            Kind::Browser | Kind::Mail | Kind::Chat => Some((
                Action::Suspend,
                "state and sign-in stay, nothing is lost; notifications wait",
            )),
            Kind::CloudSync => Some((
                Action::Suspend,
                "syncing resumes later; a long freeze can expire a token",
            )),
            Kind::Updater => Some((Action::Stop, "the program starts it again when it needs it")),
            Kind::MediaPlayer => Some((
                Action::Stop,
                "a frozen audio stream can glitch, closing it is cheap",
            )),
            Kind::Launcher | Kind::Game => {
                Some((Action::Ignore, "a game may need it, or be in use"))
            }
            Kind::Editor => Some((Action::Ignore, "may hold unsaved work")),
            Kind::PasswordManager => Some((Action::Ignore, "you may need it any moment")),
            Kind::Sensitive | Kind::System | Kind::Unknown => None,
        }
    }
}

/// How the kind was found, shown with the guess.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Source {
    Name,
    Description,
    Folder,
    Protection,
    None,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Guess {
    pub kind: Kind,
    pub source: Source,
}

/// Names shipped with the tool, without `.exe`, lower case.
const NAMES: &[(Kind, &[&str])] = &[
    (
        Kind::Browser,
        &[
            "chrome",
            "firefox",
            "msedge",
            "brave",
            "opera",
            "vivaldi",
            "iexplore",
            "arc",
            "librewolf",
            "zen",
        ],
    ),
    (
        Kind::Mail,
        &[
            "outlook",
            "olk",
            "hxoutlook",
            "thunderbird",
            "mailspring",
            "em_client",
            "mailbird",
        ],
    ),
    (
        Kind::Chat,
        &[
            "slack", "teams", "ms-teams", "discord", "telegram", "whatsapp", "signal", "skype",
            "element", "zoom", "webex",
        ],
    ),
    (
        Kind::CloudSync,
        &[
            "onedrive",
            "dropbox",
            "googledrivefs",
            "nextcloud",
            "megasync",
            "icloudservices",
        ],
    ),
    (
        Kind::MediaPlayer,
        &[
            "spotify",
            "vlc",
            "itunes",
            "musicbee",
            "foobar2000",
            "wmplayer",
            "winamp",
            "tidal",
            "applemusic",
            "mpv",
        ],
    ),
    (
        Kind::Launcher,
        &[
            "steam",
            "epicgameslauncher",
            "galaxyclient",
            "battle.net",
            "origin",
            "eadesktop",
            "upc",
            "ubisoftconnect",
            "riotclientservices",
            "gogalaxy",
        ],
    ),
    (
        Kind::Editor,
        &[
            "code",
            "devenv",
            "idea64",
            "pycharm64",
            "rider64",
            "clion64",
            "webstorm64",
            "goland64",
            "studio64",
            "notepad++",
            "sublime_text",
            "winword",
            "excel",
            "powerpnt",
            "onenote",
            "obsidian",
            "typora",
            "notepad",
        ],
    ),
    (
        Kind::PasswordManager,
        &[
            "keepass",
            "keepassxc",
            "1password",
            "bitwarden",
            "dashlane",
            "lastpass",
        ],
    ),
];

fn stem(exe: &str) -> String {
    let lower = exe.trim().to_lowercase();
    lower
        .strip_suffix(".exe")
        .map_or(lower.clone(), str::to_owned)
}

fn by_name(exe: &str) -> Option<Kind> {
    let stem = stem(exe);
    NAMES
        .iter()
        .find(|(_, names)| names.contains(&stem.as_str()))
        .map(|(kind, _)| *kind)
        .or_else(|| stem.contains("updat").then_some(Kind::Updater))
}

/// Words of the file description (M2) that point to a kind.
fn by_description(description: &str) -> Option<Kind> {
    let text = description.to_lowercase();
    let has = |words: &[&str]| words.iter().any(|w| text.contains(w));
    if has(&["update", "updater"]) {
        Some(Kind::Updater)
    } else if has(&["sync"]) {
        Some(Kind::CloudSync)
    } else if has(&["launcher", "game client", "store client"]) {
        Some(Kind::Launcher)
    } else if has(&["web browser", "browser"]) {
        Some(Kind::Browser)
    } else if has(&["messenger", "chat"]) {
        Some(Kind::Chat)
    } else if has(&["e-mail", "email", "mail client"]) {
        Some(Kind::Mail)
    } else if has(&["media player", "music player", "audio player"]) {
        Some(Kind::MediaPlayer)
    } else if has(&["password"]) {
        Some(Kind::PasswordManager)
    } else {
        None
    }
}

fn by_folder(path: &str) -> Option<Kind> {
    let lower = path.replace('/', "\\").to_lowercase();
    let has = |part: &str| lower.contains(part);
    if has("\\steamapps\\") || has("\\epic games\\") || has("\\xboxgames\\") || has("\\games\\") {
        Some(Kind::Game)
    } else if lower.starts_with("c:\\windows\\") {
        Some(Kind::System)
    } else {
        None
    }
}

/// The kind of a program: the warn list first (P6), then the names, then
/// the file description, then the folder.
pub fn classify(exe: &str, path: Option<&str>, description: Option<&str>) -> Guess {
    if matches!(protection::of(exe), Protection::Warn(_) | Protection::Hard) {
        return Guess {
            kind: Kind::Sensitive,
            source: Source::Protection,
        };
    }
    if let Some(kind) = by_name(exe) {
        return Guess {
            kind,
            source: Source::Name,
        };
    }
    if let Some(kind) = description.and_then(by_description) {
        return Guess {
            kind,
            source: Source::Description,
        };
    }
    if let Some(kind) = path.and_then(by_folder) {
        return Guess {
            kind,
            source: Source::Folder,
        };
    }
    Guess {
        kind: Kind::Unknown,
        source: Source::None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn h3_names_decide_first() {
        assert_eq!(classify("Chrome.exe", None, None).kind, Kind::Browser);
        assert_eq!(classify("slack", None, None).kind, Kind::Chat);
        assert_eq!(classify("GoogleUpdate.exe", None, None).kind, Kind::Updater);
        assert_eq!(classify("spotify.exe", None, None).kind, Kind::MediaPlayer);
        assert_eq!(classify("steam.exe", None, None).kind, Kind::Launcher);
        assert_eq!(
            classify("keepassxc.exe", None, None).kind,
            Kind::PasswordManager
        );
    }

    #[test]
    fn h3_suggestions_follow_the_table() {
        let action = |k: Kind| k.suggestion().map(|(a, _)| a);
        assert_eq!(action(Kind::Browser), Some(Action::Suspend));
        assert_eq!(action(Kind::CloudSync), Some(Action::Suspend));
        assert_eq!(action(Kind::Updater), Some(Action::Stop));
        assert_eq!(action(Kind::MediaPlayer), Some(Action::Stop));
        assert_eq!(action(Kind::Launcher), Some(Action::Ignore));
        assert_eq!(action(Kind::Editor), Some(Action::Ignore));
        assert_eq!(action(Kind::PasswordManager), Some(Action::Ignore));
        assert_eq!(action(Kind::Sensitive), None);
        assert_eq!(action(Kind::Unknown), None);
    }

    #[test]
    fn h3_the_warn_list_gets_no_suggestion() {
        let guess = classify("anydesk.exe", None, None);
        assert_eq!(guess.kind, Kind::Sensitive);
        assert_eq!(guess.source, Source::Protection);
        assert_eq!(guess.kind.suggestion(), None);
    }

    #[test]
    fn h3_description_and_folder_come_after_the_name() {
        let by_text = classify("x.exe", None, Some("Cloud Sync Client"));
        assert_eq!(
            (by_text.kind, by_text.source),
            (Kind::CloudSync, Source::Description)
        );
        let game = classify(
            "x.exe",
            Some("D:\\SteamLibrary\\steamapps\\common\\x\\x.exe"),
            None,
        );
        assert_eq!((game.kind, game.source), (Kind::Game, Source::Folder));
        // The name wins over the description.
        assert_eq!(
            classify("firefox.exe", None, Some("updater")).kind,
            Kind::Browser
        );
    }

    #[test]
    fn h3_what_cannot_be_placed_gets_no_suggestion() {
        let guess = classify(
            "mystery.exe",
            Some("C:\\tools\\mystery.exe"),
            Some("A thing"),
        );
        assert_eq!(guess.kind, Kind::Unknown);
        assert_eq!(guess.kind.suggestion(), None);
    }
}
