//! Test double for the `bw` CLI. Vault contents come from env:
//! `FAKE_BW_STATUS` (default "unlocked"), `FAKE_BW_ITEMS` as `id=password;id2=...`.
//! Portable (no shell scripts) so tests also run on Windows.

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let status = std::env::var("FAKE_BW_STATUS").unwrap_or_else(|_| "unlocked".into());
    match args
        .iter()
        .map(String::as_str)
        .collect::<Vec<_>>()
        .as_slice()
    {
        ["status"] => println!("{{\"status\":\"{status}\"}}"),
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
