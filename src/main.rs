mod server;

use std::{
    env, fs,
    io::{self, BufRead, IsTerminal, Read, Write},
    net::{IpAddr, Ipv4Addr, SocketAddr, TcpStream},
    path::{Path, PathBuf},
    process::Command,
    time::Duration,
};

use anyhow::{Context, Result, bail};
use hq_auth::{AuthConfig, AuthService};
use hq_presets::{PresetStore, apply_preset, detect_installed_agents, run_with_temporary_preset};
use hq_remote_connect::{EndpointKind, discover_endpoints};
use hq_runtime_paths::{resolve_current_user, resolve_data_dir, resolve_workspace_dir};

const DEFAULT_ADDR: &str = "127.0.0.1:9900";
const APP_DIR_NAME: &str = ".hqcode";

enum CliCommand {
    Serve {
        addr: SocketAddr,
        data_dir: PathBuf,
        workspace: PathBuf,
        public_url: Option<String>,
        no_qr: bool,
    },
    List {
        data_dir: PathBuf,
    },
    Use {
        data_dir: PathBuf,
        selector: String,
    },
    Run {
        data_dir: PathBuf,
        selector: String,
        command: Vec<String>,
    },
    Pair {
        data_dir: PathBuf,
        addr: SocketAddr,
        endpoint_url: Option<String>,
        label: Option<String>,
        ttl_secs: Option<u64>,
    },
    Doctor {
        data_dir: PathBuf,
    },
    Passwd {
        data_dir: PathBuf,
        username: Option<String>,
        password: Option<String>,
    },
    Version,
    Help,
}

