//! `btmux send-notification`: show a notification in every connected browser.

use std::time::Duration;

use crate::session::NotificationLevel;

/// Send the notification. Returns the exit code.
pub async fn run(
    profile: Option<&str>,
    level: NotificationLevel,
    title: &str,
    body: Option<&str>,
) -> i32 {
    match send(profile, level, title, body).await {
        Ok(()) => 0,
        Err(error) => {
            eprintln!("btmux: {error}");
            1
        }
    }
}

/// A server named on the command line (`btmux forward --server`), whose
/// token comes from `BTMUX_AUTH_TOKEN`.
pub static SERVER_URL: std::sync::OnceLock<String> = std::sync::OnceLock::new();

/// The server's URL and token: `SERVER_URL`, else this pane's server, unless
/// `--profile` names one, else the profile's registration.
pub fn server(profile: Option<&str>) -> Result<(String, String), String> {
    let env = |name| std::env::var(name).ok().filter(|v| !v.is_empty());
    if let Some(url) = SERVER_URL.get() {
        let token =
            env("BTMUX_AUTH_TOKEN").ok_or("--server needs the token in BTMUX_AUTH_TOKEN")?;
        return Ok((url.trim_end_matches('/').to_string(), token));
    }
    if profile.is_none() {
        if let (Some(url), Some(token)) = (env("BTMUX_API_URL"), env("BTMUX_AUTH_TOKEN")) {
            return Ok((url, token));
        }
    }
    let state = crate::persistence::state_path(profile)?
        .ok_or("cannot resolve the btmux state directory")?;
    let server = crate::discovery::read_registration(&state.with_extension("server"))
        .ok_or("no running btmux server found")?;
    Ok((format!("http://{}", server.address), server.token))
}

async fn send(
    profile: Option<&str>,
    level: NotificationLevel,
    title: &str,
    body: Option<&str>,
) -> Result<(), String> {
    let (url, token) = server(profile)?;
    let response = reqwest::Client::builder()
        .timeout(Duration::from_secs(5))
        .build()
        .map_err(|e| e.to_string())?
        .post(format!("{url}/api/notify"))
        .bearer_auth(token)
        .header(reqwest::header::CONTENT_TYPE, "application/json")
        .body(serde_json::json!({ "level": level, "title": title, "body": body }).to_string())
        .send()
        .await
        .map_err(|e| e.to_string())?;
    let status = response.status();
    if status.is_success() {
        return Ok(());
    }
    let body = response.text().await.unwrap_or_default();
    Err(if body.is_empty() {
        status.to_string()
    } else {
        body
    })
}
