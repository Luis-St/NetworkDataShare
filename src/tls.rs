//! TLS setup: loads the configured certificate or generates and persists a self-signed one.
//!
//! HTTPS is the default because browsers only grant clipboard writing and notifications to
//! secure contexts. The generated certificate is stored in the data directory so each device
//! only has to accept the browser warning once.

use std::fs;
use std::net::IpAddr;
use std::path::Path;
use std::sync::Arc;
use std::time::{Duration, SystemTime};

use rcgen::{CertificateParams, DistinguishedName, DnType, ExtendedKeyUsagePurpose, KeyPair};
use rustls_pki_types::pem::PemObject;
use rustls_pki_types::{CertificateDer, PrivateKeyDer};
use tokio_rustls::TlsAcceptor;
use tokio_rustls::rustls::ServerConfig;

use crate::Config;

/// Validity of generated certificates. Apple platforms reject server certificates valid for more
/// than 825 days, even self-signed ones.
const VALIDITY_DAYS: i64 = 800;
/// Regenerate a little before the certificate actually expires.
const REGENERATE_AFTER: Duration = Duration::from_secs(60 * 60 * 24 * 790);

pub fn acceptor(config: &Config) -> Result<TlsAcceptor, Box<dyn std::error::Error>> {
    let _ = tokio_rustls::rustls::crypto::ring::default_provider().install_default();

    let (cert_path, key_path) = match (&config.cert, &config.key) {
        (Some(c), Some(k)) => (c.clone(), k.clone()),
        (None, None) => {
            let dir = config.data_dir();
            let (cert, key, names) = (dir.join("cert.pem"), dir.join("key.pem"), dir.join("cert.names"));
            let mut wanted = subject_names();
            if needs_generation(&cert, &key, &names, &wanted) {
                // Keep earlier names so addresses that come and go (VPN, hotspot) do not cause a new
                // certificate, and with it a new browser warning, on every change.
                for old in fs::read_to_string(&names).unwrap_or_default().lines() {
                    if !old.is_empty() && !wanted.iter().any(|n| n == old) {
                        wanted.push(old.to_string());
                    }
                }
                generate(&cert, &key, &names, &wanted)?;
            }
            (cert, key)
        }
        _ => return Err("--cert and --key must be given together".into()),
    };

    let certs = CertificateDer::pem_file_iter(&cert_path)?.collect::<Result<Vec<_>, _>>()?;
    let key = PrivateKeyDer::from_pem_file(&key_path)?;
    let mut tls = ServerConfig::builder().with_no_client_auth().with_single_cert(certs, key)?;
    // HTTP/1.1 only: keeps WebSocket upgrades simple.
    tls.alpn_protocols = vec![b"http/1.1".to_vec()];
    tracing::info!("using TLS certificate {}", cert_path.display());
    Ok(TlsAcceptor::from(Arc::new(tls)))
}

/// Regenerates when files are missing, the certificate is about to expire, or a current host name or
/// IP address is not covered (e.g. the machine got a new address).
fn needs_generation(cert: &Path, key: &Path, names_file: &Path, wanted: &[String]) -> bool {
    if !cert.exists() || !key.exists() {
        return true;
    }
    let age = fs::metadata(cert).and_then(|m| m.modified()).ok().and_then(|t| SystemTime::now().duration_since(t).ok());
    if age.is_none_or(|a| a > REGENERATE_AFTER) {
        return true;
    }
    let covered = fs::read_to_string(names_file).unwrap_or_default();
    let covered: Vec<&str> = covered.lines().collect();
    wanted.iter().any(|n| !covered.contains(&n.as_str()))
}

fn subject_names() -> Vec<String> {
    let mut names = vec!["localhost".to_string()];
    if let Ok(host) = gethostname::gethostname().into_string() {
        if !host.is_empty() {
            names.push(host.clone());
            names.push(format!("{host}.local"));
        }
    }
    for ip in local_ips() {
        names.push(ip.to_string());
    }
    names.dedup();
    names
}

/// Writes a file that only the owner can read, without a window in which it is readable by others.
fn write_private(path: &Path, contents: &[u8]) -> std::io::Result<()> {
    use std::io::Write;
    let mut opts = fs::OpenOptions::new();
    opts.write(true).create(true).truncate(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::{OpenOptionsExt, PermissionsExt};
        opts.mode(0o600);
        // An existing file keeps its old mode on open, so tighten it before writing.
        if path.exists() {
            fs::set_permissions(path, fs::Permissions::from_mode(0o600))?;
        }
    }
    opts.open(path)?.write_all(contents)
}

pub fn local_ips() -> Vec<IpAddr> {
    if_addrs::get_if_addrs().map(|ifs| ifs.into_iter().map(|i| i.ip()).collect()).unwrap_or_default()
}

fn generate(cert_path: &Path, key_path: &Path, names_path: &Path, names: &[String]) -> Result<(), Box<dyn std::error::Error>> {
    let mut params = CertificateParams::new(names.to_vec())?;
    let mut dn = DistinguishedName::new();
    dn.push(DnType::CommonName, "NetworkDataShare");
    params.distinguished_name = dn;
    params.extended_key_usages = vec![ExtendedKeyUsagePurpose::ServerAuth];
    let now = time::OffsetDateTime::now_utc();
    params.not_before = now - time::Duration::days(1);
    params.not_after = now + time::Duration::days(VALIDITY_DAYS);

    let key = KeyPair::generate()?;
    let cert = params.self_signed(&key)?;

    if let Some(dir) = cert_path.parent() {
        fs::create_dir_all(dir)?;
    }
    if let Some(dir) = key_path.parent() {
        fs::create_dir_all(dir)?;
    }
    write_private(key_path, key.serialize_pem().as_bytes())?;
    fs::write(cert_path, cert.pem())?;
    fs::write(names_path, names.join("\n"))?;
    tracing::info!("generated self-signed certificate for {}", names.join(", "));
    Ok(())
}
