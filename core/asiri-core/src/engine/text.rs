//! People mentioned in system lines ("Ada added Ben").
//!
//! System lines store a reference to each person instead of their name, and the core renders
//! the current name when the line is read. Someone's name may arrive after the line was
//! written (a newcomer's profile follows their Welcome), and names change.

use rusqlite::Connection;

use super::Core;
use crate::ids::{Id16, id16};
use crate::store::{self, Message, MessageKind};

const OPEN: char = '⟦';
const CLOSE: char = '⟧';

/// A reference to a person inside a system line.
#[must_use]
pub fn mention(user: &Id16) -> String {
    format!("{OPEN}{}{CLOSE}", hex::encode(user))
}

impl Core {
    /// The name to show for a mentioned person: "You"/"you" for the local user, the contact's
    /// name, or "Someone"/"someone".
    fn mention_name(conn: &Connection, me: &Id16, user: &Id16, at_start: bool) -> String {
        let pick = |upper: &str, lower: &str| {
            if at_start {
                upper.to_owned()
            } else {
                lower.to_owned()
            }
        };
        if user == me {
            return pick("You", "you");
        }
        store::contact(conn, user)
            .ok()
            .flatten()
            .map(|c| c.display_name)
            .filter(|n| !n.is_empty())
            .unwrap_or_else(|| pick("Someone", "someone"))
    }

    /// Replaces every mention in `text` with the person's current name.
    pub(crate) fn render_mentions(conn: &Connection, me: &Id16, text: &str) -> String {
        let mut out = String::with_capacity(text.len());
        let mut rest = text;
        while let Some(start) = rest.find(OPEN) {
            let after = &rest[start + OPEN.len_utf8()..];
            let Some(end) = after.find(CLOSE) else { break };
            out.push_str(&rest[..start]);
            let token = &after[..end];
            match hex::decode(token).ok().as_deref().and_then(id16) {
                Some(user) => {
                    out.push_str(&Self::mention_name(conn, me, &user, out.trim().is_empty()))
                }
                None => {
                    out.push(OPEN);
                    out.push_str(token);
                    out.push(CLOSE);
                }
            }
            rest = &after[end + CLOSE.len_utf8()..];
        }
        out.push_str(rest);
        out
    }

    /// A stored message as the app should see it.
    pub(crate) fn render(conn: &Connection, me: &Id16, mut m: Message) -> Message {
        if m.kind == MessageKind::System && m.body.contains(OPEN) {
            m.body = Self::render_mentions(conn, me, &m.body);
        }
        m
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn mentions_render_current_names() {
        let store = store::Store::open_in_memory().unwrap();
        let me = [1u8; 16];
        let ada = [2u8; 16];
        let stranger = [3u8; 16];
        store
            .tx(|c| store::upsert_contact(c, &ada, Some("Ada")))
            .unwrap();
        let conn = store.lock().unwrap();
        let line = format!(
            "{} added {} and {}",
            mention(&ada),
            mention(&me),
            mention(&stranger)
        );
        assert_eq!(
            Core::render_mentions(&conn, &me, &line),
            "Ada added you and someone"
        );
        let line = format!("{} removed {}", mention(&me), mention(&ada));
        assert_eq!(Core::render_mentions(&conn, &me, &line), "You removed Ada");
        assert_eq!(
            Core::render_mentions(&conn, &me, "no ⟦mentions⟧ here"),
            "no ⟦mentions⟧ here"
        );
        assert_eq!(
            Core::render_mentions(&conn, &me, "dangling ⟦"),
            "dangling ⟦"
        );
    }
}
