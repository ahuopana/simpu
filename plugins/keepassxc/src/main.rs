use simpu_plugin_keepassxc::{KeePassXc, RealCli};
use std::io::{stdin, stdout};

fn main() -> std::io::Result<()> {
    let plugin = KeePassXc::from_env(RealCli::from_env());
    simpu_plugin_api::serve(&plugin, stdin().lock(), stdout().lock())
}