fn parse_args() -> Result<CliCommand> {
    let mut args = env::args().skip(1).peekable();
    let default_data_dir = resolve_data_dir("HQCODE_DATA_DIR", APP_DIR_NAME);
    let default_workspace = resolve_workspace_dir("HQCODE_WORKSPACE");
    let default_addr: SocketAddr = env::var("HQCODE_ADDR")
        .unwrap_or_else(|_| DEFAULT_ADDR.to_string())
        .parse()
        .context("invalid HQCODE_ADDR socket address")?;
    let default_public_url = env::var("HQCODE_PUBLIC_URL")
        .ok()
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty());

    let Some(first) = args.peek().cloned() else {
        return Ok(CliCommand::Serve {
            addr: default_addr,
            data_dir: default_data_dir,
            workspace: default_workspace,
            public_url: default_public_url,
            no_qr: false,
        });
    };

    match first.as_str() {
        "serve" | "--addr" | "--data-dir" | "--workspace" | "--lan" | "--public-url"
        | "--no-qr" => {
            if first == "serve" {
                args.next();
            }
            let mut addr = default_addr;
            let mut addr_explicit = false;
            let mut lan = false;
            let mut data_dir = default_data_dir;
            let mut workspace = default_workspace;
            let mut public_url = default_public_url;
            let mut no_qr = false;

            while let Some(flag) = args.next() {
                match flag.as_str() {
                    "--addr" => {
                        let val = args.next().context("--addr requires a value")?;
                        addr = val.parse().context("invalid --addr socket address")?;
                        addr_explicit = true;
                    }
                    "--lan" => {
                        lan = true;
                    }
                    "--public-url" => {
                        let val = args.next().context("--public-url requires a value")?;
                        public_url = Some(val);
                    }
                    "--no-qr" => {
                        no_qr = true;
                    }
                    "--data-dir" => {
                        let val = args.next().context("--data-dir requires a value")?;
                        data_dir = PathBuf::from(val);
                    }
                    "--workspace" => {
                        let val = args.next().context("--workspace requires a value")?;
                        workspace = PathBuf::from(val);
                    }
                    "-h" | "--help" => return Ok(CliCommand::Help),
                    other => bail!("unknown serve flag: {other}"),
                }
            }
            if lan && (!addr_explicit || addr.ip().is_loopback()) {
                addr = SocketAddr::new(IpAddr::V4(Ipv4Addr::UNSPECIFIED), addr.port());
            }
            Ok(CliCommand::Serve {
                addr,
                data_dir,
                workspace,
                public_url,
                no_qr,
            })
        }
        "list" => {
            args.next();
            let mut data_dir = default_data_dir;
            while let Some(flag) = args.next() {
                match flag.as_str() {
                    "--data-dir" => {
                        data_dir =
                            PathBuf::from(args.next().context("--data-dir requires a value")?);
                    }
                    "-h" | "--help" => return Ok(CliCommand::Help),
                    other => bail!("unknown list flag: {other}"),
                }
            }
            Ok(CliCommand::List { data_dir })
        }
        "use" => {
            args.next();
            let selector = args.next().context("usage: hqcode use <preset>")?;
            Ok(CliCommand::Use {
                data_dir: default_data_dir,
                selector,
            })
        }
        "run" => {
            args.next();
            let selector = args
                .next()
                .context("usage: hqcode run <preset> -- <command...>")?;
            if args.peek().map(String::as_str) == Some("--") {
                args.next();
            }
            let command: Vec<String> = args.collect();
            if command.is_empty() {
                bail!("usage: hqcode run <preset> -- <command...>");
            }
            Ok(CliCommand::Run {
                data_dir: default_data_dir,
                selector,
                command,
            })
        }
        "pair" => {
            args.next();
            let mut data_dir = default_data_dir;
            let mut addr = default_addr;
            let mut endpoint_url = default_public_url;
            let mut label = None;
            let mut ttl_secs = None;
            while let Some(flag) = args.next() {
                match flag.as_str() {
                    "--data-dir" => {
                        data_dir =
                            PathBuf::from(args.next().context("--data-dir requires a value")?);
                    }
                    "--addr" => {
                        let val = args.next().context("--addr requires a value")?;
                        addr = val.parse().context("invalid --addr socket address")?;
                    }
                    "--url" => {
                        endpoint_url = Some(args.next().context("--url requires a value")?);
                    }
                    "--label" => {
                        label = Some(args.next().context("--label requires a value")?);
                    }
                    "--ttl" => {
                        let raw = args.next().context("--ttl requires seconds")?;
                        ttl_secs = Some(raw.parse::<u64>().context("invalid --ttl integer")?);
                    }
                    "-h" | "--help" => return Ok(CliCommand::Help),
                    other => bail!("unknown pair flag: {other}"),
                }
            }
            Ok(CliCommand::Pair {
                data_dir,
                addr,
                endpoint_url,
                label,
                ttl_secs,
            })
        }
        "doctor" => {
            args.next();
            let mut data_dir = default_data_dir;
            while let Some(flag) = args.next() {
                match flag.as_str() {
                    "--data-dir" => {
                        data_dir =
                            PathBuf::from(args.next().context("--data-dir requires a value")?);
                    }
                    "-h" | "--help" => return Ok(CliCommand::Help),
                    other => bail!("unknown doctor flag: {other}"),
                }
            }
            Ok(CliCommand::Doctor { data_dir })
        }
        "passwd" => {
            args.next();
            let mut data_dir = default_data_dir;
            let mut username = None;
            let mut password = None;
            while let Some(arg) = args.next() {
                match arg.as_str() {
                    "--data-dir" => {
                        data_dir =
                            PathBuf::from(args.next().context("--data-dir requires a value")?);
                    }
                    "--username" => {
                        username = Some(args.next().context("--username requires a value")?);
                    }
                    "-h" | "--help" => return Ok(CliCommand::Help),
                    other if other.starts_with('-') => bail!("unknown passwd flag: {other}"),
                    val => {
                        if password.is_none() {
                            password = Some(val.to_string());
                        } else {
                            bail!("unexpected extra argument: {val}");
                        }
                    }
                }
            }
            Ok(CliCommand::Passwd {
                data_dir,
                username,
                password,
            })
        }
        "version" | "-V" | "--version" => Ok(CliCommand::Version),
        "help" | "-h" | "--help" => Ok(CliCommand::Help),
        other => bail!("unknown command `{other}` (run `hqcode help`)"),
    }
}

fn print_help() {
    println!(
        "HQCode {} — Persistent tmux control plane for Coding Agents\n\n\
USAGE:\n  \
  hqcode [serve] [--lan] [--addr HOST:PORT] [--public-url URL] [--no-qr] [--workspace DIR] [--data-dir DIR]\n  \
  hqcode pair [--url BASE_URL] [--ttl SECS] [--label TEXT] [--addr HOST:PORT] [--data-dir DIR]\n  \
  hqcode doctor [--data-dir DIR]\n  \
  hqcode passwd [NEW_PASSWORD] [--username USER] [--data-dir DIR]\n  \
  hqcode list [--data-dir DIR]\n  \
  hqcode use <preset-id-name-or-model>\n  \
  hqcode run <preset-id-name-or-model> -- <command...>\n  \
  hqcode version\n",
        env!("CARGO_PKG_VERSION")
    );
}

