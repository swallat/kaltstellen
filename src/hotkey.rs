//! Hotkey syntax (spec C7): modifiers plus one key, for example
//! `Ctrl+Shift+Alt+F1`. Parsing only; registration belongs to the tray app.

use std::fmt;

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Hash)]
pub struct Modifiers {
    pub ctrl: bool,
    pub alt: bool,
    pub shift: bool,
    pub win: bool,
}

impl Modifiers {
    fn any(self) -> bool {
        self.ctrl || self.alt || self.shift || self.win
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Key {
    /// `A` to `Z`, stored upper-case.
    Letter(char),
    /// `0` to `9` on the main block.
    Digit(u8),
    /// `F1` to `F24`.
    Function(u8),
    Named(Named),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Named {
    Space,
    Tab,
    Enter,
    Escape,
    Insert,
    Delete,
    Home,
    End,
    PageUp,
    PageDown,
    Left,
    Up,
    Right,
    Down,
    Pause,
}

const NAMED: &[(&str, Named, u32)] = &[
    ("Space", Named::Space, 0x20),
    ("Tab", Named::Tab, 0x09),
    ("Enter", Named::Enter, 0x0D),
    ("Esc", Named::Escape, 0x1B),
    ("Insert", Named::Insert, 0x2D),
    ("Delete", Named::Delete, 0x2E),
    ("Home", Named::Home, 0x24),
    ("End", Named::End, 0x23),
    ("PageUp", Named::PageUp, 0x21),
    ("PageDown", Named::PageDown, 0x22),
    ("Left", Named::Left, 0x25),
    ("Up", Named::Up, 0x26),
    ("Right", Named::Right, 0x27),
    ("Down", Named::Down, 0x28),
    ("Pause", Named::Pause, 0x13),
];

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct Hotkey {
    pub modifiers: Modifiers,
    pub key: Key,
}

impl Hotkey {
    /// Parses `Modifier+...+Key`. Names are case-insensitive. Every key except
    /// a function key needs at least one modifier.
    pub fn parse(text: &str) -> Result<Hotkey, String> {
        let mut parts: Vec<&str> = text.split('+').map(str::trim).collect();
        if parts.iter().any(|p| p.is_empty()) {
            return Err(format!(
                "hotkey \"{text}\" has an empty part; write it like \"Ctrl+Shift+Alt+F1\""
            ));
        }
        let key_name = parts.pop().unwrap_or_default();
        let mut modifiers = Modifiers::default();
        for part in parts {
            let slot = match part.to_ascii_lowercase().as_str() {
                "ctrl" | "control" => &mut modifiers.ctrl,
                "alt" => &mut modifiers.alt,
                "shift" => &mut modifiers.shift,
                "win" => &mut modifiers.win,
                _ => return Err(format!("unknown modifier \"{part}\" in hotkey \"{text}\"")),
            };
            if *slot {
                return Err(format!(
                    "modifier \"{part}\" appears twice in hotkey \"{text}\""
                ));
            }
            *slot = true;
        }
        let key = parse_key(key_name)
            .ok_or_else(|| format!("unknown key \"{key_name}\" in hotkey \"{text}\""))?;
        if !modifiers.any() && !matches!(key, Key::Function(_)) {
            return Err(format!(
                "hotkey \"{text}\" needs at least one modifier (Ctrl, Alt, Shift, Win)"
            ));
        }
        Ok(Hotkey { modifiers, key })
    }

    /// C7: on layouts with AltGr, `Ctrl+Alt+<printable>` types a character
    /// (`{`, `[`, `@`, ...). Registering it would steal that character.
    pub fn steals_altgr(&self) -> bool {
        self.modifiers.ctrl
            && self.modifiers.alt
            && matches!(self.key, Key::Letter(_) | Key::Digit(_))
    }

    /// Windows virtual-key code of the key, for `RegisterHotKey`.
    pub fn virtual_key(&self) -> u32 {
        match self.key {
            Key::Letter(c) => c as u32,
            Key::Digit(d) => 0x30 + u32::from(d),
            Key::Function(n) => 0x70 + u32::from(n) - 1,
            Key::Named(named) => NAMED
                .iter()
                .find(|(_, n, _)| *n == named)
                .map_or(0, |(_, _, vk)| *vk),
        }
    }
}

fn parse_key(name: &str) -> Option<Key> {
    let upper = name.to_ascii_uppercase();
    let mut chars = upper.chars();
    if let (Some(c), None) = (chars.next(), chars.next()) {
        return match c {
            'A'..='Z' => Some(Key::Letter(c)),
            '0'..='9' => Some(Key::Digit(c as u8 - b'0')),
            _ => None,
        };
    }
    if let Some(n) = upper.strip_prefix('F').and_then(|n| n.parse::<u8>().ok()) {
        return (1..=24).contains(&n).then_some(Key::Function(n));
    }
    let alias = match upper.as_str() {
        "ESCAPE" => "ESC",
        "RETURN" => "ENTER",
        "DEL" => "DELETE",
        "INS" => "INSERT",
        "PGUP" => "PAGEUP",
        "PGDN" => "PAGEDOWN",
        other => other,
    };
    NAMED
        .iter()
        .find(|(n, _, _)| n.eq_ignore_ascii_case(alias))
        .map(|(_, named, _)| Key::Named(*named))
}

impl fmt::Display for Hotkey {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let m = self.modifiers;
        for (on, name) in [
            (m.ctrl, "Ctrl"),
            (m.shift, "Shift"),
            (m.alt, "Alt"),
            (m.win, "Win"),
        ] {
            if on {
                write!(f, "{name}+")?;
            }
        }
        match self.key {
            Key::Letter(c) => write!(f, "{c}"),
            Key::Digit(d) => write!(f, "{d}"),
            Key::Function(n) => write!(f, "F{n}"),
            Key::Named(named) => f.write_str(
                NAMED
                    .iter()
                    .find(|(_, n, _)| *n == named)
                    .map_or("?", |(name, _, _)| name),
            ),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_recommended_form() {
        let h = Hotkey::parse("Ctrl+Shift+Alt+F1").unwrap();
        assert!(h.modifiers.ctrl && h.modifiers.shift && h.modifiers.alt && !h.modifiers.win);
        assert_eq!(h.key, Key::Function(1));
        assert_eq!(h.virtual_key(), 0x70);
        assert_eq!(h.to_string(), "Ctrl+Shift+Alt+F1");
        assert!(!h.steals_altgr());
    }

    #[test]
    fn is_case_insensitive_and_order_independent() {
        assert_eq!(
            Hotkey::parse("alt + CTRL + shift + f1").unwrap(),
            Hotkey::parse("Ctrl+Shift+Alt+F1").unwrap()
        );
        assert_eq!(
            Hotkey::parse("Win+escape").unwrap(),
            Hotkey::parse("win+Esc").unwrap()
        );
    }

    #[test]
    fn virtual_keys() {
        assert_eq!(Hotkey::parse("Win+K").unwrap().virtual_key(), 0x4B);
        assert_eq!(Hotkey::parse("Ctrl+7").unwrap().virtual_key(), 0x37);
        assert_eq!(Hotkey::parse("F24").unwrap().virtual_key(), 0x87);
        assert_eq!(Hotkey::parse("Ctrl+PageDown").unwrap().virtual_key(), 0x22);
    }

    #[test]
    fn ctrl_alt_printable_steals_altgr() {
        assert!(Hotkey::parse("Ctrl+Alt+7").unwrap().steals_altgr());
        assert!(Hotkey::parse("Ctrl+Alt+Shift+Q").unwrap().steals_altgr());
        assert!(!Hotkey::parse("Ctrl+Alt+F7").unwrap().steals_altgr());
        assert!(!Hotkey::parse("Ctrl+Shift+Q").unwrap().steals_altgr());
    }

    #[test]
    fn function_key_alone_is_allowed_other_keys_need_a_modifier() {
        assert!(Hotkey::parse("F13").is_ok());
        assert!(Hotkey::parse("K").is_err());
        assert!(Hotkey::parse("Space").is_err());
    }

    #[test]
    fn rejects_malformed_input() {
        for bad in [
            "",
            "Ctrl+",
            "+F1",
            "Ctrl++F1",
            "Ctrl+Shift",
            "Hyper+F1",
            "Ctrl+Ctrl+F1",
            "Ctrl+F25",
            "Ctrl+F0",
            "Ctrl+Ä",
            "Ctrl+AB",
        ] {
            assert!(Hotkey::parse(bad).is_err(), "{bad:?} should be rejected");
        }
    }
}
