use serde::{Deserialize, Serialize};

/// A limit window: the 5-hour session, the week, the month of credits…
///
/// `id` and `duration` are what a UI translates from; `label` is an English
/// fallback for the CLI and for windows a UI does not know.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct Window {
    pub id: String,
    pub label: String,
    /// Fraction used, 0–1.
    pub used: f64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub resets_at: Option<i64>,
    /// Window length in seconds, when the provider says.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub duration: Option<i64>,
    /// Vendor-given name of a secondary limit ("Code review"), shown as is.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub group: Option<String>,
    /// Absolute amounts behind the fraction, e.g. 604.88 of 10000 credits.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub amount: Option<Amount>,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct Amount {
    pub used: f64,
    pub total: f64,
    /// `credits`, or an ISO currency code.
    pub unit: String,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct Reading {
    pub windows: Vec<Window>,
    #[serde(default)]
    pub plan: Option<String>,
    pub fetched_at: i64,
}

#[derive(Debug)]
pub enum FetchError {
    /// No valid sign-in: the user fixes it by opening the tool, not by retrying.
    Auth(String),
    /// HTTP 429. `retry_after` in seconds when the server said.
    RateLimited(Option<i64>),
    Network(String),
    Other(String),
}

impl FetchError {
    /// Stable code a UI translates from.
    pub fn code(&self) -> &'static str {
        match self {
            FetchError::Auth(_) => "auth",
            FetchError::RateLimited(_) => "rate_limited",
            FetchError::Network(_) => "network",
            FetchError::Other(_) => "other",
        }
    }

    /// English message for logs and the CLI.
    pub fn message(&self) -> String {
        match self {
            FetchError::Auth(m) | FetchError::Network(m) | FetchError::Other(m) => m.clone(),
            FetchError::RateLimited(_) => "too many requests (429), waiting".into(),
        }
    }
}

pub fn clamp01(x: f64) -> f64 {
    if x.is_nan() {
        0.0
    } else {
        x.clamp(0.0, 1.0)
    }
}

/// "5 h", "Week", "Month"… from the window length.
pub fn duration_label(secs: i64) -> String {
    match secs {
        s if s <= 0 => "Window".into(),
        s if s < 86400 => format!("{} h", (s + 1800) / 3600),
        s if (6 * 86400..=8 * 86400).contains(&s) => "Week".into(),
        s if (27 * 86400..=32 * 86400).contains(&s) => "Month".into(),
        s => format!("{} days", (s + 43200) / 86400),
    }
}