fn run_doctor(data_dir: &Path) {
    println!("HQCode {} Environment Check\n", env!("CARGO_PKG_VERSION"));
    let tmux_ver = command_version("tmux", &["-V"]);
    println!(
        "  [{}] tmux: {}",
        if tmux_ver.is_some() { "OK" } else { "MISSING" },
        tmux_ver
            .as_deref()
            .unwrap_or("not found in PATH (required for terminal sessions)")
    );
    let git_ver = command_version("git", &["--version"]);
    println!(
        "  [{}] git: {}",
        if git_ver.is_some() { "OK" } else { "WARN" },
        git_ver
            .as_deref()
            .unwrap_or("not found in PATH (Git Diff drawer disabled)")
    );

    for agent in detect_installed_agents() {
        let status = if agent.available { "OK" } else { "SKIP" };
        let detail = if agent.available {
            agent.path
        } else {
            "not installed".to_string()
        };
        println!("  [{status}] {}: {detail}", agent.label);
    }

    let in_path = env::current_exe()
        .ok()
        .and_then(|exe| exe.parent().map(Path::to_path_buf))
        .is_some_and(|dir| {
            env::var_os("PATH")
                .map(|p| env::split_paths(&p).any(|entry| entry == dir))
                .unwrap_or(false)
        });
    println!(
        "  [{}] binary dir in PATH: {}",
        if in_path { "OK" } else { "WARN" },
        if in_path {
            "yes".to_string()
        } else {
            "add install dir to PATH so `hqcode` works everywhere".to_string()
        }
    );

    println!("  [INFO] data dir: {}", data_dir.display());
    let endpoints = discover_endpoints(
        SocketAddr::new(IpAddr::V4(Ipv4Addr::UNSPECIFIED), 9900),
        None,
    );
    let remote_eps: Vec<_> = endpoints
        .into_iter()
        .filter(|ep| ep.kind != EndpointKind::Loopback)
        .collect();
    if remote_eps.is_empty() {
        println!("  [INFO] LAN/Tailscale IPs: none detected (loopback only)");
    } else {
        println!("  [INFO] Phone-reachable candidates (when started with `--lan`):");
        for ep in remote_eps {
            println!("         - {}: {}", ep.label, ep.url);
        }
    }
}

fn command_version(bin: &str, args: &[&str]) -> Option<String> {
    let out = Command::new(bin).args(args).output().ok()?;
    if !out.status.success() {
        return None;
    }
    let text = String::from_utf8_lossy(&out.stdout).trim().to_string();
    if text.is_empty() {
        Some("available".to_string())
    } else {
        Some(text)
    }
}

async fn run_passwd(
    data_dir: &Path,
    username: Option<String>,
    password: Option<String>,
) -> Result<()> {
    let resolved_pw = match password {
        Some(pw) => pw,
        None => {
            if io::stdin().is_terminal() {
                eprint!("New password (min 8 chars): ");
                let _ = io::stderr().flush();
            }
            let mut line = String::new();
            io::stdin()
                .lock()
                .read_line(&mut line)
                .context("failed to read password from stdin")?;
            line.trim_end_matches(['\r', '\n']).to_string()
        }
    };

    let auth = AuthService::initialize(data_dir, AuthConfig::for_app("hqcode"))?;
    let updated_user = auth
        .reset_password_direct(username.as_deref(), &resolved_pw)
        .await?;
    println!(
        "Updated HQCode login credentials in {} for user `{}`.",
        data_dir.display(),
        updated_user
    );
    Ok(())
}

