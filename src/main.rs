//! codenotch — uso de límites de Claude, Codex y Kiro para la barra de Noctalia.
//!
//! El daemon consulta a cada proveedor con calma (con backoff en 429), sigue
//! las sesiones de Claude Code por inotify y escribe un único
//! `$XDG_RUNTIME_DIR/codenotch/state.json`, que pinta el plugin de Noctalia.

mod claude;
mod codex;
mod kiro;
mod model;
mod watch;
mod util;

use model::{FetchError, Reading, Window};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::os::unix::io::AsRawFd;
use std::os::unix::net::UnixDatagram;
use std::path::PathBuf;
use std::process::{Command, Stdio};
use std::sync::mpsc;
use std::time::Duration;
use util::now;

const NOCTALIA_SERVICE: &str = "juanmcanchala/codenotch:service";
/// Cuánto se muestra "terminó" tras pasar una sesión de trabajando a inactiva.
const DONE_SECS: i64 = 180;

// ── Configuración ───────────────────────────────────────────────────────────

#[derive(Deserialize)]
#[serde(default)]
struct Config {
    /// email → nombre corto en la barra.
    aliases: HashMap<String, String>,
    /// Sesiones cuyo nombre empieza así no cuentan (las de claude-mem, p. ej.).
    ignore_sessions: Vec<String>,
    claude_interval: i64,
    /// Mientras alguna sesión de esa cuenta trabaja, la cifra se mueve: se lee más a menudo.
    claude_busy_interval: i64,
    codex_interval: i64,
    kiro_interval: i64,
    codex: bool,
    kiro: bool,
    notify_noctalia: bool,
    /// Notificación de escritorio al cruzar el 80 % y el 100 % de la ventana principal.
    alerts: bool,
}

impl Default for Config {
    fn default() -> Self {
        Config {
            aliases: HashMap::new(),
            ignore_sessions: vec!["observer-sessions".into()],
            claude_interval: 300,
            claude_busy_interval: 60,
            codex_interval: 300,
            kiro_interval: 900,
            codex: true,
            kiro: true,
            notify_noctalia: true,
            alerts: true,
        }
    }
}

fn load_config() -> Config {
    let path = util::config_dir().join("config.json");
    match std::fs::read_to_string(&path) {
        Ok(text) => serde_json::from_str(&text).unwrap_or_else(|e| {
            eprintln!("codenotch: {} inválido ({e}); uso valores por defecto", path.display());
            Config::default()
        }),
        Err(_) => Config::default(),
    }
}

// ── Proveedores ─────────────────────────────────────────────────────────────

#[derive(Clone)]
enum Kind {
    Claude(Vec<PathBuf>),
    Codex,
    Kiro(PathBuf),
}

impl Kind {
    fn name(&self) -> &'static str {
        match self {
            Kind::Claude(_) => "claude",
            Kind::Codex => "codex",
            Kind::Kiro(_) => "kiro",
        }
    }
}

struct Provider {
    id: String,
    kind: Kind,
    label: String,
    account: Option<String>,
    interval: i64,
    reading: Option<Reading>,
    error: Option<String>,
    auth_error: bool,
    next_at: i64,
    failures: u32,
    in_flight: bool,
    /// Último umbral avisado (0, 80 o 100) y el reinicio de la ventana en que se avisó.
    alerted: u8,
    alerted_reset: Option<i64>,
}

