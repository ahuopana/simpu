//! Split cleaned terminal text into command blocks using prompt detection.

use regex::Regex;

/// Matches the prompt at the start of a line (`user@host:~$ `, `PS C:\x> `,
/// `switch# `, `[root@h ~]# `, `(venv) $ `). Everything after the match is the command.
pub const DEFAULT_PROMPT: &str = r"^(?:\([^)\n]{1,30}\)\s*)?(?:(?:\[[^\]\n]{1,80}\]|[^\s$#%>\n][^$#%>\n]{0,80})?[$#%>](?:\s|$)|[\w.\-]+(?:\([\w\-/]+\))?[#>])";

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Block {
    pub command: String,
    /// Full text of the block: prompt line, command and its output (no trailing newline).
    pub text: String,
}

pub struct PromptDetector {
    re: Regex,
}

impl PromptDetector {
    pub fn new(pattern: &str) -> Result<Self, regex::Error> {
        Ok(Self {
            re: Regex::new(pattern)?,
        })
    }

    pub fn default_detector() -> Self {
        Self::new(DEFAULT_PROMPT).expect("default prompt regex is valid")
    }

    /// `Some(command)` when `line` starts with a prompt.
    fn command_of<'a>(&self, line: &'a str) -> Option<&'a str> {
        self.re.find(line).map(|m| line[m.end()..].trim())
    }

    /// Blocks that start at a prompt line. Output before the first prompt is dropped.
    pub fn blocks(&self, text: &str) -> Vec<Block> {
        let mut out: Vec<(String, Vec<&str>)> = Vec::new();
        for line in text.lines() {
            match self.command_of(line) {
                Some(cmd) => out.push((cmd.to_string(), vec![line])),
                None => {
                    if let Some((_, lines)) = out.last_mut() {
                        lines.push(line);
                    }
                }
            }
        }
        out.into_iter()
            .map(|(command, lines)| Block {
                command,
                text: lines.join("\n").trim_end().to_string(),
            })
            .collect()
    }
}

#[derive(Debug, PartialEq, Eq)]
pub struct LastN {
    pub text: String,
    /// True when no prompt was found and the last N lines were used instead.
    pub fallback_to_lines: bool,
}

/// The last `n` commands (with their output); empty prompts (just Enter) don't count.
pub fn last_n(text: &str, n: usize, d: &PromptDetector) -> LastN {
    let blocks: Vec<Block> = d
        .blocks(text)
        .into_iter()
        .filter(|b| !b.command.is_empty())
        .collect();
    if blocks.is_empty() {
        let lines: Vec<&str> = text.lines().collect();
        let from = lines.len().saturating_sub(n);
        return LastN {
            text: lines[from..].join("\n"),
            fallback_to_lines: true,
        };
    }
    let from = blocks.len().saturating_sub(n);
    LastN {
        text: blocks[from..]
            .iter()
            .map(|b| b.text.as_str())
            .collect::<Vec<_>>()
            .join("\n"),
        fallback_to_lines: false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const BASH: &str = "alice@web1:~$ ls\nfile1  file2\nalice@web1:~$ \nalice@web1:~$ cat file1\nhello\nalice@web1:~$ ";
    const PS: &str =
        "PS C:\\Users\\bob> dir\n  a.txt\nPS C:\\Users\\bob> echo hi\nhi\nPS C:\\Users\\bob> ";
    const CISCO: &str = "sw1>enable\nPassword:\nsw1#show version\nCisco IOS 15.2\nsw1#";
    const ROOT: &str = "[root@db1 ~]# uptime\n 10:00 up 3 days\n[root@db1 ~]# ";
    const VENV: &str = "(venv) bob@box:/srv$ python x.py\nok\n(venv) bob@box:/srv$ ";

    fn cmds(text: &str) -> Vec<String> {
        PromptDetector::default_detector()
            .blocks(text)
            .into_iter()
            .map(|b| b.command)
            .collect()
    }

    #[test]
    fn detects_common_prompts() {
        assert_eq!(cmds(BASH), ["ls", "", "cat file1", ""]);
        assert_eq!(cmds(PS), ["dir", "echo hi", ""]);
        assert_eq!(cmds(CISCO), ["enable", "show version", ""]);
        assert_eq!(cmds(ROOT), ["uptime", ""]);
        assert_eq!(cmds(VENV), ["python x.py", ""]);
    }

    #[test]
    fn last_n_returns_commands_with_output_and_skips_empty_prompts() {
        let d = PromptDetector::default_detector();
        let r = last_n(BASH, 1, &d);
        assert_eq!(r.text, "alice@web1:~$ cat file1\nhello");
        assert!(!r.fallback_to_lines);
        let r = last_n(BASH, 2, &d);
        // the bare prompt in between (just Enter) is not a command
        assert_eq!(
            r.text,
            "alice@web1:~$ ls\nfile1  file2\nalice@web1:~$ cat file1\nhello"
        );
        assert_eq!(last_n(BASH, 99, &d).text.lines().count(), 4);
    }

    #[test]
    fn output_lines_are_not_mistaken_for_prompts() {
        assert_eq!(cmds("$ ls\ntotal 8\n-rw-r--r-- 1 a b 3 x\n$ "), ["ls", ""]);
    }

    #[test]
    fn falls_back_to_lines_without_prompts() {
        let d = PromptDetector::default_detector();
        let r = last_n("one\ntwo\nthree\n", 2, &d);
        assert_eq!(
            r,
            LastN {
                text: "two\nthree".into(),
                fallback_to_lines: true
            }
        );
    }

    #[test]
    fn custom_regex_and_validation() {
        let d = PromptDetector::new(r"^router\(config\)# ").unwrap();
        assert_eq!(
            d.blocks("router(config)# interface x\nok\n")[0].command,
            "interface x"
        );
        assert!(PromptDetector::new("(").is_err());
    }
}
