//! Claude Code: una lectura por cuenta (no por perfil) y el estado de sus sesiones.
//!
//! Varias carpetas `~/.claude*` pueden ser la misma cuenta (`~/.claude` y
//! `~/.claude-flash` aquí): se agrupan por el email de `oauthAccount`, y para
//! consultar se usa el token que caduque más tarde. Nunca se escribe en
//! `.credentials.json`; renovar el token es cosa de Claude Code.

use crate::model::{clamp01, Amount, FetchError, Reading, Window};
use crate::util::{home, now, parse_rfc3339, read_json};
use serde_json::Value;
use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};

const USAGE_URL: &str = "https://api.anthropic.com/api/oauth/usage";

#[derive(Clone, Debug)]
pub struct Account {
    pub email: String,
    pub dirs: Vec<PathBuf>,
}

/// Email de la cuenta del perfil. El perfil por defecto lo guarda en
/// `~/.claude.json`; los que van por `CLAUDE_CONFIG_DIR`, dentro de su carpeta.
fn profile_email(dir: &Path) -> Option<String> {
    let inside = dir.join(".claude.json");
    // Nunca `~/.claude.json` para otro perfil: lo fundiría con la cuenta por defecto.
    let candidates = if dir == home().join(".claude") {
        vec![home().join(".claude.json"), inside]
    } else {
        vec![inside]
    };
    candidates.iter().find_map(|p| {
        read_json(p)?
            .pointer("/oauthAccount/emailAddress")?
            .as_str()
            .map(String::from)
    })
}

/// Todas las carpetas `~/.claude` y `~/.claude-*` que tienen una sesión OAuth.
pub fn discover() -> Vec<Account> {
    let mut by_email: BTreeMap<String, Vec<PathBuf>> = BTreeMap::new();
    let Ok(entries) = fs::read_dir(home()) else {
        return Vec::new();
    };
    let mut dirs: Vec<PathBuf> = entries
        .flatten()
        .filter_map(|e| {
            let name = e.file_name().to_string_lossy().into_owned();
            (name == ".claude" || name.starts_with(".claude-")).then(|| e.path())
        })
        .filter(|p| p.is_dir() && p.join(".credentials.json").is_file())
        .collect();
    dirs.sort();
    for dir in dirs {
        let has_token = read_json(&dir.join(".credentials.json"))
            .and_then(|v| v.pointer("/claudeAiOauth/accessToken").cloned())
            .is_some_and(|t| t.is_string());
        if !has_token {
            continue;
        }
        let email = profile_email(&dir).unwrap_or_else(|| dir.file_name().unwrap().to_string_lossy().into_owned());
        by_email.entry(email).or_default().push(dir);
    }
    // La cuenta de `~/.claude` siempre primero; el resto por email. Así los
    // iconos de la barra nunca cambian de sitio.
    let default = home().join(".claude");
    let mut accounts: Vec<Account> = by_email
        .into_iter()
        .map(|(email, dirs)| Account { email, dirs })
        .collect();
    accounts.sort_by_key(|a| !a.dirs.contains(&default));
    accounts
}

struct Token {
    access: String,
    expires_at: i64,
    plan: Option<String>,
}

fn best_token(dirs: &[PathBuf]) -> Result<Token, FetchError> {
    let best = dirs
        .iter()
        .filter_map(|d| {
            let v = read_json(&d.join(".credentials.json"))?;
            let o = v.get("claudeAiOauth")?;
            Some(Token {
                access: o.get("accessToken")?.as_str()?.to_string(),
                expires_at: o.get("expiresAt").and_then(Value::as_i64).unwrap_or(0) / 1000,
                plan: o.get("subscriptionType").and_then(Value::as_str).map(String::from),
            })
        })
        .max_by_key(|t| t.expires_at)
        .ok_or_else(|| FetchError::Auth("no Claude sign-in".into()))?;
    if best.expires_at != 0 && best.expires_at < now() + 30 {
        return Err(FetchError::Auth(
            "token expired: open Claude Code with this account".into(),
        ));
    }
    Ok(best)
}

pub fn fetch(agent: &ureq::Agent, dirs: &[PathBuf]) -> Result<Reading, FetchError> {
    let token = best_token(dirs)?;
    let resp = agent
        .get(USAGE_URL)
        .set("Authorization", &format!("Bearer {}", token.access))
        .set("anthropic-beta", "oauth-2025-04-20")
        .call();
    let body: Value = match resp {
        Ok(r) => r
            .into_json()
            .map_err(|e| FetchError::Other(format!("unreadable response: {e}")))?,
        Err(ureq::Error::Status(429, r)) => {
            return Err(FetchError::RateLimited(
                r.header("retry-after").and_then(|s| s.trim().parse().ok()),
            ))
        }
        Err(ureq::Error::Status(401, _)) => {
            return Err(FetchError::Auth("sign-in rejected (401): open Claude Code".into()))
        }
        Err(ureq::Error::Status(code, _)) => return Err(FetchError::Other(format!("HTTP {code}"))),
        Err(e) => return Err(FetchError::Network(format!("network: {e}"))),
    };
    Ok(Reading {
        windows: parse_usage(&body),
        plan: token.plan.map(|p| capitalize(&p)),
        fetched_at: now(),
    })
}

fn capitalize(s: &str) -> String {
    let mut c = s.chars();
    c.next()
        .map(|f| f.to_uppercase().collect::<String>() + c.as_str())
        .unwrap_or_default()
}

