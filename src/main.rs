use anyhow::anyhow;
use regex::Regex;
use serde::Deserialize;
use std::{
    collections::HashMap,
    fs::{self, File},
    io::{BufRead, BufReader, Write, stderr, stdin},
    os::unix::fs::PermissionsExt,
    path::Path,
    process::{Child, ChildStdin, ChildStdout, Command, Stdio},
    sync::mpsc::{Receiver, Sender, channel},
    thread,
};
use users::{get_current_uid, get_user_by_uid};

const INITMSG: &str = r#"# PineEntry
# A GNU pinentry caching proxy
#
# Src: https://github.com/asciimoth/pineentry
# Config file: ~/.config/pineentry/config.yaml or alt with $PINEENTRY_CFG
#"#;

#[derive(Deserialize, Debug, Clone)]
enum PinSrc {
    // String value hardcoded in config.
    String(String),
    // File to read.
    // NOTE: File content will NOT be trimmed.
    RoFile(String),
    // Ask at first time and remember in file with provided path or in tmpdir.
    // NOTE: It cache any input even one rejected by client.
    Cache(Option<String>),
    // Read pin from env var
    Env(String),
}

#[derive(Deserialize, Debug, Clone)]
struct Rule {
    name: String,
    // Title regexp
    title: Option<String>,
    // Prompt regexp
    prompt: Option<String>,
    // PinSrc name
    src: String,
}

#[derive(Deserialize, Debug, Clone)]
struct Config {
    #[serde(default)]
    debug: bool,
    #[serde(default)]
    servers: Vec<String>,
    #[serde(default)]
    pins: HashMap<String, PinSrc>,
    #[serde(default)]
    rules: Vec<Rule>,
}