fn discover(cfg: &Config) -> Vec<Provider> {
    let mut out = Vec::new();
    let mk = |id: String, kind: Kind, label: String, account: Option<String>, interval: i64| Provider {
        id,
        kind,
        label,
        account,
        interval,
        reading: None,
        error: None,
        auth_error: false,
        next_at: 0,
        failures: 0,
        in_flight: false,
        alerted: 0,
        alerted_reset: None,
    };
    for acc in claude::discover() {
        let label = cfg.aliases.get(&acc.email).cloned().unwrap_or_else(|| {
            let local = acc.email.split('@').next().unwrap_or(&acc.email);
            local.split(['.', '_', '-']).next().unwrap_or(local).chars().take(12).collect()
        });
        out.push(mk(
            format!("claude:{}", acc.email),
            Kind::Claude(acc.dirs),
            label,
            Some(acc.email),
            cfg.claude_interval,
        ));
    }
    if cfg.codex && codex::available() {
        out.push(mk("codex".into(), Kind::Codex, "Codex".into(), None, cfg.codex_interval));
    }
    if cfg.kiro {
        if let Some(bin) = kiro::locate() {
            out.push(mk("kiro".into(), Kind::Kiro(bin), "Kiro".into(), None, cfg.kiro_interval));
        }
    }
    out
}

fn fetch(agent: &ureq::Agent, kind: &Kind) -> Result<Reading, FetchError> {
    match kind {
        Kind::Claude(dirs) => claude::fetch(agent, dirs),
        Kind::Codex => codex::fetch(agent),
        Kind::Kiro(bin) => kiro::fetch(bin),
    }
}

fn agent() -> ureq::Agent {
    ureq::AgentBuilder::new()
        .timeout(Duration::from_secs(20))
        .user_agent(concat!("codenotch-linux/", env!("CARGO_PKG_VERSION")))
        .build()
}

// ── Caché en disco ──────────────────────────────────────────────────────────

#[derive(Serialize, Deserialize)]
struct Cached {
    reading: Option<Reading>,
    error: Option<String>,
    #[serde(default)]
    auth_error: bool,
    next_at: i64,
    failures: u32,
    #[serde(default)]
    alerted: u8,
    #[serde(default)]
    alerted_reset: Option<i64>,
}

fn cache_path() -> PathBuf {
    util::state_dir().join("cache.json")
}

fn load_cache(providers: &mut [Provider]) {
    let Ok(text) = std::fs::read_to_string(cache_path()) else {
        return;
    };
    let Ok(mut map) = serde_json::from_str::<HashMap<String, Cached>>(&text) else {
        return;
    };
    for p in providers.iter_mut() {
        if let Some(c) = map.remove(&p.id) {
            p.reading = c.reading;
            p.error = c.error;
            p.auth_error = c.auth_error;
            p.failures = c.failures;
            p.alerted = c.alerted;
            p.alerted_reset = c.alerted_reset;
            // Un error de sesión puede haberse arreglado mientras no corríamos.
            p.next_at = if c.auth_error { 0 } else { c.next_at };
        }
    }
}

fn save_cache(providers: &[Provider]) {
    let map: HashMap<&str, Cached> = providers
        .iter()
        .map(|p| {
            (
                p.id.as_str(),
                Cached {
                    reading: p.reading.clone(),
                    error: p.error.clone(),
                    auth_error: p.auth_error,
                    next_at: p.next_at,
                    failures: p.failures,
                    alerted: p.alerted,
                    alerted_reset: p.alerted_reset,
                },
            )
        })
        .collect();
    if let Ok(json) = serde_json::to_vec(&map) {
        let _ = util::write_atomic(&cache_path(), &json);
    }
}

// ── Actividad de Claude Code ────────────────────────────────────────────────

#[derive(Serialize, Clone, PartialEq)]
struct OutSession {
    name: String,
    state: &'static str,
    project: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    waiting_for: Option<String>,
}

#[derive(Serialize, Clone, PartialEq)]
struct Activity {
    state: &'static str,
    busy: usize,
    waiting: usize,
    sessions: Vec<OutSession>,
}

#[derive(Default)]
struct Tracker {
    prev: HashMap<String, String>,
    done_until: HashMap<String, i64>,
}

