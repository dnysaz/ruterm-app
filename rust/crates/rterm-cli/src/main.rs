use std::io::{IsTerminal, Read, Write};
use std::path::PathBuf;

use anyhow::Context;
use base64::{engine::general_purpose::STANDARD as B64, Engine};
use clap::{Args, Parser, Subcommand};
use rterm_core::crypto;
use rterm_core::model::{AuthMethod, Host, Identity, IdentityKind, Vault};
use rterm_core::ssh::{self, HostKeyPolicy, SshEvent, SshTarget, TermSize};
use uuid::Uuid;

#[derive(Parser)]
#[command(name = "rterm", version, about = "rterm - SSH client engine (CLI test harness)")]
struct Cli {
    #[command(subcommand)]
    cmd: Cmd,
}

#[derive(Subcommand)]
enum Cmd {
    /// Run a command on a remote host over SSH.
    Exec(ExecArgs),
    /// Open an interactive shell (Ctrl-] to quit).
    Shell(ShellArgs),
    /// Generate a new SSH key pair.
    Keygen {
        #[arg(long, default_value = "ed25519")]
        kind: String,
        /// Write the private key to this file instead of stdout.
        #[arg(long)]
        out: Option<PathBuf>,
    },
    /// Seal a vault JSON with a passphrase (same envelope the apps use).
    Seal {
        #[arg(long)]
        input: PathBuf,
        #[arg(long)]
        output: PathBuf,
        #[arg(long, env = "RTERM_VAULT_PASSPHRASE")]
        passphrase: Option<String>,
    },
}

#[derive(Args)]
struct CommonArgs {
    host: String,
    #[arg(short, long, default_value_t = 22)]
    port: u16,
    #[arg(short, long, default_value = "root")]
    user: String,
    #[arg(long, env = "RTERM_PASSWORD")]
    password: Option<String>,
    #[arg(short = 'k', long)]
    key: Option<PathBuf>,
    #[arg(long, env = "RTERM_KEY_PASSPHRASE")]
    key_passphrase: Option<String>,
    /// Skip host key verification (first connection only!).
    #[arg(long)]
    insecure: bool,
    #[arg(long)]
    fingerprint: Option<String>,
    #[arg(long, default_value = "xterm-256color")]
    term: String,
}

#[derive(Args)]
struct ExecArgs {
    #[command(flatten)]
    conn: CommonArgs,
    /// Command to run remotely.
    #[arg(trailing_var_arg = true, required = true)]
    command: Vec<String>,
}

#[derive(Args)]
struct ShellArgs {
    #[command(flatten)]
    conn: CommonArgs,
}

