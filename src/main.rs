mod server;

use std::{env, net::SocketAddr, path::PathBuf, time::Duration};

use anyhow::{Context, Result, bail};
use hq_presets::{PresetStore, apply_preset, run_with_temporary_preset};
use hq_runtime_paths::{resolve_current_user, resolve_data_dir, resolve_workspace_dir};

const DEFAULT_ADDR: &str = "127.0.0.1:9900";
const APP_DIR_NAME: &str = ".hqcode";

enum CliCommand {
    Serve {
        addr: SocketAddr,
        data_dir: PathBuf,
        workspace: PathBuf,
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

    let Some(first) = args.peek().cloned() else {
        return Ok(CliCommand::Serve {
            addr: default_addr,
            data_dir: default_data_dir,
            workspace: default_workspace,
        });
    };

    match first.as_str() {
        "serve" | "--addr" | "--data-dir" | "--workspace" => {
            if first == "serve" {
                args.next();
            }
            let mut addr = default_addr;
            let mut data_dir = default_data_dir;
            let mut workspace = default_workspace;
            while let Some(flag) = args.next() {
                match flag.as_str() {
                    "--addr" => {
                        let val = args.next().context("--addr requires a value")?;
                        addr = val.parse().context("invalid --addr socket address")?;
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
            Ok(CliCommand::Serve {
                addr,
                data_dir,
                workspace,
            })
        }
        "list" => Ok(CliCommand::List {
            data_dir: default_data_dir,
        }),
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
        "version" | "-V" | "--version" => Ok(CliCommand::Version),
        "help" | "-h" | "--help" => Ok(CliCommand::Help),
        other => bail!("unknown command `{other}` (run `hqcode help`)"),
    }
}

fn print_help() {
    println!(
        "HQCode {} — Persistent tmux control plane for Coding Agents\n\n\
USAGE:\n  \
  hqcode [serve] [--addr HOST:PORT] [--workspace DIR] [--data-dir DIR]\n  \
  hqcode list\n  \
  hqcode use <preset-id-name-or-model>\n  \
  hqcode run <preset-id-name-or-model> -- <command...>\n  \
  hqcode version\n",
        env!("CARGO_PKG_VERSION")
    );
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
        } => server::serve(addr, data_dir, workspace).await,
    }
}