impl Tracker {
    /// Agrupa las sesiones vivas por cuenta y deduce el estado de cada una.
    fn compute(&mut self, providers: &[Provider], ignore: &[String], now: i64) -> HashMap<String, Activity> {
        let mut seen = HashMap::new();
        let mut out: HashMap<String, Activity> = HashMap::new();
        for p in providers {
            let Kind::Claude(dirs) = &p.kind else { continue };
            let mut sessions = Vec::new();
            for dir in dirs {
                for s in claude::scan_sessions(dir, ignore) {
                    let prev = self.prev.get(&s.session_id).map(String::as_str);
                    let state = match s.status.as_str() {
                        "busy" => "busy",
                        "waiting" => "waiting",
                        _ => {
                            if matches!(prev, Some("busy" | "waiting")) {
                                self.done_until.insert(s.session_id.clone(), now + DONE_SECS);
                            }
                            if self.done_until.get(&s.session_id).is_some_and(|&t| t > now) {
                                "done"
                            } else {
                                "idle"
                            }
                        }
                    };
                    seen.insert(s.session_id.clone(), s.status.clone());
                    let project = std::path::Path::new(&s.cwd)
                        .file_name()
                        .map(|n| n.to_string_lossy().into_owned())
                        .unwrap_or_default();
                    sessions.push(OutSession {
                        name: if s.name.is_empty() { project.clone() } else { s.name },
                        state,
                        project,
                        waiting_for: s.waiting_for,
                    });
                }
            }
            let count = |st: &str| sessions.iter().filter(|s| s.state == st).count();
            let state = if count("waiting") > 0 {
                "waiting"
            } else if count("busy") > 0 {
                "busy"
            } else if count("done") > 0 {
                "done"
            } else {
                "idle"
            };
            out.insert(
                p.id.clone(),
                Activity { state, busy: count("busy"), waiting: count("waiting"), sessions },
            );
        }
        self.done_until.retain(|id, t| *t > now && seen.contains_key(id));
        self.prev = seen;
        out
    }

    fn next_expiry(&self) -> Option<i64> {
        self.done_until.values().copied().min()
    }
}

// ── Salida ──────────────────────────────────────────────────────────────────

#[derive(Serialize, PartialEq)]
struct OutProvider {
    id: String,
    kind: &'static str,
    label: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    account: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    plan: Option<String>,
    windows: Vec<Window>,
    #[serde(skip_serializing_if = "Option::is_none")]
    fetched_at: Option<i64>,
    stale: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    error: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    activity: Option<Activity>,
}

fn snapshot(providers: &[Provider], activity: &HashMap<String, Activity>, now: i64) -> Vec<OutProvider> {
    providers
        .iter()
        .map(|p| {
            let r = p.reading.as_ref();
            OutProvider {
                id: p.id.clone(),
                kind: p.kind.name(),
                label: p.label.clone(),
                account: p.account.clone(),
                plan: r.and_then(|r| r.plan.clone()),
                windows: r.map(|r| r.windows.clone()).unwrap_or_default(),
                fetched_at: r.map(|r| r.fetched_at),
                stale: r.is_none_or(|r| p.error.is_some() || now - r.fetched_at > 3 * p.interval),
                error: p.error.clone(),
                activity: activity.get(&p.id).cloned(),
            }
        })
        .collect()
}

fn notify_noctalia() {
    std::thread::spawn(|| {
        let _ = Command::new("noctalia")
            .args(["msg", "plugin", NOCTALIA_SERVICE, "all", "update"])
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status();
    });
}

// ── Planificación y alertas ─────────────────────────────────────────────────

/// Adelanta la próxima consulta cuando la cifra está a punto de moverse:
/// una sesión trabajando, o una ventana que acaba de reiniciarse.
fn adjust_schedule(p: &mut Provider, activity: &HashMap<String, Activity>, busy_interval: i64) {
    if p.failures != 0 || p.in_flight {
        return;
    }
    let Some(r) = &p.reading else { return };
    if activity.get(&p.id).is_some_and(|a| a.state == "busy") {
        p.next_at = p.next_at.min(r.fetched_at + busy_interval);
    }
    for t in r.windows.iter().filter_map(|w| w.resets_at) {
        if t > r.fetched_at {
            // Unos segundos de margen para que el proveedor ya haya rotado la ventana.
            p.next_at = p.next_at.min(t + 5);
        }
    }
}