impl Config {
    fn match_rule(&self, title: &str, prompt: &str) -> Option<Rule> {
        'outer: for rule in &self.rules {
            if let Some(tr) = &rule.title {
                let tr = match Regex::new(tr) {
                    Ok(tr) => tr,
                    Err(err) => {
                        println!("# BROKEN TITLE REGEXP IN RULE {}: {}", rule.name, err);
                        continue 'outer;
                    }
                };
                if tr.is_match(title) {
                    println!("# MATCHING RULE {} with src {}", rule.name, rule.src);
                    return Some(rule.clone());
                }
            }
            if let Some(pr) = &rule.prompt {
                let pr = match Regex::new(pr) {
                    Ok(pr) => pr,
                    Err(err) => {
                        println!("# BROKEN PROMPT REGEXP IN RULE {}: {}", rule.name, err);
                        continue 'outer;
                    }
                };
                if pr.is_match(prompt) {
                    println!("# MATCHING RULE {} with src {}", rule.name, rule.src);
                    return Some(rule.clone());
                }
            }
        }
        None
    }
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
    let default_servers = vec![
        String::from("pinentry-qt"),
        String::from("pinentry-gtk"),
        String::from("pinentry-curses"),
        String::from("pinentry-tty"),
    ];

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
                servers: default_servers,
                pins: HashMap::new(),
                rules: vec![],
            });
        }
    };
    let mut cfg: Config = serde_yaml::from_str(&yaml_content)?;
    if cfg.servers.is_empty() {
        cfg.servers = default_servers;
    }
    Ok(cfg)
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
    let mut title = String::new();

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
                if let Some(rp) = s.strip_prefix("SETPROMPT ") {
                    let p = unescape(rp);
                    if cfg.debug {
                        print!("# SETTING PROMPT TO {}", rp);
                        stdout.flush()?;
                    }
                    prompt = p;
                } else if let Some(rt) = s.strip_prefix("SETTITLE ") {
                    let t = unescape(rt);
                    if cfg.debug {
                        print!("# SETTING TITLE TO {}", rt);
                        stdout.flush()?;
                    }
                    title = t;
                } else if s.starts_with("GETPIN") {
                    if cfg.debug {
                        println!("# ASKED FOR PIN");
                    }
                    if let Some(pin) = get_pin(cfg, &prompt, &title, &rx, &stdin)? {
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

fn escape(s: &str) -> String {
    let mut result = String::with_capacity(s.len());
    for c in s.chars() {
        match c {
            '%' => result.push_str("%25"),
            '\r' => result.push_str("%0D"),
            '\n' => result.push_str("%0A"),
            _ => result.push(c),
        }
    }
    result
}

fn unescape(raw: &str) -> String {
    // Trim trailing CR and LF
    let raw = raw.trim_end_matches(|c| c == '\r' || c == '\n');

    let mut result = String::with_capacity(raw.len());
    let mut chars = raw.chars().peekable();

    while let Some(c) = chars.next() {
        if c == '%' {
            // Decode percent-encoded sequence
            let hex: String = chars.by_ref().take(2).collect();
            if hex.len() == 2 {
                if let Ok(byte_val) = u8::from_str_radix(&hex, 16) {
                    result.push(byte_val as char);
                } else {
                    // Invalid hex, keep original
                    result.push('%');
                    result.push_str(&hex);
                }
            } else {
                // Incomplete sequence, keep original
                result.push('%');
                result.push_str(&hex);
            }
        } else {
            result.push(c);
        }
    }

    result
}

fn ensure_parent_dirs(path: &str) -> std::io::Result<()> {
    if let Some(parent) = Path::new(path).parent() {
        fs::create_dir_all(parent)?;
        let permissions = fs::Permissions::from_mode(0o700);
        if let Err(_) = fs::set_permissions(parent, permissions) {
            println!("# FAILED TO SET 700 PERM for {}", parent.to_string_lossy());
        }
    }
    Ok(())
}

fn ask_pin(rx: &Receiver<Event>, mut stdin: &ChildStdin) -> anyhow::Result<String> {
    let err = anyhow!("Failed to get pin from server");
    writeln!(stdin, "GETPIN")?;
    stdin.flush()?;
    let mut pin: Option<String> = None;
    for event in rx {
        let resp = match event {
            Event::ServerOutput(resp) => resp,
            _ => {
                return Err(err);
            }
        };
        if let Some(d) = resp.strip_prefix("D ") {
            pin = Some(unescape(d));
            break;
        }
    }
    Ok(match pin {
        Some(pin) => pin,
        None => {
            return Err(err);
        }
    })
}

fn get_pin(
    cfg: &Config,
    prompt: &str,
    title: &str,
    rx: &Receiver<Event>,
    stdin: &ChildStdin,
) -> anyhow::Result<Option<String>> {
    let rule = match cfg.match_rule(title, prompt) {
        Some(rule) => rule,
        None => {
            return Ok(None);
        }
    };
    let src = match cfg.pins.get(&rule.src) {
        Some(pin) => pin,
        None => {
            return Ok(None);
        }
    };
    let pin = match src {
        PinSrc::String(s) => s.clone(),
        PinSrc::RoFile(path) => fs::read_to_string(path)?,
        PinSrc::Env(var) => std::env::var(var)?,
        PinSrc::Cache(path) => {
            let path = match path {
                Some(path) => path,
                None => &std::env::temp_dir()
                    .join(format!(
                        "pinentry-{}",
                        get_user_by_uid(get_current_uid())
                            .ok_or(anyhow!("Failed to get current user"))?
                            .name()
                            .to_string_lossy()
                    ))
                    .join(rule.src)
                    .to_string_lossy()
                    .to_string(),
            };
            let pin = match fs::read_to_string(path) {
                Ok(pin) => pin,
                Err(_) => ask_pin(rx, stdin)?,
            };

            ensure_parent_dirs(&path)?;
            let mut cache = File::create(path)?;
            write!(cache, "{}", pin)?;
            drop(cache);
            let permissions = fs::Permissions::from_mode(0o600);
            if let Err(_) = fs::set_permissions(path, permissions) {
                println!("# FAILED TO SET 700 PERM for {}", path);
            }
            pin
        }
    };
    Ok(Some(escape(&pin)))
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
