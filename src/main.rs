use anyhow::anyhow;
use serde::Deserialize;
use std::{fs, io::{stderr, BufRead, BufReader, Write}, process::{Child, ChildStdin, ChildStdout, Command, Stdio}};

const INITMSG: &str = r#"# PineEntry
# A GNU pinentry caching proxy
#
# Src: https://github.com/asciimoth/pineentry
# Config file: ~/.config/pineentry/config.yaml or alt with $PINEENTRY_CFG
# Usage: TODO
#"#;

#[derive(Deserialize, Debug, Clone)]
struct Config {
    servers: Vec<String>,
}

#[derive(Debug)]
struct Server {
    process: Child,
    stdin: Option<ChildStdin>,
    stdout: Option<ChildStdout>,
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
    let yaml_content = match fs::read_to_string(path) {
        Ok(y) => y,
        Err(_) => {
            println!("# Missing config, using default");
            return Ok(Config {
                servers: vec![
                    String::from("pinentry-qt"),
                    String::from("pinentry-gtk"),
                    String::from("pinentry-curses"),
                    String::from("pinentry-tty"),
                ],
            });
        },
    };
    Ok(serde_yaml::from_str(&yaml_content)?)
}

fn run_server(server: &str) -> anyhow::Result<Server> {
    let mut child = Command::new(server)
        .stdin(Stdio::piped())   // we will write to stdin
        .stdout(Stdio::piped())  // we will capture stdout
        .stderr(stderr())     // redirect stderr
        .spawn()?;
    let stdin = child.stdin.take().ok_or(anyhow!("Failed to take stdin"))?;
    let stdout = child.stdout.take().ok_or(anyhow!("Failed to take stdout"))?;
    Ok(Server { process: child, stdin: Some(stdin), stdout: Some(stdout) })
}

fn launch(cfg: &Config) -> anyhow::Result<Server> {
    if cfg.servers.is_empty() {
        return Err(anyhow!("There is no servers in config"));
    }
    for server in &cfg.servers {
        println!("# Running {}", server);
        match run_server(server) {
            Ok(s) => {
                return Ok(s);
            },
            Err(err) => println!("# ERR: {}", err),
        }
    }
    Err(anyhow!("Failed to run any server"))
}

fn main() -> anyhow::Result<()> {
    println!("{}", INITMSG);
    let cfg = load()?;
    let mut server = launch(&cfg)?;
    // proxy(&cfg, &mut server)?;
    Ok(())
}
