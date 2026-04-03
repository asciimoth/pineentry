use anyhow::anyhow;
use serde::Deserialize;
use std::{
    fmt::format,
    fs,
    io::{BufRead, BufReader, Write, stderr, stdin},
    process::{Child, ChildStdin, ChildStdout, Command, Stdio},
    sync::mpsc::{Sender, channel},
    thread,
};

const INITMSG: &str = r#"# PineEntry
# A GNU pinentry caching proxy
#
# Src: https://github.com/asciimoth/pineentry
# Config file: ~/.config/pineentry/config.yaml or alt with $PINEENTRY_CFG
# Usage: TODO
#"#;

#[derive(Deserialize, Debug, Clone)]
struct Config {
    #[serde(default)]
    debug: bool,
    servers: Vec<String>,
}

type Server = (Child, ChildStdin, ChildStdout);

#[derive(Debug)]
enum Event {
    Fail(anyhow::Error),
    ClientInput(String),
    ServerOutput(String),
    ServerStop(),
}

fn load() -> anyhow::Result<Config> {
    let mut path = String::from("~/.config/pineentry/config.yaml");
    if let Ok(env) = std::env::var("PINEENTRY_CFG") {
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
                debug: false,
                servers: vec![
                    String::from("pinentry-qt"),
                    String::from("pinentry-gtk"),
                    String::from("pinentry-curses"),
                    String::from("pinentry-tty"),
                ],
            });
        }
    };
    Ok(serde_yaml::from_str(&yaml_content)?)
}

fn run_server(server: &str) -> anyhow::Result<Server> {
    let mut child = Command::new(server)
        .stdin(Stdio::piped()) // we will write to stdin
        .stdout(Stdio::piped()) // we will capture stdout
        .stderr(stderr()) // redirect stderr
        .spawn()?;
    let stdin = child.stdin.take().ok_or(anyhow!("Failed to take stdin"))?;
    let stdout = child
        .stdout
        .take()
        .ok_or(anyhow!("Failed to take stdout"))?;
    Ok((child, stdin, stdout))
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
            }
            Err(err) => println!("# ERR: {}", err),
        }
    }
    Err(anyhow!("Failed to run any server"))
}

fn client_read(tx: Sender<Event>) {
    let mut reader = BufReader::new(stdin());

    loop {
        let mut line = String::new();
        let bytes_read = match reader.read_line(&mut line) {
            Ok(b) => b,
            Err(err) => {
                let _ = tx.send(Event::Fail(err.into()));
                return;
            }
        };
        if bytes_read == 0 {
            let _ = tx.send(Event::Fail(anyhow!("stdin eof")));
            break; // EOF
        }
        if let Err(_) = tx.send(Event::ClientInput(line)) {
            return; // stopping
        }
    }
}

fn server_read(tx: Sender<Event>, stdout: ChildStdout) {
    let mut reader = BufReader::new(stdout);

    loop {
        let mut line = String::new();
        let bytes_read = match reader.read_line(&mut line) {
            Ok(b) => b,
            Err(err) => {
                let _ = tx.send(Event::Fail(err.into()));
                return;
            }
        };
        if bytes_read == 0 {
            let _ = tx.send(Event::ServerStop());
            break; // EOF
        }
        if let Err(_) = tx.send(Event::ServerOutput(line)) {
            return; // stopping
        }
    }
}

fn proxy(cfg: &Config, server: Server) -> anyhow::Result<()> {
    let (mut proc, mut stdin, stdout) = server;

    let (tx, rx) = channel::<Event>();
    let client_tx = tx.clone();
    let server_tx = tx.clone();
    thread::spawn(move || {
        client_read(client_tx);
    });
    thread::spawn(move || {
        server_read(server_tx, stdout);
    });

    let mut stdout = std::io::stdout();

    let mut prompt = String::new();

    'outer: for ev in rx.iter() {
        match ev {
            Event::Fail(error) => {
                let _ = proc.kill();
                return Err(error);
            }
            Event::ServerStop() => {
                let _ = proc.kill();
                if cfg.debug {
                    println!("# SERVER STOPPED");
                    stdout.flush()?;
                }
                return Ok(());
            }
            Event::ClientInput(s) => {
                if let Some(p) = s.strip_prefix("SETPROMPT ") {
                    // TODO: Usescape prompt
                    if cfg.debug {
                        println!("# SETTING PROMPT TO {}", p);
                    }
                    prompt = p.to_string();
                } else if s.starts_with("GETPIN") {
                    if cfg.debug {
                        println!("# ASKED FOR PIN");
                    }
                    if let Some(pin) = get_pin(cfg, &prompt)? {
                        println!("D {}", pin);
                        stdout.flush()?;
                        continue 'outer;
                    }
                }
                if cfg.debug {
                    print!("# PIPTING client -> server {}", s);
                    stdout.flush()?;
                }
                write!(stdin, "{}", s)?;
                stdin.flush()?;
            }
            Event::ServerOutput(s) => {
                if cfg.debug {
                    print!("# PIPTING client <- server {}", s);
                    stdout.flush()?;
                }
                print!("{}", s);
                stdout.flush()?;
            }
        }
    }
    proc.kill()?;
    Ok(())
}

fn get_pin(cfg: &Config, prompt: &str) -> anyhow::Result<Option<String>> {
    Ok(Some(format!("pin for ptompt {}", prompt)))
}

fn main() -> anyhow::Result<()> {
    println!("{}", INITMSG);
    let cfg = load()?;
    if cfg.debug {
        println!("# DEBUG MODE ON");
    }
    let srv = launch(&cfg)?;
    println!("#\n# Try to type HELP command\n#");
    proxy(&cfg, srv)?;
    Ok(())
}
