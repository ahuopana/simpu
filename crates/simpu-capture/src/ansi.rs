//! Turn raw terminal output into readable text: strip escape sequences and
//! apply carriage returns / backspaces / erase-to-end-of-line like a terminal
//! would, so progress bars and edited command lines read sanely.

/// Clean raw bytes (lossy UTF-8) into plain text with `\n` line endings.
pub fn clean(raw: &[u8]) -> String {
    let text = String::from_utf8_lossy(raw);
    let mut lines: Vec<String> = Vec::new();
    let mut line: Vec<char> = Vec::new();
    let mut cursor = 0usize;
    let mut it = text.chars().peekable();
    while let Some(c) = it.next() {
        match c {
            '\x1b' => match it.next() {
                Some('[') => {
                    let mut params = String::new();
                    let mut fin = None;
                    for n in it.by_ref() {
                        if ('@'..='~').contains(&n) {
                            fin = Some(n);
                            break;
                        }
                        params.push(n);
                    }
                    if fin == Some('K') && matches!(params.as_str(), "" | "0") {
                        line.truncate(cursor);
                    } else if fin == Some('K') && params == "2" {
                        line.clear();
                        cursor = 0;
                    }
                }
                Some(']') => {
                    // OSC: until BEL or ESC \
                    while let Some(n) = it.next() {
                        if n == '\x07' {
                            break;
                        }
                        if n == '\x1b' {
                            it.next();
                            break;
                        }
                    }
                }
                Some('(' | ')' | '*' | '+') => {
                    it.next();
                }
                _ => {}
            },
            '\r' => cursor = 0,
            '\n' => {
                lines.push(line.iter().collect());
                line.clear();
                cursor = 0;
            }
            '\x08' => cursor = cursor.saturating_sub(1),
            '\x07' | '\0' => {}
            c => {
                if cursor < line.len() {
                    line[cursor] = c;
                } else {
                    line.push(c);
                }
                cursor += 1;
            }
        }
    }
    if !line.is_empty() {
        lines.push(line.iter().collect());
    }
    let mut out = lines
        .iter()
        .map(|l| l.trim_end())
        .collect::<Vec<_>>()
        .join("\n");
    if text.ends_with('\n') {
        out.push('\n');
    }
    out
}

#[cfg(test)]
mod tests {
    use super::clean;

    #[test]
    fn strips_colors_and_osc_titles() {
        assert_eq!(clean(b"\x1b[31mred\x1b[0m ok\r\n"), "red ok\n");
        assert_eq!(clean(b"\x1b]0;user@host: ~\x07$ ls\r\n"), "$ ls\n");
        assert_eq!(clean(b"\x1b]0;title\x1b\\x"), "x");
        assert_eq!(clean(b"\x1b(Babc"), "abc");
    }

    #[test]
    fn carriage_return_overwrites_progress_bars() {
        assert_eq!(clean(b"10%\r50%\r100%\r\ndone\r\n"), "100%\ndone\n");
        assert_eq!(clean(b"abcdef\rxy\r\n"), "xycdef\n");
    }

    #[test]
    fn backspace_and_erase() {
        assert_eq!(clean(b"lsx\x08 \x08\r\n"), "ls\n");
        assert_eq!(clean(b"hello\r\x1b[Kbye\r\n"), "bye\n");
        assert_eq!(clean(b"hello\x1b[2Kx"), "x");
    }

    #[test]
    fn handles_invalid_utf8_and_unterminated_sequences() {
        assert_eq!(clean(&[b'a', 0xff, b'b']), "a\u{fffd}b");
        assert_eq!(clean(b"a\x1b[31"), "a");
        assert_eq!(clean(b""), "");
    }
}
