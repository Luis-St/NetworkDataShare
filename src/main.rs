//! NetworkDataShare: share text, images and files between devices on the local network.

mod assets;
mod config;
mod protocol;
mod state;
mod tls;
mod transfer;
mod ws;

use std::convert::Infallible;
use std::time::Duration;

use axum::Router;
use axum::body::Body;
use axum::extract::{ConnectInfo, DefaultBodyLimit, Request};
use axum::http::{HeaderValue, StatusCode, header};
use axum::response::Response;
use axum::routing::{get, post};
use hyper::body::Incoming;
use hyper_util::rt::{TokioExecutor, TokioIo, TokioTimer};
use hyper_util::server::conn::auto;
use hyper_util::service::TowerToHyperService;
use tokio::io::{AsyncRead, AsyncWrite};
use tokio::net::TcpListener;
use tower::ServiceExt;
use tracing_subscriber::EnvFilter;

pub use crate::config::Config;
use crate::config::Mode;
use crate::state::AppState;

/// Time a client has to finish the TLS handshake.
const HANDSHAKE_TIMEOUT: Duration = Duration::from_secs(10);
/// Time a client has to send the complete headers of a request.
const HEADER_READ_TIMEOUT: Duration = Duration::from_secs(30);

#[tokio::main]
async fn main() {
    tracing_subscriber::fmt()
        .with_env_filter(EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("info")))
        .init();

    let config = match config::load() {
        Mode::Serve(c) => c,
        Mode::Exit(code) => std::process::exit(code),
    };
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
            match tokio::time::timeout(HANDSHAKE_TIMEOUT, stream.peek(&mut first)).await {
                Ok(Ok(1)) => {}
                _ => return,
            }
            if first[0] == 0x16 {
                match tokio::time::timeout(HANDSHAKE_TIMEOUT, acceptor.accept(stream)).await {
                    Ok(Ok(tls)) => serve(tls, svc).await,
                    Ok(Err(e)) => tracing::debug!("TLS handshake with {addr} failed: {e}"),
                    Err(_) => tracing::debug!("TLS handshake with {addr} timed out"),
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
    let mut builder = auto::Builder::new(TokioExecutor::new());
    builder.http1().timer(TokioTimer::new()).header_read_timeout(HEADER_READ_TIMEOUT);
    if let Err(e) = builder.serve_connection_with_upgrades(TokioIo::new(io), TowerToHyperService::new(svc)).await {
        tracing::debug!("connection error: {e}");
    }
}

async fn redirect_to_https(req: Request<Incoming>) -> Result<Response, Infallible> {
    let host = req.headers().get(header::HOST).and_then(|h| h.to_str().ok()).unwrap_or_default();
    let path = req.uri().path_and_query().map(|p| p.as_str()).unwrap_or("/");
    let path = if path.starts_with('/') { path } else { "/" };
    // Only a plain host[:port] is accepted, so the Host header cannot smuggle in a path or userinfo.
    let valid_host =
        !host.is_empty() && host.chars().all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '-' | ':' | '[' | ']'));
    let mut res = Response::new(Body::empty());
    match HeaderValue::from_str(&format!("https://{host}{path}")) {
        Ok(location) if valid_host => {
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
