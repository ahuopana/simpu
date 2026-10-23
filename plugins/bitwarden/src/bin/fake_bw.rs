//! Test double for the `bw` CLI. Vault contents come from env:
//! `FAKE_BW_STATUS` (default "unlocked"), `FAKE_BW_ITEMS` as `id=password;id2=...`.
//! Portable (no shell scripts) so tests also run on Windows.

const TOKEN: &str = "session-token-123";

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    // Unlocked when configured so, or once `unlock` handed out the token and it is passed back.
    let configured = std::env::var("FAKE_BW_STATUS").unwrap_or_else(|_| "unlocked".into());
    let status = if std::env::var("BW_SESSION").as_deref() == Ok(TOKEN) {
        "unlocked".to_string()
    } else {
        configured
    };
    match args
        .iter()
        .map(String::as_str)
        .collect::<Vec<_>>()
        .as_slice()
    {
        ["unlock", "--passwordenv", var, "--raw"] => {
            let given = std::env::var(var).unwrap_or_default();
            if Some(given) == std::env::var("FAKE_BW_PASSWORD").ok() {
                println!("{TOKEN}");
            } else {
                eprintln!("Invalid master password.");
                std::process::exit(1);
            }
        }
        ["status"] => println!("{{\"status\":\"{status}\"}}"),
        ["get", _field, _] if status != "unlocked" => {
            eprintln!("Vault is locked.");
            std::process::exit(1);
        }
        ["get", _field, item] => {
            let items = std::env::var("FAKE_BW_ITEMS").unwrap_or_default();
            for pair in items.split(';') {
                if let Some((k, v)) = pair.split_once('=') {
                    if k == *item {
                        println!("{v}");
                        return;
                    }
                }
            }
            eprintln!("Not found.");
            std::process::exit(1);
        }
        _ => {
            eprintln!("unsupported");
            std::process::exit(2);
        }
    }
}
