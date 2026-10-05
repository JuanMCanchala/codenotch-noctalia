//! Kiro: el `/usage` del propio `kiro-cli`, sin tocar su sesión.
//!
//! `kiro-cli chat --no-interactive /usage` imprime una tarjeta de texto:
//!
//! ```text
//! Estimated Usage | resets on 2026-11-01 | KIRO POWER
//! Credits (604.88 of 10000 covered in plan), 6.0%
//! ```
//!
//! Tarda ~12 s en arrancar, así que se consulta con poca frecuencia y en su
//! propio hilo. Al vencer el tiempo se mata el grupo de procesos entero:
//! kiro-cli lanza hijos que sobrevivirían a un kill del padre.

use crate::model::{clamp01, Amount, FetchError, Reading, Window};
use crate::util::{home, local_midnight, now, strip_ansi};
use std::io::Read;
use std::os::unix::process::CommandExt;
use std::path::PathBuf;
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

const TIMEOUT: Duration = Duration::from_secs(45);

pub fn locate() -> Option<PathBuf> {
    if let Some(p) = std::env::var_os("KIRO_CLI_PATH").map(PathBuf::from) {
        return p.is_file().then_some(p);
    }
    let mut candidates = vec![home().join(".local/bin/kiro-cli"), PathBuf::from("/usr/bin/kiro-cli")];
    if let Some(path) = std::env::var_os("PATH") {
        candidates.extend(std::env::split_paths(&path).filter(|d| d.is_absolute()).map(|d| d.join("kiro-cli")));
    }
    candidates.into_iter().find(|p| p.is_file())
}

pub fn fetch(binary: &PathBuf) -> Result<Reading, FetchError> {
    let mut child = Command::new(binary)
        .args(["chat", "--no-interactive", "/usage"])
        .env("TERM", "dumb")
        .env("KIRO_CHAT_UI", "classic")
        .env("NO_COLOR", "1")
        // chat indexa el cwd: nunca un árbol grande.
        .current_dir(std::env::temp_dir())
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .process_group(0)
        .spawn()
        .map_err(|e| FetchError::Other(format!("could not start kiro-cli: {e}")))?;
    let pid = child.id() as i32;

    // Se drenan ambas tuberías mientras corre: una llena bloquearía al hijo.
    let mut out = child.stdout.take().unwrap();
    let mut err = child.stderr.take().unwrap();
    let t_out = std::thread::spawn(move || {
        let mut s = String::new();
        let _ = out.read_to_string(&mut s);
        s
    });
    let t_err = std::thread::spawn(move || {
        let mut s = String::new();
        let _ = err.read_to_string(&mut s);
        s
    });

    let start = Instant::now();
    let status = loop {
        match child.try_wait() {
            Ok(Some(st)) => break Some(st),
            Ok(None) if start.elapsed() < TIMEOUT => std::thread::sleep(Duration::from_millis(200)),
            _ => break None,
        }
    };
    let Some(status) = status else {
        unsafe { libc::killpg(pid, libc::SIGKILL) };
        let _ = child.wait();
        return Err(FetchError::Other("kiro-cli timed out".into()));
    };
    // Si un nieto se quedó con la tubería, que no bloquee la lectura.
    unsafe { libc::killpg(pid, libc::SIGKILL) };
    let stdout = t_out.join().unwrap_or_default();
    let stderr = t_err.join().unwrap_or_default();

    // Según la versión, la tarjeta sale por stdout o por stderr.
    let text = if stdout.contains("Estimated Usage") || !stderr.contains("Estimated Usage") && !stdout.trim().is_empty() {
        stdout
    } else {
        stderr
    };
    let reading = parse(&text, now());
    if reading.is_err() && !status.success() {
        return Err(FetchError::Auth("kiro-cli is not signed in: run kiro-cli login".into()));
    }
    reading
}