/// Avisa una vez al cruzar el 80 % y el 100 % de la ventana principal, y de
/// nuevo solo cuando esa ventana se reinicia.
fn check_alert(p: &mut Provider) {
    let Some(w) = p.reading.as_ref().and_then(|r| r.windows.first()) else {
        return;
    };
    // El reinicio de Claude baila unos milisegundos entre lecturas: misma
    // ventana si difiere menos de dos minutos.
    let same_window = match (p.alerted_reset, w.resets_at) {
        (Some(a), Some(b)) => (a - b).abs() < 120,
        (a, b) => a == b,
    };
    if !same_window {
        p.alerted = 0;
        p.alerted_reset = w.resets_at;
    }
    let level = if w.used >= 0.995 {
        100
    } else if w.used >= 0.8 {
        80
    } else {
        0
    };
    if level <= p.alerted {
        return;
    }
    p.alerted = level;
    let who = match p.kind {
        Kind::Claude(_) => format!("Claude {}", p.label),
        _ => p.label.clone(),
    };
    let title = format!("{who}: {:.0}% de {}", w.used * 100.0, w.label);
    let body = w
        .resets_at
        .map(|t| format!("Reinicia a las {}", util::clock(t)))
        .unwrap_or_default();
    let urgency = if level == 100 { "critical" } else { "normal" };
    if cfg!(test) {
        return;
    }
    std::thread::spawn(move || {
        let _ = Command::new("notify-send")
            .args(["-a", "Codenotch", "-u", urgency, "-i", "dialog-warning", &title, &body])
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status();
    });
}

// ── Daemon ──────────────────────────────────────────────────────────────────

enum Ev {
    Fetched(String, Result<Reading, FetchError>),
    Files(Vec<String>),
    Refresh,
}

fn lock_single_instance() -> Option<std::fs::File> {
    let dir = util::runtime_dir();
    std::fs::create_dir_all(&dir).ok()?;
    let f = std::fs::File::create(dir.join("daemon.lock")).ok()?;
    let ok = unsafe { libc::flock(f.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB) } == 0;
    ok.then_some(f)
}

fn apply(p: &mut Provider, res: Result<Reading, FetchError>, now: i64) {
    p.in_flight = false;
    match res {
        Ok(r) => {
            p.reading = Some(r);
            p.error = None;
            p.auth_error = false;
            p.failures = 0;
            p.next_at = now + p.interval;
        }
        Err(e) => {
            p.error = Some(e.message());
            p.failures = p.failures.saturating_add(1);
            p.auth_error = matches!(e, FetchError::Auth(_));
            let backoff = |base: i64, cap: i64| (base << (p.failures.min(6) - 1)).min(cap);
            p.next_at = now
                + match e {
                    // Lo arregla el usuario; el cambio de archivo nos despierta antes.
                    FetchError::Auth(_) => p.interval.max(600),
                    // Anthropic responde `Retry-After: 0`: el valor solo sube el
                    // mínimo; la espera se dobla con cada 429 seguido, hasta 15 min.
                    FetchError::RateLimited(s) => s.unwrap_or(0).max(backoff(60, 900)).min(3600),
                    FetchError::Other(_) => backoff(60, 900),
                };
        }
    }
}

