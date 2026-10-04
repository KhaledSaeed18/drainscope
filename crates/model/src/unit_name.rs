//! systemd unit-name escaping (`systemd.unit(5)`, `systemd-escape(1)`): bytes outside the
//! allowed set are written as `\xNN`, e.g. `-` inside an application id becomes `\x2d`.

/// Reverses `\xNN` escaping. Malformed escapes are kept literally; invalid UTF-8 is replaced.
#[must_use]
pub fn unescape(name: &str) -> String {
    let bytes = name.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while let Some(&byte) = bytes.get(i) {
        if byte == b'\\'
            && bytes.get(i + 1) == Some(&b'x')
            && let Some(decoded) = bytes
                .get(i + 2..i + 4)
                .and_then(|hex| std::str::from_utf8(hex).ok())
                .and_then(|hex| u8::from_str_radix(hex, 16).ok())
        {
            out.push(decoded);
            i += 4;
            continue;
        }
        out.push(byte);
        i += 1;
    }
    String::from_utf8_lossy(&out).into_owned()
}

#[cfg(test)]
mod tests {
    use super::unescape;

    #[test]
    fn decodes_escaped_dashes() {
        assert_eq!(
            unescape(r"app-gnome\x2dsession\x2dmanager.slice"),
            "app-gnome-session-manager.slice"
        );
    }

    #[test]
    fn decodes_multibyte_utf8() {
        assert_eq!(unescape(r"caf\xc3\xa9.service"), "café.service");
    }

    #[test]
    fn keeps_malformed_escapes() {
        assert_eq!(unescape(r"a\x2"), r"a\x2");
        assert_eq!(unescape(r"a\xZZb"), r"a\xZZb");
        assert_eq!(unescape(r"trailing\"), r"trailing\");
    }

    #[test]
    fn plain_names_are_unchanged() {
        assert_eq!(unescape("NetworkManager.service"), "NetworkManager.service");
    }
}
