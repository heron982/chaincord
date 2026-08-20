use crate::peer;
use std::time::Duration;
use tauri::AppHandle;

pub fn spawn(app: AppHandle, port: u16) {
    tauri::async_runtime::spawn(async move {
        if let Some(ip) = tokio::task::spawn_blocking(fetch_public_v4)
            .await
            .ok()
            .flatten()
        {
            peer::add_listen_url(&app, format!("ws://{ip}:{port}"));
        }
        if let Some(ip) = tokio::task::spawn_blocking(fetch_public_v6)
            .await
            .ok()
            .flatten()
        {
            peer::add_listen_url(&app, format!("ws://[{ip}]:{port}"));
        }
    });
}

fn fetch_public_v4() -> Option<String> {
    let body = ureq::get("https://api.ipify.org")
        .timeout(Duration::from_secs(4))
        .call()
        .ok()?
        .into_string()
        .ok()?;
    let ip = body.trim();
    if ip.parse::<std::net::Ipv4Addr>().is_ok() {
        Some(ip.to_string())
    } else {
        None
    }
}

fn fetch_public_v6() -> Option<String> {
    let body = ureq::get("https://api64.ipify.org")
        .timeout(Duration::from_secs(4))
        .call()
        .ok()?
        .into_string()
        .ok()?;
    let ip = body.trim();
    if ip.parse::<std::net::Ipv6Addr>().is_ok() && ip.contains(':') {
        Some(ip.to_string())
    } else {
        None
    }
}