fn daemon() {
    let Some(_lock) = lock_single_instance() else {
        eprintln!("codenotch: ya hay un daemon corriendo");
        std::process::exit(1);
    };
    let cfg = load_config();
    let mut providers = discover(&cfg);
    load_cache(&mut providers);
    eprintln!(
        "codenotch: vigilando {}",
        providers.iter().map(|p| p.id.as_str()).collect::<Vec<_>>().join(", ")
    );

    let (tx, rx) = mpsc::channel::<Ev>();

    // inotify: sesiones y credenciales de cada perfil de Claude, y la de Codex.
    if let Some(w) = watch::Watcher::new() {
        for p in &providers {
            match &p.kind {
                Kind::Claude(dirs) => {
                    for d in dirs {
                        w.add(d);
                        w.add(&d.join("sessions"));
                    }
                }
                Kind::Codex => {
                    w.add(&util::home().join(".codex"));
                }
                Kind::Kiro(_) => {}
            }
        }
        let tx = tx.clone();
        std::thread::spawn(move || loop {
            let names = w.wait();
            if !names.is_empty() && tx.send(Ev::Files(names)).is_err() {
                break;
            }
        });
    }

    // Socket para `codenotch refresh` (clic derecho en la barra).
    let sock = util::socket_path();
    let _ = std::fs::remove_file(&sock);
    if let Ok(s) = UnixDatagram::bind(&sock) {
        let tx = tx.clone();
        std::thread::spawn(move || {
            let mut buf = [0u8; 64];
            while let Ok(n) = s.recv(&mut buf) {
                if &buf[..n] == b"refresh" && tx.send(Ev::Refresh).is_err() {
                    break;
                }
            }
        });
    }

    let agent = agent();
    let mut tracker = Tracker::default();
    let mut activity = tracker.compute(&providers, &cfg.ignore_sessions, now());
    let mut last_out = String::new();
    let state_file = util::state_file();
    let mut prev_states: HashMap<String, &'static str> = HashMap::new();

    loop {
        let now = now();
        for p in providers.iter_mut() {
            adjust_schedule(p, &activity, cfg.claude_busy_interval);
        }
        for p in providers.iter_mut().filter(|p| !p.in_flight && p.next_at <= now) {
            p.in_flight = true;
            let (tx, id, kind, agent) = (tx.clone(), p.id.clone(), p.kind.clone(), agent.clone());
            std::thread::spawn(move || {
                let res = fetch(&agent, &kind);
                let _ = tx.send(Ev::Fetched(id, res));
            });
        }

        // Escribe solo si algo cambió.
        let out = serde_json::to_string(&snapshot(&providers, &activity, now)).unwrap_or_default();
        if out != last_out {
            let doc = format!("{{\"generated_at\":{now},\"providers\":{out}}}");
            if util::write_atomic(&state_file, doc.as_bytes()).is_ok() && cfg.notify_noctalia {
                notify_noctalia();
            }
            last_out = out;
        }

        // Duerme hasta la próxima consulta, el fin de un "terminó" o 60 s
        // (para notar sesiones que murieron sin borrar su archivo).
        let wake = providers
            .iter()
            .filter(|p| !p.in_flight)
            .map(|p| p.next_at)
            .chain(tracker.next_expiry())
            .min()
            .unwrap_or(now + 60)
            .min(now + 60);
        let timeout = Duration::from_secs((wake - now).max(1) as u64);

        let mut rescan = false;
        let mut cache_dirty = false;
        let mut handle = |ev: Ev, providers: &mut Vec<Provider>, rescan: &mut bool| match ev {
            Ev::Fetched(id, res) => {
                if let Some(p) = providers.iter_mut().find(|p| p.id == id) {
                    apply(p, res, util::now());
                    if cfg.alerts {
                        check_alert(p);
                    }
                    cache_dirty = true;
                }
            }
            Ev::Files(names) => {
                for n in &names {
                    match n.as_str() {
                        ".credentials.json" => providers
                            .iter_mut()
                            .filter(|p| p.auth_error && matches!(p.kind, Kind::Claude(_)))
                            .for_each(|p| p.next_at = 0),
                        // `codex login` puede ser otra cuenta u otro plan: se
                        // lee ya, salvo que un 429 diga que hay que esperar.
                        "auth.json" => providers
                            .iter_mut()
                            .filter(|p| matches!(p.kind, Kind::Codex))
                            .filter(|p| p.auth_error || p.failures == 0)
                            .for_each(|p| p.next_at = 0),
                        n if n.ends_with(".json") && n.as_bytes()[0].is_ascii_digit() => *rescan = true,
                        _ => {}
                    }
                }
            }
            Ev::Refresh => {
                let now = util::now();
                for p in providers.iter_mut() {
                    // No se salta un 429 (el servidor ya dijo cuándo) ni repite
                    // una lectura de hace menos de un minuto por clics seguidos.
                    let fresh = p.reading.as_ref().is_some_and(|r| now - r.fetched_at < 60);
                    if (p.failures == 0 && !fresh) || p.auth_error {
                        p.next_at = 0;
                    }
                }
                *rescan = true;
            }
        };
        match rx.recv_timeout(timeout) {
            Ok(ev) => {
                handle(ev, &mut providers, &mut rescan);
                // Claude escribe varios archivos seguidos: se agrupan en un lote.
                std::thread::sleep(Duration::from_millis(120));
                while let Ok(ev) = rx.try_recv() {
                    handle(ev, &mut providers, &mut rescan);
                }
            }
            Err(mpsc::RecvTimeoutError::Timeout) => rescan = true,
            Err(mpsc::RecvTimeoutError::Disconnected) => break,
        }
        if rescan {
            let now = util::now();
            activity = tracker.compute(&providers, &cfg.ignore_sessions, now);
            // Una sesión que deja de trabajar acaba de gastar: una lectura ya,
            // para que el total aparezca en segundos y no en cinco minutos.
            for p in providers.iter_mut() {
                let was = prev_states.get(&p.id).copied();
                let is = activity.get(&p.id).map(|a| a.state);
                let stopped = was == Some("busy") && matches!(is, Some("done" | "idle"));
                let fresh = p.reading.as_ref().is_some_and(|r| now - r.fetched_at < 15);
                if stopped && p.failures == 0 && !p.in_flight && !fresh {
                    p.next_at = 0;
                }
            }
            prev_states = activity.iter().map(|(k, a)| (k.clone(), a.state)).collect();
        }
        if cache_dirty {
            save_cache(&providers);
        }
    }
}

