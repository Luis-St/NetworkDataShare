//! Command line, environment and config file handling, plus the `--info` and `--setup` modes.
//!
//! A value is taken from the first place that sets it: command line, `NDS_*` environment
//! variable, config file, built-in default.

use std::net::SocketAddr;
use std::path::{Path, PathBuf};

use clap::parser::ValueSource;
use clap::{ArgMatches, Args, CommandFactory, FromArgMatches, Parser};
use serde::Deserialize;

const APP_DIR: &str = "network-data-share";
const SERVICE_NAME: &str = "network-data-share.service";

const AFTER_HELP: &str = "\
Configuration is read from the command line, then NDS_* environment variables, then the config file
(default ~/.config/network-data-share/config.toml), then the built-in defaults.
Run with --setup to write the config file, generate the certificate and install a systemd user service.";

#[derive(Parser, Debug)]
#[command(version, about = "Share text, images and files between devices on the local network", after_help = AFTER_HELP)]
struct Cli {
    #[command(flatten)]
    config: Config,

    /// Config file to read, or with --setup to write [default: ~/.config/network-data-share/config.toml].
    #[arg(long = "config", env = "NDS_CONFIG", value_name = "FILE")]
    config_file: Option<PathBuf>,

    /// Print a short description of the application and the configuration in effect, then exit.
    #[arg(long, conflicts_with = "setup")]
    info: bool,

    /// Write the given options (defaults for all others) to the config file, generate the certificate
    /// and install and start a systemd user service, then exit.
    #[arg(long)]
    setup: bool,

    /// With --setup: do not install the systemd user service.
    #[arg(long, requires = "setup")]
    no_service: bool,
}

#[derive(Args, Debug, Clone)]
pub struct Config {
    /// Address and port to listen on.
    #[arg(long, env = "NDS_BIND", default_value = "0.0.0.0:8080")]
    pub bind: SocketAddr,

    /// Seconds until an unanswered share invite expires.
    #[arg(long, env = "NDS_INVITE_TIMEOUT", default_value_t = 60, value_name = "SECS")]
    pub invite_timeout: u64,

    /// Seconds a disconnected client may take to reconnect before its sessions are closed.
    #[arg(long, env = "NDS_RECONNECT_GRACE", default_value_t = 10, value_name = "SECS")]
    pub reconnect_grace: u64,

    /// Serve plain HTTP instead of HTTPS (clipboard copy and notifications then only work on localhost).
    #[arg(long, env = "NDS_HTTP")]
    pub http: bool,

    /// PEM certificate to use instead of the generated self-signed one (requires --key).
    #[arg(long, env = "NDS_CERT", value_name = "FILE")]
    pub cert: Option<PathBuf>,

    /// PEM private key matching --cert.
    #[arg(long, env = "NDS_KEY", value_name = "FILE")]
    pub key: Option<PathBuf>,

    /// Directory where the generated certificate is stored [default: ~/.local/share/network-data-share].
    #[arg(long, env = "NDS_DATA_DIR", value_name = "DIR")]
    pub data_dir: Option<PathBuf>,

    /// Maximum number of clients (browser tabs) per IP address.
    #[arg(long, env = "NDS_MAX_CLIENTS_PER_IP", default_value_t = 32, value_name = "N")]
    pub max_clients_per_ip: usize,
}

impl Config {
    pub fn data_dir(&self) -> PathBuf {
        self.data_dir
            .clone()
            .unwrap_or_else(|| dirs::data_local_dir().map(|d| d.join(APP_DIR)).unwrap_or_else(|| PathBuf::from("data")))
    }
}

/// The config file: every key is optional and named like its command line option.
#[derive(Deserialize, Default)]
#[serde(deny_unknown_fields, rename_all = "kebab-case")]
struct FileConfig {
    bind: Option<SocketAddr>,
    invite_timeout: Option<u64>,
    reconnect_grace: Option<u64>,
    http: Option<bool>,
    cert: Option<PathBuf>,
    key: Option<PathBuf>,
    data_dir: Option<PathBuf>,
    max_clients_per_ip: Option<usize>,
}

pub enum Mode {
    Serve(Config),
    Exit(i32),
}