fn title_case(s: &str) -> String {
    s.split_whitespace()
        .map(|w| {
            let lower = w.to_lowercase();
            let mut c = lower.chars();
            c.next().map(|f| f.to_uppercase().collect::<String>() + c.as_str()).unwrap_or_default()
        })
        .collect::<Vec<_>>()
        .join(" ")
}

/// Primer número decimal que empieza en `s`.
fn leading_number(s: &str) -> Option<f64> {
    let s = s.trim_start();
    let end = s.find(|c: char| !(c.is_ascii_digit() || c == '.')).unwrap_or(s.len());
    s[..end].parse().ok()
}

pub fn parse(raw: &str, now: i64) -> Result<Reading, FetchError> {
    let text = strip_ansi(raw);
    let lower = text.to_ascii_lowercase(); // ASCII: mismos offsets que `text`
    if ["not logged in", "login required", "kiro-cli login", "oauth error", "failed to initialize auth"]
        .iter()
        .any(|m| lower.contains(m))
    {
        return Err(FetchError::Auth("kiro-cli is not signed in: run kiro-cli login".into()));
    }

    let plan = text
        .find("| KIRO")
        .map(|i| &text[i + 2..])
        .map(|s| s.split(['|', '\n', '\r']).next().unwrap_or("").trim())
        .filter(|s| !s.is_empty())
        .map(title_case);

    let resets_at = lower
        .find("resets on ")
        .and_then(|i| text.get(i + 10..i + 20))
        .and_then(local_midnight);

    // "(604.88 of 10000 covered in plan)"
    let credits = lower.find(" covered").and_then(|end| {
        let open = lower[..end].rfind('(')?;
        let inner = &lower[open + 1..end];
        let (used, total) = inner.split_once(" of ")?;
        Some((leading_number(used)?, leading_number(total)?))
    });

    // "…covered in plan), 6.0%": el porcentaje que imprime manda sobre el cálculo.
    let percent = lower.find("covered").and_then(|i| {
        let rest = &lower[i..];
        let pct = rest.find('%')?;
        let before = &rest[..pct];
        let start = before.rfind(|c: char| !(c.is_ascii_digit() || c == '.'))? + 1;
        before[start..].parse::<f64>().ok()
    });

    let used = percent.map(|p| p / 100.0).or_else(|| {
        credits.and_then(|(u, t)| (t > 0.0).then(|| u / t))
    });

    let Some(used) = used else {
        return Err(FetchError::Other(if plan.is_some() {
            "Kiro reported no credits".into()
        } else {
            "unrecognised kiro-cli output".into()
        }));
    };

    Ok(Reading {
        windows: vec![Window {
            id: "credits".into(),
            label: "Monthly credits".into(),
            used: clamp01(used),
            resets_at,
            duration: None,
            group: None,
            amount: credits.map(|(used, total)| Amount { used, total, unit: "credits".into() }),
        }],
        plan,
        fetched_at: now,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn power_plan() {
        let out = "\x1b[1mEstimated Usage\x1b[0m | resets on 2026-11-01 | KIRO POWER\n\
                   Credits (604.88 of 10000 covered in plan), 6.0%\n\
                   Your plan is managed by your organization's administrator.\n";
        let r = parse(out, 0).unwrap();
        assert_eq!(r.plan.as_deref(), Some("Kiro Power"));
        assert!((r.windows[0].used - 0.06).abs() < 1e-9);
        let a = r.windows[0].amount.as_ref().unwrap();
        assert_eq!((a.used, a.total, a.unit.as_str()), (604.88, 10000.0, "credits"));
        assert!(r.windows[0].resets_at.is_some());
    }

    #[test]
    fn credits_without_percent() {
        let r = parse("Credits (50 of 200 covered in plan)\n| KIRO FREE", 0).unwrap();
        assert!((r.windows[0].used - 0.25).abs() < 1e-9);
        assert_eq!(r.plan.as_deref(), Some("Kiro Free"));
    }

    #[test]
    fn logged_out() {
        assert!(matches!(parse("Error: not logged in", 0), Err(FetchError::Auth(_))));
    }
}