// ── Comandos auxiliares ─────────────────────────────────────────────────────

fn refresh() {
    let s = UnixDatagram::unbound().expect("socket");
    if s.send_to(b"refresh", util::socket_path()).is_err() {
        eprintln!("codenotch: el daemon no está corriendo (systemctl --user start codenotch)");
        std::process::exit(1);
    }
}

fn fmt_reset(t: Option<i64>, now: i64) -> String {
    let Some(t) = t else { return String::new() };
    let d = t - now;
    if d <= 0 {
        return " · reinicia ya".into();
    }
    let (days, h, m) = (d / 86400, d % 86400 / 3600, d % 3600 / 60);
    if days > 0 {
        format!(" · reinicia en {days} d {h} h")
    } else if h > 0 {
        format!(" · reinicia en {h} h {m} min")
    } else {
        format!(" · reinicia en {m} min")
    }
}

fn status() {
    let Some(v) = util::read_json(&util::state_file()) else {
        eprintln!("codenotch: no hay estado todavía (¿está corriendo el daemon?)");
        std::process::exit(1);
    };
    let now = now();
    for p in v["providers"].as_array().into_iter().flatten() {
        let label = p["label"].as_str().unwrap_or("?");
        let kind = p["kind"].as_str().unwrap_or("?");
        let plan = p["plan"].as_str().map(|s| format!(" ({s})")).unwrap_or_default();
        let act = p["activity"]["state"].as_str().map(|s| format!(" — {s}")).unwrap_or_default();
        let stale = if p["stale"] == true { " [desactualizado]" } else { "" };
        println!("{kind}:{label}{plan}{act}{stale}");
        for w in p["windows"].as_array().into_iter().flatten() {
            let used = w["used"].as_f64().unwrap_or(0.0) * 100.0;
            let detail = w["detail"].as_str().map(|d| format!(" · {d}")).unwrap_or_default();
            println!(
                "  {:<18} {:>5.1}%{}{}",
                w["label"].as_str().unwrap_or(""),
                used,
                detail,
                fmt_reset(w["resets_at"].as_i64(), now)
            );
        }
        if let Some(e) = p["error"].as_str() {
            println!("  ! {e}");
        }
    }
}