fn build_target(args: &CommonArgs) -> anyhow::Result<SshTarget> {
    let (auth, identity) = if let Some(path) = &args.key {
        let pem = std::fs::read_to_string(path)
            .with_context(|| format!("cannot read key {}", path.display()))?;
        let id = Uuid::new_v4();
        let identity = Identity {
            id,
            label: format!("cli:{}", path.display()),
            private_key: pem,
            passphrase: args.key_passphrase.clone(),
            kind: Some(IdentityKind::Imported),
            fingerprint: None,
            created_at: rterm_core::model::now(),
            updated_at: rterm_core::model::now(),
            deleted: false,
        };
        (
            AuthMethod::Key {
                identity_id: id,
                passphrase: args.key_passphrase.clone(),
            },
            Some(identity),
        )
    } else {
        let password = match &args.password {
            Some(p) => p.clone(),
            None => rpassword::prompt_password("SSH password: ")?,
        };
        (AuthMethod::Password { password }, None)
    };

    let host = Host {
        id: Uuid::new_v4(),
        label: args.host.clone(),
        address: args.host.clone(),
        port: args.port,
        username: args.user.clone(),
        auth,
        group_id: None,
        tags: vec![],
        color: None,
        icon: None,
        favorite: false,
        notes: String::new(),
        startup_snippet_id: None,
        jump_host_id: None,
        keepalive_secs: Some(30),
        connect_timeout_secs: Some(15),
        env: vec![],
        terminal: Default::default(),
        agent_forwarding: false,
        compression: false,
        os: None,
        host_key_fingerprint: args.fingerprint.clone(),
        created_at: rterm_core::model::now(),
        updated_at: rterm_core::model::now(),
        last_used_at: None,
        deleted: false,
    };

    let mut target = SshTarget::from_host(&host, identity.as_ref());
    target.term = args.term.clone();
    if args.insecure {
        target.host_key_policy = HostKeyPolicy::AcceptAll;
    }
    Ok(target)
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| "rterm=warn".into()),
        )
        .init();

    match Cli::parse().cmd {
        Cmd::Exec(args) => {
            let target = build_target(&args.conn)?;
            let command = args.command.join(" ");
            let out = ssh::exec_once(target, &command).await?;
            let mut stdout = std::io::stdout();
            stdout.write_all(&out.stdout)?;
            std::io::stderr().write_all(&out.stderr)?;
            std::process::exit(out.exit_status.unwrap_or(0) as i32);
        }
        Cmd::Shell(args) => {
            let target = build_target(&args.conn)?;
            interactive_shell(target).await?;
        }
        Cmd::Keygen { kind, out } => {
            let generated = ssh::generate_key(&kind)?;
            match out {
                Some(path) => {
                    std::fs::write(&path, &generated.private_key)?;
                    #[cfg(unix)]
                    {
                        use std::os::unix::fs::PermissionsExt;
                        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600))?;
                    }
                    println!("{}", generated.public_key);
                    eprintln!("fingerprint: {}", generated.fingerprint);
                    eprintln!("private key: {}", path.display());
                }
                None => {
                    print!("{}", generated.private_key);
                    println!("{}", generated.public_key);
                    eprintln!("fingerprint: {}", generated.fingerprint);
                }
            }
        }
        Cmd::Seal {
            input,
            output,
            passphrase,
        } => {
            let vault: Vault = serde_json::from_slice(&std::fs::read(&input)?)?;
            let passphrase = match passphrase {
                Some(p) => p,
                None => rpassword::prompt_password("Vault passphrase: ")?,
            };
            let (sealed, _key) = crypto::seal_vault_with_passphrase(&vault, &passphrase)?;
            std::fs::write(&output, serde_json::to_vec_pretty(&sealed)?)?;
            println!("sealed -> {}", output.display());
        }
    }
    Ok(())
}

async fn interactive_shell(target: SshTarget) -> anyhow::Result<()> {
    let size = TermSize { cols: 80, rows: 24 };
    let mut session = ssh::open_shell(target, size).await?;
    eprintln!(
        "connected (host key {}). Ctrl-] to quit.",
        session.fingerprint
    );

    // Raw mode only makes sense on a real terminal. With redirected stdin
    // (`echo ls | rterm shell …`) we forward plain bytes instead.
    let is_tty = std::io::stdin().is_terminal();
    if is_tty {
        crossterm::terminal::enable_raw_mode()?;
    }
    let (tx, mut rx) = tokio::sync::mpsc::channel::<Vec<u8>>(256);
    std::thread::spawn(move || {
        let mut stdin = std::io::stdin();
        let mut buf = [0u8; 4096];
        loop {
            match stdin.read(&mut buf) {
                Ok(0) => break,
                Ok(n) => {
                    if tx.blocking_send(buf[..n].to_vec()).is_err() {
                        break;
                    }
                }
                Err(_) => break,
            }
        }
    });

    let mut out = std::io::stdout();
    let result: anyhow::Result<()> = async {
        loop {
            tokio::select! {
                input = rx.recv() => {
                    match input {
                        Some(bytes) => {
                            if bytes.contains(&0x1d) {
                                break;
                            }
                            session.write(bytes);
                        }
                        None => {
                            // stdin closed (pipe/EOF): keep draining the remote
                            // side until the shell exits by itself.
                        }
                    }
                }
                ev = session.next_event() => {
                    match ev {
                        Some(SshEvent::Data { data_b64 }) | Some(SshEvent::Stderr { data_b64 }) => {
                            out.write_all(&B64.decode(data_b64)?)?;
                            out.flush()?;
                        }
                        Some(SshEvent::Connected { fingerprint }) => {
                            eprintln!("\r\n[host key {fingerprint}]");
                        }
                        Some(SshEvent::Error { message }) => {
                            eprintln!("\r\n[error] {message}");
                        }
                        Some(SshEvent::ExitStatus { code }) => {
                            eprintln!("\r\n[exit {code}]");
                            break;
                        }
                        Some(SshEvent::Closed { .. }) => break,
                        Some(_) => {}
                        None => break,
                    }
                }
            }
        }
        Ok(())
    }
    .await;

    if is_tty {
        crossterm::terminal::disable_raw_mode()?;
    }
    session.request_close().await;
    println!();
    result
}
