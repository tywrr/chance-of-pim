//! chance-of-pim: mail, contacts, calendars and tasks for Claude, as one MCP endpoint that sits
//! behind the auth Worker (auth/) and a Cloudflare Tunnel.

mod config;
mod davtools;
mod mailtools;
mod mcp;
mod when;

use std::sync::Arc;

#[tokio::main]
async fn main() {
    let path = std::env::args()
        .skip_while(|a| a != "--config")
        .nth(1)
        .or_else(|| std::env::var("CHANCE_OF_PIM_CONFIG").ok())
        .unwrap_or_else(|| "/etc/chance-of-pim/config.toml".into());
    let config = match config::load(&path) {
        Ok(c) => c,
        Err(e) => {
            eprintln!("chance-of-pim: {e}");
            std::process::exit(2);
        }
    };
    let token = match std::env::var("INTERNAL_TOKEN") {
        Ok(t) if t.trim().len() >= 16 => t.trim().to_owned(),
        _ => {
            eprintln!("chance-of-pim: INTERNAL_TOKEN must be set (16+ characters)");
            std::process::exit(2);
        }
    };
    let listen = config.server.listen.clone();
    eprintln!(
        "chance-of-pim {}: {} mail, {} contacts, {} calendar account(s); listening on {listen}",
        env!("CARGO_PKG_VERSION"),
        config.mail.accounts.len(),
        config.contacts.accounts.len(),
        config.calendars.accounts.len(),
    );
    let state = Arc::new(mcp::State { config, token });
    let listener = match tokio::net::TcpListener::bind(&listen).await {
        Ok(l) => l,
        Err(e) => {
            eprintln!("chance-of-pim: cannot listen on {listen}: {e}");
            std::process::exit(2);
        }
    };
    let shutdown = async {
        let mut term = tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate())
            .expect("signal");
        tokio::select! {
            _ = term.recv() => {}
            _ = tokio::signal::ctrl_c() => {}
        }
    };
    if let Err(e) = axum::serve(listener, mcp::router(state))
        .with_graceful_shutdown(shutdown)
        .await
    {
        eprintln!("chance-of-pim: {e}");
        std::process::exit(1);
    }
}