fn run_pair(
    data_dir: &Path,
    addr: SocketAddr,
    endpoint_url: Option<String>,
    label: Option<String>,
    ttl_secs: Option<u64>,
) -> Result<()> {
    let token_path = data_dir.join(".hqcode-local-token");
    let token = fs::read_to_string(&token_path)
        .with_context(|| {
            format!(
                "missing local API token at {} (start `hqcode serve --lan` first)",
                token_path.display()
            )
        })?
        .trim()
        .to_string();

    let connect_addr = if addr.ip().is_unspecified() {
        SocketAddr::new(IpAddr::V4(Ipv4Addr::LOCALHOST), addr.port())
    } else {
        addr
    };

    let body = serde_json::json!({
        "endpoint_url": endpoint_url,
        "label": label,
        "ttl_secs": ttl_secs,
    })
    .to_string();

    let mut stream = TcpStream::connect_timeout(&connect_addr, Duration::from_secs(3))
        .with_context(|| {
            format!(
                "could not connect to running HQCode server at {connect_addr} (run `hqcode serve --lan` first)"
            )
        })?;
    stream.set_read_timeout(Some(Duration::from_secs(5)))?;
    stream.set_write_timeout(Some(Duration::from_secs(5)))?;

    let req = format!(
        "POST /api/remote/tickets HTTP/1.1\r\n\
Host: {connect_addr}\r\n\
Authorization: Bearer {token}\r\n\
Content-Type: application/json\r\n\
Content-Length: {}\r\n\
Connection: close\r\n\r\n\
{body}",
        body.len()
    );
    stream.write_all(req.as_bytes())?;

    let mut raw = Vec::new();
    stream.read_to_end(&mut raw)?;
    let response = String::from_utf8_lossy(&raw);
    let (head, resp_body) = response
        .split_once("\r\n\r\n")
        .or_else(|| response.split_once("\n\n"))
        .context("invalid HTTP response from local HQCode server")?;
    let status_line = head.lines().next().unwrap_or("");
    if !status_line.contains(" 200 ") {
        bail!("failed to issue pairing ticket ({status_line}): {resp_body}");
    }

    let payload: serde_json::Value = serde_json::from_str(resp_body)
        .context("invalid JSON response from /api/remote/tickets")?;
    let pair_url = payload
        .pointer("/ticket/pair_url")
        .and_then(|v| v.as_str())
        .unwrap_or("");
    let expires_in = payload
        .pointer("/ticket/expires_in_secs")
        .and_then(|v| v.as_u64())
        .unwrap_or(300);
    let qr_terminal = payload
        .get("qr_terminal")
        .and_then(|v| v.as_str())
        .unwrap_or("");

    println!("\nScan to pair phone/tablet (one-time link, expires in {expires_in}s):");
    println!("{qr_terminal}");
    println!("Pair URL: {pair_url}\n");
    Ok(())
}

#[tokio::main]
async fn main() -> Result<()> {
    match parse_args()? {
        CliCommand::Help => {
            print_help();
            Ok(())
        }
        CliCommand::Version => {
            println!("hqcode {}", env!("CARGO_PKG_VERSION"));
            Ok(())
        }
        CliCommand::Doctor { data_dir } => {
            run_doctor(&data_dir);
            Ok(())
        }
        CliCommand::Passwd {
            data_dir,
            username,
            password,
        } => run_passwd(&data_dir, username, password).await,
        CliCommand::Pair {
            data_dir,
            addr,
            endpoint_url,
            label,
            ttl_secs,
        } => run_pair(&data_dir, addr, endpoint_url, label, ttl_secs),
        CliCommand::List { data_dir } => {
            let store = PresetStore::load(&data_dir, "hqcode");
            for preset in store.list() {
                println!(
                    "{}\t{}\t{:?}\t{}",
                    preset.id, preset.name, preset.kind, preset.model
                );
            }
            Ok(())
        }
        CliCommand::Use { data_dir, selector } => {
            let store = PresetStore::load(&data_dir, "hqcode");
            let preset = store
                .resolve(&selector)
                .with_context(|| format!("preset not found: {selector}"))?;
            let user = resolve_current_user().context("failed to resolve current OS user")?;
            let home = hq_runtime_paths::resolve_user_home_preferring_env(&user.name)
                .unwrap_or_else(|_| user.home.clone());
            apply_preset(&home, &preset)?;
            println!("Applied preset `{}` ({})", preset.name, preset.model);
            Ok(())
        }
        CliCommand::Run {
            data_dir,
            selector,
            command,
        } => {
            let store = PresetStore::load(&data_dir, "hqcode");
            let preset = store
                .resolve(&selector)
                .with_context(|| format!("preset not found: {selector}"))?;
            let user = resolve_current_user().context("failed to resolve current OS user")?;
            let home = hq_runtime_paths::resolve_user_home_preferring_env(&user.name)
                .unwrap_or_else(|_| user.home.clone());
            let code =
                run_with_temporary_preset(&user, &home, &preset, &command, Duration::from_secs(4))?;
            std::process::exit(code);
        }
        CliCommand::Serve {
            addr,
            data_dir,
            workspace,
            public_url,
            no_qr,
        } => server::serve(addr, data_dir, workspace, public_url, no_qr).await,
    }
}
