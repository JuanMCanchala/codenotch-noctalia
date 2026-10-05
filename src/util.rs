use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

pub fn now() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}

pub fn home() -> PathBuf {
    std::env::var_os("HOME").map(PathBuf::from).unwrap_or_else(|| PathBuf::from("/"))
}

fn xdg(var: &str, fallback: &str) -> PathBuf {
    match std::env::var_os(var) {
        Some(v) if !v.is_empty() => PathBuf::from(v),
        _ => home().join(fallback),
    }
}

/// `$XDG_RUNTIME_DIR/codenotch`: estado vivo y socket. Se borra al cerrar sesión.
pub fn runtime_dir() -> PathBuf {
    let base = match std::env::var_os("XDG_RUNTIME_DIR") {
        Some(v) if !v.is_empty() => PathBuf::from(v),
        _ => PathBuf::from(format!("/run/user/{}", unsafe { libc::getuid() })),
    };
    base.join("codenotch")
}

/// `$XDG_STATE_HOME/codenotch`: última lectura de cada proveedor, para que un
/// reinicio no vuelva a consultar lo que todavía está fresco.
pub fn state_dir() -> PathBuf {
    xdg("XDG_STATE_HOME", ".local/state").join("codenotch")
}

pub fn config_dir() -> PathBuf {
    xdg("XDG_CONFIG_HOME", ".config").join("codenotch")
}

pub fn state_file() -> PathBuf {
    runtime_dir().join("state.json")
}

pub fn socket_path() -> PathBuf {
    runtime_dir().join("sock")
}

/// Escribe en un temporal y renombra: quien lee nunca ve un archivo a medias.
pub fn write_atomic(path: &Path, data: &[u8]) -> std::io::Result<()> {
    if let Some(dir) = path.parent() {
        fs::create_dir_all(dir)?;
    }
    let tmp = path.with_extension("tmp");
    {
        let mut f = fs::File::create(&tmp)?;
        f.write_all(data)?;
    }
    fs::rename(&tmp, path)
}

pub fn read_json(path: &Path) -> Option<serde_json::Value> {
    let text = fs::read_to_string(path).ok()?;
    serde_json::from_str(&text).ok()
}

/// Días desde 1970-01-01 para una fecha civil (algoritmo de Howard Hinnant).
fn days_from_civil(y: i64, m: i64, d: i64) -> i64 {
    let y = if m <= 2 { y - 1 } else { y };
    let era = if y >= 0 { y } else { y - 399 } / 400;
    let yoe = y - era * 400;
    let mp = (m + 9) % 12;
    let doy = (153 * mp + 2) / 5 + d - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    era * 146097 + doe - 719468
}

fn num(s: &str, range: std::ops::Range<usize>) -> Option<i64> {
    s.get(range)?.parse().ok()
}

/// `2026-10-05T03:39:59.639565+00:00` / `...Z` → segundos unix.
pub fn parse_rfc3339(s: &str) -> Option<i64> {
    let s = s.trim();
    let (y, mo, d) = (num(s, 0..4)?, num(s, 5..7)?, num(s, 8..10)?);
    let (h, mi, se) = (num(s, 11..13)?, num(s, 14..16)?, num(s, 17..19)?);
    let mut rest = &s[19..];
    if let Some(r) = rest.strip_prefix('.') {
        let digits = r.find(|c: char| !c.is_ascii_digit()).unwrap_or(r.len());
        rest = &r[digits..];
    }
    let offset = match rest.chars().next() {
        None | Some('Z') | Some('z') => 0,
        Some(sign @ ('+' | '-')) => {
            let oh = num(rest, 1..3)?;
            let om = num(rest, 4..6).unwrap_or(0);
            let o = oh * 3600 + om * 60;
            if sign == '+' {
                o
            } else {
                -o
            }
        }
        _ => return None,
    };
    Some(days_from_civil(y, mo, d) * 86400 + h * 3600 + mi * 60 + se - offset)
}

/// Medianoche local de una fecha `YYYY-MM-DD`.
pub fn local_midnight(date: &str) -> Option<i64> {
    let (y, m, d) = (num(date, 0..4)?, num(date, 5..7)?, num(date, 8..10)?);
    let mut tm: libc::tm = unsafe { std::mem::zeroed() };
    tm.tm_year = (y - 1900) as i32;
    tm.tm_mon = (m - 1) as i32;
    tm.tm_mday = d as i32;
    tm.tm_isdst = -1;
    let t = unsafe { libc::mktime(&mut tm) };
    (t != -1).then_some(t as i64)
}

/// `HH:MM` en hora local, o `DD/MM HH:MM` si falta más de un día.
pub fn clock(t: i64) -> String {
    let mut tm: libc::tm = unsafe { std::mem::zeroed() };
    let tt = t as libc::time_t;
    unsafe { libc::localtime_r(&tt, &mut tm) };
    if t - now() < 86400 {
        format!("{:02}:{:02}", tm.tm_hour, tm.tm_min)
    } else {
        format!("{:02}/{:02} {:02}:{:02}", tm.tm_mday, tm.tm_mon + 1, tm.tm_hour, tm.tm_min)
    }
}

/// Quita secuencias ANSI: CSI (`ESC [ … final`), OSC (`ESC ] … BEL|ESC \`),
/// juego de caracteres (`ESC ( B`) y C1 de dos bytes.
pub fn strip_ansi(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut it = text.chars().peekable();
    while let Some(c) = it.next() {
        if c != '\x1b' {
            out.push(c);
            continue;
        }
        match it.next() {
            Some('[') => {
                for c in it.by_ref() {
                    if ('\x40'..='\x7e').contains(&c) {
                        break;
                    }
                }
            }
            Some(']') => {
                while let Some(c) = it.next() {
                    if c == '\x07' {
                        break;
                    }
                    if c == '\x1b' {
                        it.next();
                        break;
                    }
                }
            }
            Some('(') | Some(')') => {
                it.next();
            }
            _ => {}
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rfc3339() {
        assert_eq!(parse_rfc3339("1970-01-01T00:00:00Z"), Some(0));
        assert_eq!(parse_rfc3339("2026-10-05T03:39:59.639565+00:00"), Some(1791171599));
        assert_eq!(parse_rfc3339("2026-10-04T22:39:59-05:00"), Some(1791171599));
        assert_eq!(parse_rfc3339("nope"), None);
    }

    #[test]
    fn ansi() {
        assert_eq!(strip_ansi("\x1b[1;32mKIRO\x1b[0m ok"), "KIRO ok");
        assert_eq!(strip_ansi("\x1b]0;title\x07x\x1b(By"), "xy");
    }
}