/// Parses the command line and handles `--info` and `--setup`. Returns the config to serve with.
pub fn load() -> Mode {
    let matches = Cli::command().get_matches();
    let cli = match Cli::from_arg_matches(&matches) {
        Ok(c) => c,
        Err(e) => e.exit(),
    };
    let explicit = cli.config_file.is_some();
    let path = cli.config_file.clone().unwrap_or_else(default_config_file);

    if cli.setup {
        // Setup stores what was passed and the defaults for everything else, not an older file.
        return Mode::Exit(match setup(cli.config, &path, !cli.no_service) {
            Ok(()) => 0,
            Err(e) => {
                eprintln!("setup failed: {e}");
                1
            }
        });
    }

    let mut config = cli.config;
    let loaded = match read_file(&path, explicit) {
        Ok(Some(file)) => {
            merge(&mut config, file, &matches);
            true
        }
        Ok(None) => false,
        Err(e) => {
            eprintln!("{e}");
            return Mode::Exit(1);
        }
    };

    if cli.info {
        info(&config, &path, loaded);
        return Mode::Exit(0);
    }
    Mode::Serve(config)
}

fn default_config_file() -> PathBuf {
    dirs::config_dir().map(|d| d.join(APP_DIR)).unwrap_or_else(|| PathBuf::from(".")).join("config.toml")
}

/// A missing default file is fine, a missing file that was asked for explicitly is an error.
fn read_file(path: &Path, explicit: bool) -> Result<Option<FileConfig>, String> {
    let text = match std::fs::read_to_string(path) {
        Ok(t) => t,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound && !explicit => return Ok(None),
        Err(e) => return Err(format!("cannot read config file {}: {e}", path.display())),
    };
    toml::from_str(&text).map(Some).map_err(|e| format!("invalid config file {}: {e}", path.display()))
}

/// Applies file values to every option that was not set on the command line or in the environment.
fn merge(config: &mut Config, file: FileConfig, matches: &ArgMatches) {
    let unset = |id: &str| !matches!(matches.value_source(id), Some(ValueSource::CommandLine | ValueSource::EnvVariable));
    macro_rules! take {
        ($field:ident) => {
            if let Some(v) = file.$field
                && unset(stringify!($field))
            {
                config.$field = v;
            }
        };
        ($field:ident, optional) => {
            if file.$field.is_some() && unset(stringify!($field)) {
                config.$field = file.$field;
            }
        };
    }
    take!(bind);
    take!(invite_timeout);
    take!(reconnect_grace);
    take!(http);
    take!(cert, optional);
    take!(key, optional);
    take!(data_dir, optional);
    take!(max_clients_per_ip);
}

fn info(config: &Config, path: &Path, loaded: bool) {
    println!("NetworkDataShare {}", env!("CARGO_PKG_VERSION"));
    println!();
    println!("Shares text, images and files between devices in the local network through the browser.");
    println!("Open the page on two devices, send a share invite from one and accept it on the other,");
    println!("then a chat-like session opens. Files are piped from sender to receiver, nothing is stored.");
    println!();
    println!("Config file:     {}{}", path.display(), if loaded { "" } else { " (not present, defaults used)" });
    println!("Listening on:    {}://{}", if config.http { "http" } else { "https" }, config.bind);
    println!("Invite timeout:  {} s", config.invite_timeout);
    println!("Reconnect grace: {} s", config.reconnect_grace);
    println!("Clients per IP:  {}", config.max_clients_per_ip);
    if config.http {
        println!("Certificate:     none, plain HTTP");
    } else if let Some(cert) = &config.cert {
        println!("Certificate:     {}", cert.display());
    } else {
        println!("Certificate:     self-signed, in {}", config.data_dir().display());
    }
    println!();
    println!("Run with --help for all options and --setup to install it.");
}

