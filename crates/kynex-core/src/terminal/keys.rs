//! Keys and mouse events to the bytes a terminal program expects (xterm and Kitty keyboard protocol).

/// Terminal keyboard modes that change how keystrokes are encoded.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct KeyMode {
    /// Application-cursor mode (DECCKM, e.g. vim/less): arrows send `ESC O`.
    pub app_cursor: bool,
    /// Kitty keyboard protocol: `DISAMBIGUATE_ESC_CODES` (`CSI > 1 u`).
    pub kitty_disambiguate: bool,
    /// Kitty keyboard protocol: `REPORT_ALL_KEYS_AS_ESC` (`CSI > 8 u`).
    pub kitty_report_all: bool,
    /// Kitty keyboard protocol: `REPORT_ASSOCIATED_TEXT` (`CSI > 16 u`).
    pub kitty_associated_text: bool,
}

impl KeyMode {
    fn kitty(self) -> bool {
        self.kitty_disambiguate || self.kitty_report_all
    }
}

/// A key press, in gpui's naming (`enter`, `up`, `f5`, `a`…).
#[derive(Clone, Copy, Debug, Default)]
pub struct Key<'a> {
    pub key: &'a str,
    /// The text the key types, with shift and layout applied.
    pub text: Option<&'a str>,
    pub ctrl: bool,
    pub alt: bool,
    pub shift: bool,
}

/// The bytes for `key`, or `None` if it means nothing to a terminal.
pub fn key_bytes(k: &Key, mode: KeyMode) -> Option<Vec<u8>> {
    let modifier = 1 + k.shift as u8 + 2 * k.alt as u8 + 4 * k.ctrl as u8;
    let kitty = mode.kitty();
    let csi = |tail: char| -> Vec<u8> {
        if modifier > 1 {
            format!("\x1b[1;{modifier}{tail}").into_bytes()
        } else if mode.app_cursor {
            format!("\x1bO{tail}").into_bytes()
        } else {
            format!("\x1b[{tail}").into_bytes()
        }
    };
    let f1_f4 = |tail: char| -> Vec<u8> {
        if modifier > 1 { format!("\x1b[1;{modifier}{tail}").into_bytes() } else { format!("\x1bO{tail}").into_bytes() }
    };
    let tilde = |n: u8| -> Vec<u8> {
        if modifier > 1 { format!("\x1b[{n};{modifier}~").into_bytes() } else { format!("\x1b[{n}~").into_bytes() }
    };
    let alt = |bytes: Vec<u8>| if k.alt { [b"\x1b".as_slice(), &bytes].concat() } else { bytes };

    // AltGr arrives as Ctrl+Alt with printable text on Windows.
    let altgr = k.ctrl && k.alt && k.text.is_some_and(|t| !t.is_empty() && !t.chars().all(char::is_control));

    Some(match k.key {
        "enter" if kitty && (modifier > 1 || mode.kitty_report_all) => csi_u(13, modifier, None, false),
        "enter" if !kitty && k.shift && !k.ctrl && !k.alt => b"\x1b\r".to_vec(),
        "enter" if !kitty && k.ctrl && !k.alt => b"\n".to_vec(),
        "enter" => alt(b"\r".to_vec()),
        "backspace" if kitty && (modifier > 1 || mode.kitty_report_all) => csi_u(127, modifier, None, false),
        "backspace" => alt(if k.ctrl { vec![0x08] } else { vec![0x7f] }),
        "tab" if kitty && (modifier > 1 || mode.kitty_report_all) => csi_u(9, modifier, None, false),
        "tab" if k.shift => b"\x1b[Z".to_vec(),
        "tab" => alt(b"\t".to_vec()),
        "escape" if kitty => csi_u(27, modifier, None, false),
        "escape" => alt(b"\x1b".to_vec()),
        "up" => csi('A'),
        "down" => csi('B'),
        "right" => csi('C'),
        "left" => csi('D'),
        "home" => csi('H'),
        "end" => csi('F'),
        "insert" => tilde(2),
        "delete" => tilde(3),
        "pageup" => tilde(5),
        "pagedown" => tilde(6),
        "f1" => f1_f4('P'),
        "f2" => f1_f4('Q'),
        "f3" => f1_f4('R'),
        "f4" => f1_f4('S'),
        "f5" => tilde(15),
        "f6" => tilde(17),
        "f7" => tilde(18),
        "f8" => tilde(19),
        "f9" => tilde(20),
        "f10" => tilde(21),
        "f11" => tilde(23),
        "f12" => tilde(24),
        "space" if kitty && (k.ctrl || k.alt || mode.kitty_report_all) => {
            csi_u(32, modifier, Some(" "), mode.kitty_associated_text)
        }
        "space" if k.ctrl => alt(vec![0]),
        "space" => alt(b" ".to_vec()),
        _ if altgr => k.text?.as_bytes().to_vec(),
        _ if kitty && (k.ctrl || k.alt) => csi_u(base_codepoint(k)?, modifier, None, false),
        _ if mode.kitty_report_all => match base_codepoint(k) {
            Some(cp) => csi_u(cp, modifier, k.text, mode.kitty_associated_text),
            None => k.text.filter(|t| !t.is_empty())?.as_bytes().to_vec(),
        },
        key if k.ctrl => alt(control(key)?),
        _ => alt(k.text.filter(|t| !t.is_empty())?.as_bytes().to_vec()),
    })
}

