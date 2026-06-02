//! Engine-agnostic key → PTY-byte encoding.
//!
//! The terminal must translate a key press into the byte sequence a shell or
//! TUI expects on its PTY. The exact sequences (CSI `ESC [ x`, SS3 `ESC O x`,
//! the `ESC [ n ~` editing-key tilde form, Alt-as-Meta `ESC`-prefixing) are the
//! single source of truth and live HERE, in the UI-free core, so every front
//! end (the legacy winit shell, the modern egui shell, and the headless tests)
//! shares ONE encoding rather than re-deriving it.
//!
//! Each front end maps its native key type onto [`LogicalKey`] and calls
//! [`encode_key`]; the escape sequences are never duplicated. The winit shell's
//! `key_to_bytes` and the egui shell's input-forwarding both delegate here.

/// A platform-neutral key press, abstracted away from any windowing toolkit.
///
/// A front end translates its native key event into one of these (named keys
/// for the special keys it recognises; [`LogicalKey::Text`] for everything that
/// produces composed Unicode, including ordinary characters and IME output).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LogicalKey {
    /// Composed text (a normal character, an IME commit, a dead-key result).
    /// Carries the already-composed UTF-8 string the front end received.
    Text(String),
    /// Return / Enter — encodes as CR (`\r`).
    Enter,
    /// Backspace — encodes as DEL (`0x7f`), the xterm default.
    Backspace,
    /// Tab — encodes as `\t`.
    Tab,
    /// Escape — encodes as `0x1b`.
    Escape,
    /// Space — encodes as `' '`.
    Space,
    /// Up arrow.
    ArrowUp,
    /// Down arrow.
    ArrowDown,
    /// Right arrow.
    ArrowRight,
    /// Left arrow.
    ArrowLeft,
    /// Home key.
    Home,
    /// End key.
    End,
    /// Insert key.
    Insert,
    /// Delete (forward-delete) key.
    Delete,
    /// Page Up.
    PageUp,
    /// Page Down.
    PageDown,
    /// Function key F1–F12 (`n` is 1..=12). Out-of-range `n` encodes nothing.
    Function(u8),
}

/// Active modifier keys at the time of the press. Only `alt` currently affects
/// the encoding (Alt-as-Meta `ESC`-prefixing); `ctrl`/`shift`/`logo` are carried
/// for completeness and forward-compatibility so callers need not change shape.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct KeyModifiers {
    /// Control key held.
    pub ctrl: bool,
    /// Alt / Option key held (acts as Meta — `ESC`-prefixes a text key).
    pub alt: bool,
    /// Shift key held.
    pub shift: bool,
    /// Logo / Super / Command key held.
    pub logo: bool,
}

impl KeyModifiers {
    /// No modifiers.
    pub const NONE: Self = Self {
        ctrl: false,
        alt: false,
        shift: false,
        logo: false,
    };
}