fn setup(mut config: Config, path: &Path, service: bool) -> Result<(), Box<dyn std::error::Error>> {
    // The service runs in another working directory, so relative paths are resolved now.
    for p in [&mut config.cert, &mut config.key, &mut config.data_dir].into_iter().flatten() {
        *p = std::path::absolute(&*p)?;
    }
    let path = std::path::absolute(path)?;

    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir)?;
    }
    std::fs::write(&path, render(&config)?)?;
    println!("Wrote config file {}", path.display());

    if config.http {
        println!("Plain HTTP configured, no certificate needed.");
    } else {
        let (cert, _) = crate::tls::certificate(&config)?;
        println!("Certificate ready: {}", cert.display());
    }

    if service {
        install_service(&path)?;
    }
    Ok(())
}

fn render(config: &Config) -> Result<String, Box<dyn std::error::Error>> {
    fn path(p: &Path) -> Result<String, Box<dyn std::error::Error>> {
        let s = p.to_str().ok_or_else(|| format!("path is not valid UTF-8: {}", p.display()))?;
        Ok(toml::Value::String(s.to_string()).to_string())
    }
    fn optional(key: &str, value: &Option<PathBuf>, example: &str) -> Result<String, Box<dyn std::error::Error>> {
        Ok(match value {
            Some(p) => format!("{key} = {}\n", path(p)?),
            None => format!("# {key} = {example}\n"),
        })
    }
    let mut out = String::from(
        "# NetworkDataShare configuration, written by --setup.\n\
         # Command line options and NDS_* environment variables override these values.\n\n",
    );
    out += &format!("# Address and port to listen on.\nbind = \"{}\"\n\n", config.bind);
    out += &format!("# Seconds until an unanswered share invite expires.\ninvite-timeout = {}\n\n", config.invite_timeout);
    out += &format!(
        "# Seconds a disconnected client may take to reconnect before its sessions are closed.\nreconnect-grace = {}\n\n",
        config.reconnect_grace
    );
    out += &format!("# Serve plain HTTP instead of HTTPS.\nhttp = {}\n\n", config.http);
    out += "# Own PEM certificate and key instead of the generated self-signed one.\n";
    out += &optional("cert", &config.cert, "\"/path/to/cert.pem\"")?;
    out += &optional("key", &config.key, "\"/path/to/key.pem\"")?;
    out += "\n# Directory where the generated certificate is stored.\n";
    out += &optional("data-dir", &config.data_dir, &path(&config.data_dir())?)?;
    out += &format!(
        "\n# Maximum number of clients (browser tabs) per IP address.\nmax-clients-per-ip = {}\n",
        config.max_clients_per_ip
    );
    Ok(out)
}

#[cfg(target_os = "linux")]
fn install_service(config_path: &Path) -> Result<(), Box<dyn std::error::Error>> {
    use std::process::Command;

    let exe = std::env::current_exe()?;
    let quote = |p: &Path| format!("\"{}\"", p.display().to_string().replace('\\', "\\\\").replace('"', "\\\""));
    let unit = format!(
        "[Unit]\n\
         Description=NetworkDataShare, share data between devices on the local network\n\n\
         [Service]\n\
         ExecStart={} --config {}\n\
         Restart=on-failure\n\n\
         [Install]\n\
         WantedBy=default.target\n",
        quote(&exe),
        quote(config_path)
    );
    let dir = dirs::config_dir().ok_or("no user config directory")?.join("systemd/user");
    std::fs::create_dir_all(&dir)?;
    let unit_path = dir.join(SERVICE_NAME);
    std::fs::write(&unit_path, unit)?;
    println!("Wrote systemd user service {}", unit_path.display());

    for args in [&["daemon-reload"][..], &["enable", SERVICE_NAME], &["restart", SERVICE_NAME]] {
        let status = Command::new("systemctl")
            .arg("--user")
            .args(args)
            .status()
            .map_err(|e| format!("cannot run systemctl ({e}), use --no-service to skip the service"))?;
        if !status.success() {
            return Err(format!("systemctl --user {} failed, use --no-service to skip the service", args.join(" ")).into());
        }
    }
    println!("Service enabled and started. Logs: journalctl --user -u {SERVICE_NAME}");
    println!("To keep it running while you are logged out: loginctl enable-linger");
    Ok(())
}

#[cfg(not(target_os = "linux"))]
fn install_service(_config_path: &Path) -> Result<(), Box<dyn std::error::Error>> {
    Err("installing a service is only supported on Linux, use --no-service to skip it".into())
}