/// Encodes one key under the Kitty keyboard protocol (`CSI codepoint [; modifiers [; text]] u`).
fn csi_u(codepoint: u32, modifier: u8, text: Option<&str>, associated_text: bool) -> Vec<u8> {
    if associated_text && let Some(t) = text {
        let cps: Vec<String> = t.chars().filter(|c| !c.is_control()).map(|c| (c as u32).to_string()).collect();
        if !cps.is_empty() {
            return format!("\x1b[{codepoint};{modifier};{}u", cps.join(":")).into_bytes();
        }
    }
    if modifier > 1 {
        format!("\x1b[{codepoint};{modifier}u").into_bytes()
    } else {
        format!("\x1b[{codepoint}u").into_bytes()
    }
}

/// Unshifted Unicode codepoint for a single-character key (ASCII letters lowercased per Kitty spec).
fn base_codepoint(k: &Key) -> Option<u32> {
    let mut chars = k.key.chars();
    let ch = match (chars.next(), chars.next()) {
        (Some(c), None) => Some(c.to_ascii_lowercase()),
        _ => {
            let mut t = k.text?.chars();
            match (t.next(), t.next()) {
                (Some(c), None) if !c.is_control() => Some(c.to_ascii_lowercase()),
                _ => None,
            }
        }
    }?;
    Some(ch as u32)
}

/// Ctrl+key as a C0 control byte: Ctrl+C is 0x03, Ctrl+[ is Esc.
fn control(key: &str) -> Option<Vec<u8>> {
    let c = key.chars().next().filter(|_| key.chars().count() == 1)?;
    let byte = match c.to_ascii_lowercase() {
        c @ 'a'..='z' => c as u8 - b'a' + 1,
        '@' | '2' => 0,
        '[' | '3' => 0x1b,
        '\\' | '4' => 0x1c,
        ']' | '5' => 0x1d,
        '^' | '6' => 0x1e,
        '_' | '-' | '7' => 0x1f,
        '/' => 0x1f,
        _ => return None,
    };
    Some(vec![byte])
}

