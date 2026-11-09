//! NetworkDataShare: share text, images and files between devices on the local network.

mod assets;
mod protocol;
mod state;
mod tls;
mod transfer;
mod ws;

use std::convert::Infallible;
use std::net::SocketAddr;
use std::path::PathBuf;
use std::time::Duration;

use axum::Router;
use axum::body::Body;
use axum::extract::{ConnectInfo, DefaultBodyLimit, Request};
use axum::http::{HeaderValue, StatusCode, header};
use axum::response::Response;
use axum::routing::{get, post};
use clap::Parser;
use hyper::body::Incoming;
use hyper_util::rt::{TokioExecutor, TokioIo};
use hyper_util::server::conn::auto;
use hyper_util::service::TowerToHyperService;
use tokio::io::{AsyncRead, AsyncWrite};
use tokio::net::TcpListener;
use tower::ServiceExt;
use tracing_subscriber::EnvFilter;

use crate::state::AppState;

#[derive(Parser, Debug, Clone)]
#[command(version, about = "Share text, images and files between devices on the local network")]
pub struct Config {
    /// Address and port to listen on.
    #[arg(long, env = "NDS_BIND", default_value = "0.0.0.0:8080")]
    pub bind: SocketAddr,

    /// Seconds until an unanswered share invite expires.
    #[arg(long, env = "NDS_INVITE_TIMEOUT", default_value_t = 60)]
    pub invite_timeout: u64,

    /// Seconds a disconnected client may take to reconnect before its sessions are closed.
    #[arg(long, env = "NDS_RECONNECT_GRACE", default_value_t = 10)]
    pub reconnect_grace: u64,

    /// Serve plain HTTP instead of HTTPS (clipboard copy and notifications then only work on localhost).
    #[arg(long, env = "NDS_HTTP")]
    pub http: bool,

    /// PEM certificate to use instead of the generated self-signed one (requires --key).
    #[arg(long, env = "NDS_CERT")]
    pub cert: Option<PathBuf>,

    /// PEM private key matching --cert.
    #[arg(long, env = "NDS_KEY")]
    pub key: Option<PathBuf>,

    /// Directory where the generated certificate is stored.
    #[arg(long, env = "NDS_DATA_DIR", default_value = "data")]
    pub data_dir: PathBuf,
}

#[tokio::main]
async fn main() {
    tracing_subscriber::fmt()
        .with_env_filter(EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("info")))
        .init();

    let config = Config::parse();
    let acceptor = if config.http {
        None
    } else {
        match tls::acceptor(&config) {
            Ok(a) => Some(a),
            Err(e) => {
                eprintln!("TLS setup failed: {e}");
                std::process::exit(1);
            }
        }
    };

    let listener = match TcpListener::bind(config.bind).await {
        Ok(l) => l,
        Err(e) => {
            eprintln!("cannot listen on {}: {e}", config.bind);
            std::process::exit(1);
        }
    };
    print_urls(&config, acceptor.is_some());

    let state = AppState::new(config);
    let app = Router::new()
        .route("/ws", get(ws::handler))
        .route("/api/files/{file_id}", get(transfer::download))
        .route("/api/upload/{req_id}", post(transfer::upload))
        .fallback(assets::serve)
        .layer(DefaultBodyLimit::disable())
        .with_state(state);

    loop {
        let (stream, addr) = match listener.accept().await {
            Ok(c) => c,
            Err(e) => {
                tracing::warn!("accept failed: {e}");
                continue;
            }
        };
        let _ = stream.set_nodelay(true);
        let app = app.clone();
        let acceptor = acceptor.clone();
        tokio::spawn(async move {
            let svc = app.map_request(move |mut req: Request<Incoming>| {
                req.extensions_mut().insert(ConnectInfo(addr));
                req
            });
            let Some(acceptor) = acceptor else {
                return serve(stream, svc).await;
            };
            // Plain HTTP requests on the HTTPS port are redirected; a TLS handshake starts with 0x16.
            let mut first = [0u8; 1];
            match tokio::time::timeout(Duration::from_secs(10), stream.peek(&mut first)).await {
                Ok(Ok(1)) => {}
                _ => return,
            }
            if first[0] == 0x16 {
                match acceptor.accept(stream).await {
                    Ok(tls) => serve(tls, svc).await,
                    Err(e) => tracing::debug!("TLS handshake with {addr} failed: {e}"),
                }
            } else {
                serve(stream, tower::service_fn(redirect_to_https)).await;
            }
        });
    }
}

async fn serve<I, S>(io: I, svc: S)
where
    I: AsyncRead + AsyncWrite + Unpin + Send + 'static,
    S: tower::Service<Request<Incoming>, Response = Response, Error = Infallible> + Clone + Send + 'static,
    S::Future: Send + 'static,
{
    let builder = auto::Builder::new(TokioExecutor::new());
    if let Err(e) = builder.serve_connection_with_upgrades(TokioIo::new(io), TowerToHyperService::new(svc)).await {
        tracing::debug!("connection error: {e}");
    }
}

async fn redirect_to_https(req: Request<Incoming>) -> Result<Response, Infallible> {
    let host = req.headers().get(header::HOST).and_then(|h| h.to_str().ok()).unwrap_or_default();
    let path = req.uri().path_and_query().map(|p| p.as_str()).unwrap_or("/");
    let mut res = Response::new(Body::empty());
    match HeaderValue::from_str(&format!("https://{host}{path}")) {
        Ok(location) if !host.is_empty() => {
            *res.status_mut() = StatusCode::PERMANENT_REDIRECT;
            res.headers_mut().insert(header::LOCATION, location);
        }
        _ => *res.status_mut() = StatusCode::BAD_REQUEST,
    }
    Ok(res)
}

fn print_urls(config: &Config, https: bool) {
    let scheme = if https { "https" } else { "http" };
    let port = config.bind.port();
    println!("NetworkDataShare is running. Open one of these on your devices:");
    if config.bind.ip().is_unspecified() {
        for ip in tls::local_ips() {
            if ip.is_loopback() || (ip.is_ipv6() && config.bind.ip().is_ipv4()) {
                continue;
            }
            match ip {
                std::net::IpAddr::V4(v4) => println!("  {scheme}://{v4}:{port}"),
                std::net::IpAddr::V6(v6) => println!("  {scheme}://[{v6}]:{port}"),
            }
        }
        println!("  {scheme}://localhost:{port}");
    } else {
        println!("  {scheme}://{}", config.bind);
    }
    if https {
        println!("The certificate is self-signed: accept the browser warning once per device.");
    }
}