/// Consulta todo una vez, sin daemon ni caché, e imprime el JSON.
fn once() {
    let cfg = load_config();
    let agent = agent();
    let now = now();
    let mut providers = discover(&cfg);
    for p in providers.iter_mut() {
        let res = fetch(&agent, &p.kind);
        apply(p, res, now);
    }
    let activity = Tracker::default().compute(&providers, &cfg.ignore_sessions, now);
    println!("{}", serde_json::to_string_pretty(&snapshot(&providers, &activity, now)).unwrap());
}

fn main() {
    match std::env::args().nth(1).as_deref() {
        None | Some("daemon") => daemon(),
        Some("refresh") => refresh(),
        Some("status") => status(),
        Some("once") => once(),
        Some("-V" | "--version") => println!("codenotch {}", env!("CARGO_PKG_VERSION")),
        _ => {
            eprintln!("uso: codenotch [daemon|refresh|status|once|--version]");
            std::process::exit(2);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn provider(used: f64, resets_at: Option<i64>, fetched_at: i64) -> Provider {
        Provider {
            id: "claude:a@b".into(),
            kind: Kind::Codex,
            label: "Codex".into(),
            account: None,
            interval: 300,
            reading: Some(Reading {
                windows: vec![Window {
                    id: "session".into(),
                    label: "5 h".into(),
                    used,
                    resets_at,
                    duration: None,
                    detail: None,
                }],
                plan: None,
                fetched_at,
            }),
            error: None,
            auth_error: false,
            next_at: fetched_at + 300,
            failures: 0,
            in_flight: false,
            alerted: 0,
            alerted_reset: None,
        }
    }

    #[test]
    fn alert_once_per_crossing_and_window() {
        let mut p = provider(0.85, Some(1000), 0);
        check_alert(&mut p);
        assert_eq!(p.alerted, 80);
        // Misma ventana (con baile de milisegundos): no repite.
        p.reading.as_mut().unwrap().windows[0].resets_at = Some(1001);
        check_alert(&mut p);
        assert_eq!(p.alerted, 80);
        p.reading.as_mut().unwrap().windows[0].used = 1.0;
        check_alert(&mut p);
        assert_eq!(p.alerted, 100);
        // Ventana nueva por debajo del umbral: se rearma.
        let w = &mut p.reading.as_mut().unwrap().windows[0];
        w.used = 0.1;
        w.resets_at = Some(19000);
        check_alert(&mut p);
        assert_eq!(p.alerted, 0);
    }

    #[test]
    fn busy_and_rollover_pull_next_read_forward() {
        let mut p = provider(0.2, Some(100), 0);
        adjust_schedule(&mut p, &HashMap::new(), 60);
        assert_eq!(p.next_at, 105, "reinicio de ventana");

        let mut p = provider(0.2, None, 0);
        let mut act = HashMap::new();
        act.insert(
            p.id.clone(),
            Activity { state: "busy", busy: 1, waiting: 0, sessions: vec![] },
        );
        adjust_schedule(&mut p, &act, 60);
        assert_eq!(p.next_at, 60, "sesión trabajando");

        p.failures = 1;
        p.next_at = 900;
        adjust_schedule(&mut p, &act, 60);
        assert_eq!(p.next_at, 900, "un 429 manda sobre la actividad");
    }

    #[test]
    fn rate_limit_backoff_ignores_zero_retry_after() {
        let mut p = provider(0.2, None, 0);
        apply(&mut p, Err(FetchError::RateLimited(Some(0))), 0);
        assert_eq!(p.next_at, 60);
        apply(&mut p, Err(FetchError::RateLimited(Some(0))), 0);
        assert_eq!(p.next_at, 120);
        for _ in 0..6 {
            apply(&mut p, Err(FetchError::RateLimited(Some(0))), 0);
        }
        assert_eq!(p.next_at, 900);
    }
}
