use serde::Deserialize;
use std::fs;

const INITMSG: &str = r#"# PineEntry
# A GNU pinentry caching proxy
#
# Src: https://github.com/asciimoth/pineentry
# Config file: ~/.config/pineentry/config.yaml or alt with $PINEENTRY_CFG
# Usage: TODO
#"#;

#[derive(Deserialize, Debug, Clone)]
struct Config {
}

fn load() -> anyhow::Result<Config> {
    let mut path = String::from("~/.config/pineentry/config.yaml");
    if let Ok(env) = std::env::var("PINEENTRY_CFG"){
        if env.len() > 0 {
            path = env
        }
    }
    path = shellexpand::tilde(&path).to_string();
    println!("# Loading config from {}", path);
    let yaml_content = fs::read_to_string(path)?;
    Ok(serde_yaml::from_str(&yaml_content)?)
}

fn main() -> anyhow::Result<()> {
    println!("{}", INITMSG);
    load()?;
    Ok(())
}