/// Encode a logical key into the bytes to write to the PTY.
///
/// Honours DECCKM (`app_cursor`): in application-cursor mode the arrow and
/// Home/End keys use SS3 (`ESC O x`) instead of CSI (`ESC [ x`), which is what
/// readline/vim expect. Encodes the function keys (F1–F12) and editing keys
/// (Home/End/Insert/Delete/PageUp/PageDown). Alt acts as Meta: an `Alt`-modified
/// text key is prefixed with `ESC` (the xterm convention behind Alt+B / Alt+F
/// word motion in bash). Returns `None` when the key produces no PTY bytes
/// (e.g. an empty text string, or an out-of-range function key).
pub fn encode_key(key: &LogicalKey, app_cursor: bool, mods: KeyModifiers) -> Option<Vec<u8>> {
    // Arrow / Home / End: SS3 in application-cursor mode, else CSI.
    let cursor = |c: u8| -> Vec<u8> {
        if app_cursor {
            vec![0x1b, b'O', c]
        } else {
            vec![0x1b, b'[', c]
        }
    };
    // `ESC [ <n> ~` editing/function-key form.
    let tilde = |n: &[u8]| -> Vec<u8> {
        let mut v = Vec::with_capacity(n.len() + 3);
        v.extend_from_slice(b"\x1b[");
        v.extend_from_slice(n);
        v.push(b'~');
        v
    };
    let base: Option<Vec<u8>> = match key {
        LogicalKey::Enter => Some(vec![b'\r']),
        LogicalKey::Backspace => Some(vec![0x7f]),
        LogicalKey::Tab => Some(vec![b'\t']),
        LogicalKey::Escape => Some(vec![0x1b]),
        LogicalKey::Space => Some(vec![b' ']),
        LogicalKey::ArrowUp => Some(cursor(b'A')),
        LogicalKey::ArrowDown => Some(cursor(b'B')),
        LogicalKey::ArrowRight => Some(cursor(b'C')),
        LogicalKey::ArrowLeft => Some(cursor(b'D')),
        LogicalKey::Home => Some(cursor(b'H')),
        LogicalKey::End => Some(cursor(b'F')),
        LogicalKey::Insert => Some(tilde(b"2")),
        LogicalKey::Delete => Some(tilde(b"3")),
        LogicalKey::PageUp => Some(tilde(b"5")),
        LogicalKey::PageDown => Some(tilde(b"6")),
        // F1–F4 use SS3 (the VT100 PF-key form); F5–F12 use CSI tilde.
        LogicalKey::Function(n) => match n {
            1 => Some(vec![0x1b, b'O', b'P']),
            2 => Some(vec![0x1b, b'O', b'Q']),
            3 => Some(vec![0x1b, b'O', b'R']),
            4 => Some(vec![0x1b, b'O', b'S']),
            5 => Some(tilde(b"15")),
            6 => Some(tilde(b"17")),
            7 => Some(tilde(b"18")),
            8 => Some(tilde(b"19")),
            9 => Some(tilde(b"20")),
            10 => Some(tilde(b"21")),
            11 => Some(tilde(b"23")),
            12 => Some(tilde(b"24")),
            _ => None,
        },
        LogicalKey::Text(s) => {
            if s.is_empty() {
                None
            } else {
                Some(s.as_bytes().to_vec())
            }
        }
    };
    // Alt = Meta: prefix ESC for a text-producing key (never double-prefix a
    // sequence that already starts with ESC, e.g. the arrows above).
    match base {
        Some(bytes) if mods.alt && bytes.first() != Some(&0x1b) => {
            let mut v = Vec::with_capacity(bytes.len() + 1);
            v.push(0x1b);
            v.extend_from_slice(&bytes);
            Some(v)
        }
        other => other,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn enter_encodes_carriage_return() {
        assert_eq!(
            encode_key(&LogicalKey::Enter, false, KeyModifiers::NONE),
            Some(vec![b'\r'])
        );
    }

    #[test]
    fn text_passes_through_as_utf8() {
        assert_eq!(
            encode_key(&LogicalKey::Text("hi".into()), false, KeyModifiers::NONE),
            Some(b"hi".to_vec())
        );
    }

    #[test]
    fn empty_text_encodes_nothing() {
        assert_eq!(
            encode_key(&LogicalKey::Text(String::new()), false, KeyModifiers::NONE),
            None
        );
    }

    #[test]
    fn arrow_up_csi_in_normal_mode_ss3_in_app_mode() {
        assert_eq!(
            encode_key(&LogicalKey::ArrowUp, false, KeyModifiers::NONE),
            Some(b"\x1b[A".to_vec())
        );
        assert_eq!(
            encode_key(&LogicalKey::ArrowUp, true, KeyModifiers::NONE),
            Some(b"\x1bOA".to_vec())
        );
    }

    #[test]
    fn home_end_track_app_cursor_mode() {
        assert_eq!(
            encode_key(&LogicalKey::Home, false, KeyModifiers::NONE),
            Some(b"\x1b[H".to_vec())
        );
        assert_eq!(
            encode_key(&LogicalKey::End, true, KeyModifiers::NONE),
            Some(b"\x1bOF".to_vec())
        );
    }

    #[test]
    fn editing_keys_use_tilde_form() {
        assert_eq!(
            encode_key(&LogicalKey::Delete, false, KeyModifiers::NONE),
            Some(b"\x1b[3~".to_vec())
        );
        assert_eq!(
            encode_key(&LogicalKey::PageUp, false, KeyModifiers::NONE),
            Some(b"\x1b[5~".to_vec())
        );
    }

    #[test]
    fn function_keys_f1_f4_use_ss3_f5_plus_use_tilde() {
        assert_eq!(
            encode_key(&LogicalKey::Function(1), false, KeyModifiers::NONE),
            Some(b"\x1bOP".to_vec())
        );
        assert_eq!(
            encode_key(&LogicalKey::Function(5), false, KeyModifiers::NONE),
            Some(b"\x1b[15~".to_vec())
        );
        assert_eq!(
            encode_key(&LogicalKey::Function(12), false, KeyModifiers::NONE),
            Some(b"\x1b[24~".to_vec())
        );
        // Out-of-range function key encodes nothing.
        assert_eq!(
            encode_key(&LogicalKey::Function(99), false, KeyModifiers::NONE),
            None
        );
    }

    #[test]
    fn alt_prefixes_esc_for_text_but_not_for_escape_sequences() {
        // Alt+b → ESC b (Meta-b word-back in bash).
        let alt = KeyModifiers {
            alt: true,
            ..KeyModifiers::NONE
        };
        assert_eq!(
            encode_key(&LogicalKey::Text("b".into()), false, alt),
            Some(vec![0x1b, b'b'])
        );
        // Alt+ArrowUp must NOT double-prefix ESC (it already starts with ESC).
        assert_eq!(
            encode_key(&LogicalKey::ArrowUp, false, alt),
            Some(b"\x1b[A".to_vec())
        );
    }
}
