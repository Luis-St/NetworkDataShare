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
            let cert = config.data_dir.join("cert.pem");
            let key = config.data_dir.join("key.pem");
            if needs_generation(&cert, &key) {
                generate(&cert, &key)?;
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

fn needs_generation(cert: &Path, key: &Path) -> bool {
    if !cert.exists() || !key.exists() {
        return true;
    }
    let age = fs::metadata(cert).and_then(|m| m.modified()).ok().and_then(|t| SystemTime::now().duration_since(t).ok());
    age.is_none_or(|a| a > REGENERATE_AFTER)
}

pub fn local_ips() -> Vec<IpAddr> {
    if_addrs::get_if_addrs().map(|ifs| ifs.into_iter().map(|i| i.ip()).collect()).unwrap_or_default()
}

fn generate(cert_path: &Path, key_path: &Path) -> Result<(), Box<dyn std::error::Error>> {
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

    let mut params = CertificateParams::new(names.clone())?;
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
    fs::write(cert_path, cert.pem())?;
    fs::write(key_path, key.serialize_pem())?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(key_path, fs::Permissions::from_mode(0o600))?;
    }
    tracing::info!("generated self-signed certificate for {}", names.join(", "));
    Ok(())
}
