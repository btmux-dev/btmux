//! Authenticated requests from CLI subcommands to a running server.

use std::time::Duration;

use reqwest::Method;

use crate::send_notification::server;

/// Sends a request and returns the response body, or the error body (else the
/// status) when it fails.
pub async fn request(
    profile: Option<&str>,
    method: Method,
    path: &str,
    body: Option<serde_json::Value>,
    timeout: Duration,
) -> Result<String, String> {
    let (status, body) = request_with_status(profile, method, path, body, timeout).await?;
    if !status.is_success() {
        return Err(if body.is_empty() {
            status.to_string()
        } else {
            body
        });
    }
    Ok(body)
}

/// Sends a request and returns its status and body; `Err` only if no response came.
pub async fn request_with_status(
    profile: Option<&str>,
    method: Method,
    path: &str,
    body: Option<serde_json::Value>,
    timeout: Duration,
) -> Result<(reqwest::StatusCode, String), String> {
    let (url, token) = server(profile)?;
    let mut request = reqwest::Client::builder()
        .timeout(timeout)
        .build()
        .map_err(|e| e.to_string())?
        .request(method, format!("{url}{path}"))
        .bearer_auth(token);
    if let Some(body) = body {
        request = request
            .header(reqwest::header::CONTENT_TYPE, "application/json")
            .body(body.to_string());
    }
    let response = request.send().await.map_err(|e| e.to_string())?;
    let status = response.status();
    let body = response.text().await.map_err(|e| e.to_string())?;
    Ok((status, body))
}
