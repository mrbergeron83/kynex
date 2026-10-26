//! Keys to the bytes a terminal program expects (xterm conventions).

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
/// `app_cursor` is the terminal's application-cursor mode (vim, less…).
pub fn key_bytes(k: &Key, app_cursor: bool) -> Option<Vec<u8>> {
    let modifier = 1 + k.shift as u8 + 2 * k.alt as u8 + 4 * k.ctrl as u8;
    let csi = |tail: char| -> Vec<u8> {
        if modifier > 1 {
            format!("\x1b[1;{modifier}{tail}").into_bytes()
        } else if app_cursor {
            format!("\x1bO{tail}").into_bytes()
        } else {
            format!("\x1b[{tail}").into_bytes()
        }
    };
    let tilde = |n: u8| -> Vec<u8> {
        if modifier > 1 { format!("\x1b[{n};{modifier}~").into_bytes() } else { format!("\x1b[{n}~").into_bytes() }
    };
    let alt = |bytes: Vec<u8>| if k.alt { [b"\x1b".as_slice(), &bytes].concat() } else { bytes };
    Some(match k.key {
        "enter" => alt(b"\r".to_vec()),
        "backspace" => alt(if k.ctrl { vec![0x08] } else { vec![0x7f] }),
        "tab" if k.shift => b"\x1b[Z".to_vec(),
        "tab" => b"\t".to_vec(),
        "escape" => b"\x1b".to_vec(),
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
        "f1" => b"\x1bOP".to_vec(),
        "f2" => b"\x1bOQ".to_vec(),
        "f3" => b"\x1bOR".to_vec(),
        "f4" => b"\x1bOS".to_vec(),
        "f5" => tilde(15),
        "f6" => tilde(17),
        "f7" => tilde(18),
        "f8" => tilde(19),
        "f9" => tilde(20),
        "f10" => tilde(21),
        "f11" => tilde(23),
        "f12" => tilde(24),
        "space" if k.ctrl => vec![0],
        // AltGr arrives as Ctrl+Alt on Windows: send what it typed.
        _ if k.ctrl && k.alt && k.text.is_some_and(|t| !t.is_empty()) => k.text?.as_bytes().to_vec(),
        key if k.ctrl && !k.alt => alt(control(key)?),
        _ => alt(k.text.filter(|t| !t.is_empty())?.as_bytes().to_vec()),
    })
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

#[cfg(test)]
mod tests {
    use super::*;

    fn bytes(key: &str, text: Option<&str>, ctrl: bool, alt: bool, shift: bool) -> Vec<u8> {
        key_bytes(&Key { key, text, ctrl, alt, shift }, false).unwrap()
    }

    #[test]
    fn plain_and_special_keys() {
        assert_eq!(bytes("a", Some("a"), false, false, false), b"a");
        assert_eq!(bytes("a", Some("A"), false, false, true), b"A");
        assert_eq!(bytes("enter", None, false, false, false), b"\r");
        assert_eq!(bytes("backspace", None, false, false, false), [0x7f]);
        assert_eq!(bytes("tab", None, false, false, true), b"\x1b[Z");
        assert_eq!(bytes("up", None, false, false, false), b"\x1b[A");
        assert_eq!(key_bytes(&Key { key: "up", ..Default::default() }, true).unwrap(), b"\x1bOA");
        assert_eq!(bytes("right", None, true, false, false), b"\x1b[1;5C");
        assert_eq!(bytes("delete", None, false, false, false), b"\x1b[3~");
        assert_eq!(bytes("f5", None, false, false, false), b"\x1b[15~");
    }

    #[test]
    fn control_and_alt() {
        assert_eq!(bytes("c", Some("c"), true, false, false), [3]);
        assert_eq!(bytes("[", Some("["), true, false, false), [0x1b]);
        assert_eq!(bytes("space", None, true, false, false), [0]);
        assert_eq!(bytes("b", Some("b"), false, true, false), b"\x1bb");
        assert!(key_bytes(&Key { key: "shift", ..Default::default() }, false).is_none());
    }

    #[test]
    fn pasting() {
        assert_eq!(paste_bytes("a\r\nb\n", false), b"a\rb\r");
        assert_eq!(paste_bytes("x", true), b"\x1b[200~x\x1b[201~");
        assert_eq!(paste_bytes("x\x1b[201~y", true), b"\x1b[200~xy\x1b[201~");
    }
}
