//! Test double for `keepassxc-cli` (portable, no shell scripts).
//!
//! Environment:
//! - `FAKE_KP_PASSWORD`: expected master password (read from stdin).
//! - `FAKE_KP_ENTRIES`: `Entry:Attr=value;Entry:att:name=value;...`
//!   (`Attr` is Password/Notes, `att:` marks an attachment).

use std::io::Read;

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let mut stdin = String::new();
    let _ = std::io::stdin().read_to_string(&mut stdin);
    let given = stdin.lines().next().unwrap_or("");
    if Some(given.to_string()) != std::env::var("FAKE_KP_PASSWORD").ok() {
        eprintln!("Error while reading the database: Invalid credentials were provided, please try again.");
        std::process::exit(1);
    }
    let cmd = args.first().map(String::as_str).unwrap_or("");
    // Split options (with values for -a/-k) from positionals.
    let mut attr = String::new();
    let mut pos: Vec<&str> = Vec::new();
    let mut i = 1;
    while i < args.len() {
        match args[i].as_str() {
            "-a" => {
                attr = args.get(i + 1).cloned().unwrap_or_default();
                i += 1;
            }
            "-k" => i += 1,
            "-q" | "-s" | "--stdout" => {}
            other => pos.push(other),
        }
        i += 1;
    }
    let entries = std::env::var("FAKE_KP_ENTRIES").unwrap_or_default();
    let lookup = |entry: &str, key: &str| {
        entries.split(';').find_map(|e| {
            let (k, v) = e.split_once('=')?;
            (k == format!("{entry}:{key}")).then(|| v.to_string())
        })
    };
    match cmd {
        "ls" => {}
        "show" => match pos.get(1).and_then(|e| lookup(e, &attr)) {
            Some(v) => println!("{v}"),
            None => {
                eprintln!(
                    "Could not find entry with path {}.",
                    pos.get(1).unwrap_or(&"")
                );
                std::process::exit(1);
            }
        },
        "attachment-export" => match (pos.get(1), pos.get(2)) {
            (Some(e), Some(a)) => match lookup(e, &format!("att:{a}")) {
                Some(v) => print!("{v}"),
                None => {
                    eprintln!("Could not find attachment {a}.");
                    std::process::exit(1);
                }
            },
            _ => std::process::exit(2),
        },
        _ => std::process::exit(2),
    }
}