/// Text to paste: bracketed when the program asked for it, so a shell doesn't
/// run pasted lines one by one; bare newlines become carriage returns.
pub fn paste_bytes(text: &str, bracketed: bool) -> Vec<u8> {
    let text = text.replace("\r\n", "\n").replace('\n', "\r");
    if bracketed {
        // A pasted end marker would let the rest run as typed input.
        let text = text.replace("\x1b[201~", "");
        format!("\x1b[200~{text}\x1b[201~").into_bytes()
    } else {
        text.into_bytes()
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MouseBtn {
    Left,
    Middle,
    Right,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MouseEventKind {
    Press(MouseBtn),
    Release(MouseBtn),
    Drag(MouseBtn),
    ScrollUp,
    ScrollDown,
}

/// Encodes a mouse event at 0-based `(line, col)` using SGR (preferred) or legacy X10 encoding.
pub fn mouse_bytes(
    kind: MouseEventKind,
    line: usize,
    col: usize,
    shift: bool,
    alt: bool,
    ctrl: bool,
    sgr: bool,
) -> Option<Vec<u8>> {
    let mods = (shift as u8) * 4 + (alt as u8) * 8 + (ctrl as u8) * 16;
    let (base, release) = match kind {
        MouseEventKind::Press(b) => (btn_code(b), false),
        MouseEventKind::Release(b) => (if sgr { btn_code(b) } else { 3 }, true),
        MouseEventKind::Drag(b) => (btn_code(b) + 32, false),
        MouseEventKind::ScrollUp => (64, false),
        MouseEventKind::ScrollDown => (65, false),
    };
    let cb = base + mods;
    if sgr {
        let tail = if release { 'm' } else { 'M' };
        Some(format!("\x1b[<{cb};{};{}{tail}", col + 1, line + 1).into_bytes())
    } else if col < 223 && line < 223 {
        Some(vec![0x1b, b'[', b'M', 32 + cb, 33 + col as u8, 33 + line as u8])
    } else {
        None
    }
}

fn btn_code(btn: MouseBtn) -> u8 {
    match btn {
        MouseBtn::Left => 0,
        MouseBtn::Middle => 1,
        MouseBtn::Right => 2,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn bytes(key: &str, text: Option<&str>, ctrl: bool, alt: bool, shift: bool) -> Vec<u8> {
        key_bytes(&Key { key, text, ctrl, alt, shift }, KeyMode::default()).unwrap()
    }

    #[test]
    fn plain_and_special_keys() {
        assert_eq!(bytes("a", Some("a"), false, false, false), b"a");
        assert_eq!(bytes("a", Some("A"), false, false, true), b"A");
        assert_eq!(bytes("enter", None, false, false, false), b"\r");
        assert_eq!(bytes("enter", None, false, false, true), b"\x1b\r");
        assert_eq!(bytes("enter", None, true, false, false), b"\n");
        assert_eq!(bytes("backspace", None, false, false, false), [0x7f]);
        assert_eq!(bytes("tab", None, false, false, true), b"\x1b[Z");
        assert_eq!(bytes("up", None, false, false, false), b"\x1b[A");
        assert_eq!(
            key_bytes(&Key { key: "up", ..Default::default() }, KeyMode { app_cursor: true, ..Default::default() })
                .unwrap(),
            b"\x1bOA"
        );
        assert_eq!(bytes("right", None, true, false, false), b"\x1b[1;5C");
        assert_eq!(bytes("delete", None, false, false, false), b"\x1b[3~");
        assert_eq!(bytes("f1", None, false, false, false), b"\x1bOP");
        assert_eq!(bytes("f1", None, false, false, true), b"\x1b[1;2P");
        assert_eq!(bytes("f5", None, false, false, false), b"\x1b[15~");
    }

    #[test]
    fn control_and_alt() {
        assert_eq!(bytes("c", Some("c"), true, false, false), [3]);
        assert_eq!(bytes("c", None, true, true, false), [0x1b, 3]);
        assert_eq!(bytes("[", Some("["), true, false, false), [0x1b]);
        assert_eq!(bytes("space", None, true, false, false), [0]);
        assert_eq!(bytes("b", Some("b"), false, true, false), b"\x1bb");
        assert_eq!(bytes("q", Some("@"), true, true, false), b"@");
        assert!(key_bytes(&Key { key: "shift", ..Default::default() }, KeyMode::default()).is_none());
    }

    #[test]
    fn kitty_keyboard_protocol_disambiguates_modifiers() {
        let kitty = KeyMode { kitty_disambiguate: true, ..Default::default() };
        assert_eq!(key_bytes(&Key { key: "enter", ..Default::default() }, kitty).unwrap(), b"\r");
        assert_eq!(key_bytes(&Key { key: "enter", shift: true, ..Default::default() }, kitty).unwrap(), b"\x1b[13;2u");
        assert_eq!(key_bytes(&Key { key: "enter", ctrl: true, ..Default::default() }, kitty).unwrap(), b"\x1b[13;5u");
        assert_eq!(key_bytes(&Key { key: "tab", shift: true, ..Default::default() }, kitty).unwrap(), b"\x1b[9;2u");
        assert_eq!(
            key_bytes(&Key { key: "backspace", ctrl: true, ..Default::default() }, kitty).unwrap(),
            b"\x1b[127;5u"
        );
        assert_eq!(key_bytes(&Key { key: "escape", ..Default::default() }, kitty).unwrap(), b"\x1b[27u");
        assert_eq!(
            key_bytes(&Key { key: "c", text: Some("c"), ctrl: true, ..Default::default() }, kitty).unwrap(),
            b"\x1b[99;5u"
        );
        assert_eq!(
            key_bytes(&Key { key: "a", text: Some("A"), shift: true, ..Default::default() }, kitty).unwrap(),
            b"A"
        );
        // Report-all encodes plain text keys too, optionally with associated text.
        let report_all = KeyMode {
            kitty_disambiguate: true,
            kitty_report_all: true,
            kitty_associated_text: true,
            ..Default::default()
        };
        assert_eq!(
            key_bytes(&Key { key: "a", text: Some("A"), shift: true, ..Default::default() }, report_all).unwrap(),
            b"\x1b[97;2;65u"
        );
        assert_eq!(key_bytes(&Key { key: "enter", ..Default::default() }, report_all).unwrap(), b"\x1b[13u");
    }

    #[test]
    fn mouse_sgr_and_legacy_encoding() {
        assert_eq!(
            mouse_bytes(MouseEventKind::Press(MouseBtn::Left), 4, 9, false, false, false, true).unwrap(),
            b"\x1b[<0;10;5M"
        );
        assert_eq!(
            mouse_bytes(MouseEventKind::Release(MouseBtn::Left), 4, 9, false, false, false, true).unwrap(),
            b"\x1b[<0;10;5m"
        );
        assert_eq!(mouse_bytes(MouseEventKind::ScrollUp, 0, 0, false, false, false, true).unwrap(), b"\x1b[<64;1;1M");
        assert_eq!(mouse_bytes(MouseEventKind::ScrollDown, 2, 3, false, false, true, true).unwrap(), b"\x1b[<81;4;3M");
        assert_eq!(
            mouse_bytes(MouseEventKind::Press(MouseBtn::Left), 0, 0, false, false, false, false).unwrap(),
            vec![0x1b, b'[', b'M', 32, 33, 33]
        );
    }

    #[test]
    fn pasting() {
        assert_eq!(paste_bytes("a\r\nb\n", false), b"a\rb\r");
        assert_eq!(paste_bytes("x", true), b"\x1b[200~x\x1b[201~");
        assert_eq!(paste_bytes("x\x1b[201~y", true), b"\x1b[200~xy\x1b[201~");
    }
}
