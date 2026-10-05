use serde::{Deserialize, Serialize};

/// Una ventana de límite: la sesión de 5 h, la semana, el mes de créditos…
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct Window {
    pub id: String,
    pub label: String,
    /// Fracción usada, 0–1.
    pub used: f64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub resets_at: Option<i64>,
    /// Duración de la ventana en segundos, cuando el proveedor la dice.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub duration: Option<i64>,
    /// Texto extra para el panel, p. ej. "604.88 / 10000 créditos".
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub detail: Option<String>,
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
    /// Sin sesión válida: lo arregla el usuario abriendo la herramienta, no reintentar más.
    Auth(String),
    /// HTTP 429. `retry_after` en segundos si el servidor lo dijo.
    RateLimited(Option<i64>),
    Other(String),
}

impl FetchError {
    pub fn message(&self) -> String {
        match self {
            FetchError::Auth(m) | FetchError::Other(m) => m.clone(),
            FetchError::RateLimited(_) => "demasiadas consultas (429), en espera".into(),
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

/// "5 h", "Semana", "Mes"… a partir de la duración de la ventana.
pub fn duration_label(secs: i64) -> String {
    match secs {
        s if s <= 0 => "Ventana".into(),
        s if s < 86400 => format!("{} h", (s + 1800) / 3600),
        s if (6 * 86400..=8 * 86400).contains(&s) => "Semana".into(),
        s if (27 * 86400..=32 * 86400).contains(&s) => "Mes".into(),
        s => format!("{} días", (s + 43200) / 86400),
    }
}
