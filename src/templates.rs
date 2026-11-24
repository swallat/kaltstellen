//! H7: ready-made starting points for a profile. A template is data shipped
//! with the tool, like the warn list (P6): a name, a purpose and rules by
//! kind (H3), never by program name. It sets no hotkey, no triggers, no
//! `start` entries and nothing for a program of an unknown kind; the owner
//! sees and changes every row before anything is written.

use crate::config::Action;
use crate::kinds::Kind;

pub struct Template {
    pub id: &'static str,
    pub name: &'static str,
    pub purpose: &'static str,
    pub rules: &'static [(Kind, Action)],
}

use Action::{Ignore, Stop, Suspend};
use Kind::{
    Browser, Chat, CloudSync, Editor, Game, Launcher, Mail, MediaPlayer, PasswordManager, Updater,
};

/// A launcher or a game may be needed, so every template leaves it as it is.
pub const TEMPLATES: &[Template] = &[
    Template {
        id: "focus",
        name: "Focus",
        purpose: "Quiet for work: nothing chimes, nothing updates, the browser stays.",
        rules: &[
            (Chat, Suspend),
            (Mail, Suspend),
            (CloudSync, Suspend),
            (Updater, Stop),
            (Browser, Ignore),
            (Editor, Ignore),
            (PasswordManager, Ignore),
            (Launcher, Ignore),
            (Game, Ignore),
        ],
    },
    Template {
        id: "gaming",
        name: "Gaming",
        purpose: "Memory and processor for the game: what runs beside it waits.",
        rules: &[
            (Browser, Suspend),
            (Mail, Suspend),
            (CloudSync, Suspend),
            (Updater, Stop),
            (MediaPlayer, Stop),
            (Chat, Ignore),
            (PasswordManager, Ignore),
            (Launcher, Ignore),
            (Game, Ignore),
        ],
    },
    Template {
        id: "presentation",
        name: "Presentation",
        purpose: "Nothing pops up on the screen while you present.",
        rules: &[
            (Chat, Suspend),
            (Mail, Suspend),
            (CloudSync, Suspend),
            (Updater, Stop),
            (Browser, Ignore),
            (Editor, Ignore),
            (MediaPlayer, Ignore),
            (PasswordManager, Ignore),
            (Launcher, Ignore),
            (Game, Ignore),
        ],
    },
    Template {
        id: "battery",
        name: "Battery saver",
        purpose: "Less running in the background while you are on battery.",
        rules: &[
            (Browser, Suspend),
            (Chat, Suspend),
            (Mail, Suspend),
            (CloudSync, Suspend),
            (Updater, Stop),
            (MediaPlayer, Stop),
            (Editor, Ignore),
            (PasswordManager, Ignore),
            (Launcher, Ignore),
            (Game, Ignore),
        ],
    },
];

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashSet;

    #[test]
    fn h7_templates_have_unique_ids_names_and_one_rule_per_kind() {
        let mut ids = HashSet::new();
        for template in TEMPLATES {
            assert!(ids.insert(template.id), "{} twice", template.id);
            let mut kinds = HashSet::new();
            for (kind, _) in template.rules {
                assert!(
                    kinds.insert(kind.id()),
                    "{}: {} twice",
                    template.id,
                    kind.id()
                );
            }
        }
    }

    #[test]
    fn h7_rules_never_start_and_never_touch_what_has_no_suggestion() {
        for template in TEMPLATES {
            for (kind, action) in template.rules {
                assert_ne!(*action, Action::Start, "{}", template.id);
                assert!(
                    kind.suggestion().is_some(),
                    "{}: {} is never placed",
                    template.id,
                    kind.id()
                );
            }
        }
    }

    #[test]
    fn h7_launchers_and_games_are_left_as_they_are_everywhere() {
        for template in TEMPLATES {
            for wanted in [Launcher, Game] {
                let rule = template.rules.iter().find(|(k, _)| *k == wanted);
                assert_eq!(rule.map(|(_, a)| *a), Some(Ignore), "{}", template.id);
            }
        }
    }
}
