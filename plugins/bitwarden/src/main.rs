use simpu_plugin_bitwarden::{Bitwarden, CliRunner};
use std::io::{stdin, stdout};

fn main() -> std::io::Result<()> {
    let plugin = Bitwarden {
        runner: CliRunner::from_env(),
    };
    simpu_plugin_api::serve(&plugin, stdin().lock(), stdout().lock())
}
