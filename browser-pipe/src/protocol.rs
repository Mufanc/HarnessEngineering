use std::collections::HashMap;

use serde::{Deserialize, Serialize};

// ── MCP client <-> Daemon (WebSocket JSON) ──

#[derive(Debug, Serialize, Deserialize)]
pub struct DaemonRequest {
    pub id: String,
    pub url: String,
    pub method: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub headers: Option<HashMap<String, String>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub body: Option<String>,
    pub redirect: String,
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DaemonResponse {
    pub id: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub status: Option<u16>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub status_text: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub body: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub body_base64: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub redirected: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub url: Option<String>,
}

impl DaemonResponse {
    pub fn error(id: String, error: String) -> Self {
        Self {
            id,
            error: Some(error),
            status: None,
            status_text: None,
            body: None,
            body_base64: None,
            redirected: None,
            url: None,
        }
    }
}

// ── Daemon <-> Chrome Extension (WebSocket JSON) ──

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ChromeRequest {
    pub r#type: &'static str,
    pub id: String,
    pub url: String,
    pub method: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub headers: Option<HashMap<String, String>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub body: Option<String>,
    pub redirect: String,
}

impl From<DaemonRequest> for ChromeRequest {
    fn from(req: DaemonRequest) -> Self {
        Self {
            r#type: "fetch_request",
            id: req.id,
            url: req.url,
            method: req.method,
            headers: req.headers,
            body: req.body,
            redirect: req.redirect,
        }
    }
}

#[derive(Debug, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ChromeMessage {
    FetchResponse {
        id: String,
        status: u16,
        #[serde(rename = "statusText")]
        status_text: String,
        body: Option<String>,
        #[serde(rename = "bodyBase64")]
        body_base64: Option<String>,
        redirected: bool,
        url: String,
    },
    FetchError {
        id: String,
        error: String,
    },
}

impl ChromeMessage {
    pub fn into_daemon_resp(self) -> DaemonResponse {
        match self {
            ChromeMessage::FetchResponse {
                id,
                status,
                status_text,
                body,
                body_base64,
                redirected,
                url,
            } => DaemonResponse {
                id,
                error: None,
                status: Some(status),
                status_text: Some(status_text),
                body,
                body_base64,
                redirected: Some(redirected),
                url: Some(url),
            },
            ChromeMessage::FetchError { id, error } => DaemonResponse::error(id, error),
        }
    }
}