pub fn parse_usage(body: &Value) -> Vec<Window> {
    const WINDOWS: [(&str, &str, &str, i64); 4] = [
        ("five_hour", "session", "5-hour session", 5 * 3600),
        ("seven_day", "week", "Week", 7 * 86400),
        ("seven_day_opus", "week_opus", "Week · Opus", 7 * 86400),
        ("seven_day_sonnet", "week_sonnet", "Week · Sonnet", 7 * 86400),
    ];
    let mut out = Vec::new();
    for (key, id, label, duration) in WINDOWS {
        let Some(w) = body.get(key).filter(|w| w.is_object()) else {
            continue;
        };
        let Some(pct) = w.get("utilization").and_then(Value::as_f64) else {
            continue;
        };
        out.push(Window {
            id: id.into(),
            label: label.into(),
            used: clamp01(pct / 100.0),
            resets_at: w.get("resets_at").and_then(Value::as_str).and_then(parse_rfc3339),
            duration: Some(duration),
            group: None,
            amount: None,
        });
    }
    // Uso extra de pago: solo cuando hay un tope mensual con el que comparar.
    if let Some(x) = body.get("extra_usage").filter(|x| x["is_enabled"] == true) {
        let limit = x.get("monthly_limit").and_then(Value::as_f64).unwrap_or(0.0);
        let used = x.get("used_credits").and_then(Value::as_f64).unwrap_or(0.0);
        if limit > 0.0 {
            let cur = x.get("currency").and_then(Value::as_str).unwrap_or("USD");
            out.push(Window {
                id: "extra".into(),
                label: "Extra usage (month)".into(),
                used: clamp01(used / limit),
                resets_at: None,
                duration: None,
                group: None,
                amount: Some(Amount { used: used / 100.0, total: limit / 100.0, unit: cur.into() }),
            });
        }
    }
    out
}

// ── Sesiones ────────────────────────────────────────────────────────────────

/// Lo que Claude Code escribe en `<perfil>/sessions/<pid>.json`.
#[derive(Clone, Debug, PartialEq)]
pub struct Session {
    pub session_id: String,
    pub name: String,
    pub cwd: String,
    /// `busy`, `waiting` o `idle` (también `shell`, que tratamos como idle).
    pub status: String,
    pub waiting_for: Option<String>,
}

/// El pid sigue vivo y es el mismo proceso (no un pid reciclado).
fn alive(pid: u64, proc_start: Option<&str>) -> bool {
    let Ok(stat) = fs::read_to_string(format!("/proc/{pid}/stat")) else {
        return false;
    };
    let Some(start) = proc_start else {
        return true;
    };
    // El nombre del proceso va entre paréntesis y puede tener espacios:
    // los campos se cuentan desde el último ')'. starttime es el campo 22.
    stat.rsplit_once(')')
        .and_then(|(_, rest)| rest.split_whitespace().nth(19))
        .is_some_and(|s| s == start)
}

pub fn scan_sessions(dir: &Path, ignore_prefixes: &[String]) -> Vec<Session> {
    let Ok(entries) = fs::read_dir(dir.join("sessions")) else {
        return Vec::new();
    };
    let mut out = Vec::new();
    for e in entries.flatten() {
        let path = e.path();
        if path.extension().and_then(|x| x.to_str()) != Some("json") {
            continue;
        }
        let Some(v) = read_json(&path) else { continue };
        let Some(pid) = v.get("pid").and_then(Value::as_u64) else {
            continue;
        };
        // Solo sesiones interactivas: `claude -p` y los SDK no son "Claude trabajando para mí".
        if v.get("kind").and_then(Value::as_str).is_some_and(|k| k != "interactive") {
            continue;
        }
        let name = v.get("name").and_then(Value::as_str).unwrap_or("").to_string();
        if ignore_prefixes.iter().any(|p| name.starts_with(p.as_str())) {
            continue;
        }
        if !alive(pid, v.get("procStart").and_then(Value::as_str)) {
            continue;
        }
        let s = |k: &str| v.get(k).and_then(Value::as_str).map(String::from);
        out.push(Session {
            session_id: s("sessionId").unwrap_or_else(|| pid.to_string()),
            name,
            cwd: s("cwd").unwrap_or_default(),
            status: s("status").unwrap_or_else(|| "idle".into()),
            waiting_for: s("waitingFor"),
        });
    }
    out.sort_by(|a, b| a.name.cmp(&b.name));
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn usage_windows() {
        let body: Value = serde_json::from_str(
            r#"{"five_hour":{"utilization":23.0,"resets_at":"2026-10-05T03:39:59.639565+00:00"},
                "seven_day":{"utilization":24.0,"resets_at":"2026-10-05T06:59:59+00:00"},
                "seven_day_opus":null,
                "extra_usage":{"is_enabled":true,"monthly_limit":0,"used_credits":0.0}}"#,
        )
        .unwrap();
        let w = parse_usage(&body);
        assert_eq!(w.len(), 2);
        assert_eq!(w[0].id, "session");
        assert!((w[0].used - 0.23).abs() < 1e-9);
        assert_eq!(w[0].resets_at, Some(1791171599));
        assert_eq!(w[1].label, "Week");
    }

    #[test]
    fn self_is_alive() {
        let pid = std::process::id() as u64;
        assert!(alive(pid, None));
        assert!(!alive(pid, Some("0")));
    }
}
