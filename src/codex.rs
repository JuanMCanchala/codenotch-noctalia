//! Codex: el endpoint de uso de ChatGPT con la sesión de `~/.codex/auth.json`.
//!
//! Solo lectura. El token lo renueva el propio `codex` cada vez que se usa
//! (dura unos 10 días); si caduca, la última lectura queda marcada como vieja.

use crate::model::{clamp01, duration_label, FetchError, Reading, Window};
use crate::util::{home, now, read_json};
use serde_json::Value;

const USAGE_URL: &str = "https://chatgpt.com/backend-api/wham/usage";

pub fn available() -> bool {
    home().join(".codex/auth.json").is_file()
}

pub fn fetch(agent: &ureq::Agent) -> Result<Reading, FetchError> {
    let auth = read_json(&home().join(".codex/auth.json"))
        .ok_or_else(|| FetchError::Auth("sin ~/.codex/auth.json".into()))?;
    let token = auth
        .pointer("/tokens/access_token")
        .and_then(Value::as_str)
        .ok_or_else(|| FetchError::Auth("Codex no tiene sesión de ChatGPT".into()))?;
    let mut req = agent
        .get(USAGE_URL)
        .set("Authorization", &format!("Bearer {token}"))
        .set("User-Agent", "codex_cli_rs");
    if let Some(account) = auth.pointer("/tokens/account_id").and_then(Value::as_str) {
        req = req.set("ChatGPT-Account-Id", account);
    }
    let body: Value = match req.call() {
        Ok(r) => r
            .into_json()
            .map_err(|e| FetchError::Other(format!("respuesta ilegible: {e}")))?,
        Err(ureq::Error::Status(429, r)) => {
            return Err(FetchError::RateLimited(
                r.header("retry-after").and_then(|s| s.trim().parse().ok()),
            ))
        }
        Err(ureq::Error::Status(401 | 403, _)) => {
            return Err(FetchError::Auth("sesión caducada: ejecuta codex".into()))
        }
        Err(ureq::Error::Status(code, _)) => return Err(FetchError::Other(format!("HTTP {code}"))),
        Err(e) => return Err(FetchError::Other(format!("red: {e}"))),
    };
    Ok(parse(&body, now()))
}

fn window(id: &str, w: &Value, label: Option<&str>, now: i64) -> Option<Window> {
    let pct = w.get("used_percent").and_then(Value::as_f64)?;
    let duration = w.get("limit_window_seconds").and_then(Value::as_i64);
    let resets_at = w.get("reset_at").and_then(Value::as_i64).or_else(|| {
        w.get("reset_after_seconds").and_then(Value::as_i64).map(|s| now + s)
    });
    let base = duration.map(duration_label).unwrap_or_else(|| "Ventana".into());
    Some(Window {
        id: id.into(),
        label: label.map(|l| format!("{l} · {base}")).unwrap_or(base),
        used: clamp01(pct / 100.0),
        resets_at,
        duration,
        detail: None,
    })
}

fn push_limit(out: &mut Vec<Window>, prefix: &str, rl: &Value, label: Option<&str>, now: i64) {
    for (key, suffix) in [("primary_window", "primary"), ("secondary_window", "secondary")] {
        if let Some(w) = rl.get(key).filter(|w| w.is_object()) {
            if let Some(w) = window(&format!("{prefix}{suffix}"), w, label, now) {
                out.push(w);
            }
        }
    }
}

pub fn parse(body: &Value, now: i64) -> Reading {
    let mut windows = Vec::new();
    if let Some(rl) = body.get("rate_limit").filter(|v| v.is_object()) {
        push_limit(&mut windows, "", rl, None, now);
    }
    if let Some(rl) = body.get("code_review_rate_limit").filter(|v| v.is_object()) {
        push_limit(&mut windows, "review_", rl, Some("Code review"), now);
    }
    if let Some(extra) = body.get("additional_rate_limits").and_then(Value::as_array) {
        for (i, item) in extra.iter().enumerate() {
            let name = item
                .get("limit_name")
                .or_else(|| item.get("metered_feature"))
                .and_then(Value::as_str);
            if let Some(rl) = item.get("rate_limit").filter(|v| v.is_object()) {
                push_limit(&mut windows, &format!("extra{i}_"), rl, name.or(Some("Extra")), now);
            }
        }
    }
    let plan = body
        .get("plan_type")
        .and_then(Value::as_str)
        .map(|p| {
            let mut c = p.chars();
            c.next().map(|f| f.to_uppercase().collect::<String>() + c.as_str()).unwrap_or_default()
        });
    Reading { windows, plan, fetched_at: now }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn free_plan_monthly() {
        let body: Value = serde_json::from_str(
            r#"{"plan_type":"free","rate_limit":{"allowed":true,"primary_window":
               {"used_percent":2,"limit_window_seconds":2592000,"reset_after_seconds":100,"reset_at":1793761123},
               "secondary_window":null},"code_review_rate_limit":null,"additional_rate_limits":null}"#,
        )
        .unwrap();
        let r = parse(&body, 0);
        assert_eq!(r.plan.as_deref(), Some("Free"));
        assert_eq!(r.windows.len(), 1);
        assert_eq!(r.windows[0].label, "Mes");
        assert_eq!(r.windows[0].resets_at, Some(1793761123));
        assert!((r.windows[0].used - 0.02).abs() < 1e-9);
    }

    #[test]
    fn plus_plan_two_windows() {
        let body: Value = serde_json::from_str(
            r#"{"plan_type":"plus","rate_limit":{
               "primary_window":{"used_percent":40,"limit_window_seconds":18000,"reset_after_seconds":60},
               "secondary_window":{"used_percent":10,"limit_window_seconds":604800,"reset_at":5}}}"#,
        )
        .unwrap();
        let r = parse(&body, 1000);
        assert_eq!(r.windows[0].label, "5 h");
        assert_eq!(r.windows[0].resets_at, Some(1060));
        assert_eq!(r.windows[1].label, "Semana");
    }
}
