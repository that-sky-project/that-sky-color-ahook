//! Runtime hook settings, driven by the UI Settings tab.

use std::sync::Mutex;
use std::sync::atomic::AtomicBool;

/// Skip certificate verification (cert_verify hook active).
pub static SKIP_CERT_VERIFY: AtomicBool = AtomicBool::new(true);
/// Force every request to plain HTTP.
pub static FORCE_HTTP: AtomicBool = AtomicBool::new(false);
/// Apply domain rewrite rules to HTTP requests.
pub static REWRITE_DOMAIN: AtomicBool = AtomicBool::new(true);

/// One rewrite rule: `origin[:port] -> target[:port]`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DomainRule {
    pub origin: String,
    pub origin_port: Option<String>,
    pub target: String,
    pub target_port: Option<String>,
}

static RULES: Mutex<Vec<DomainRule>> = Mutex::new(Vec::new());

/// Snapshot of the active rewrite rules.
pub fn rules() -> Vec<DomainRule> {
    RULES.lock().unwrap_or_else(|p| p.into_inner()).clone()
}

/// Replace the active rewrite rules.
pub fn set_rules(new_rules: Vec<DomainRule>) {
    *RULES.lock().unwrap_or_else(|p| p.into_inner()) = new_rules;
}

/// Parse comma/newline/`;`-separated `origin[:port] -> target[:port]` rules.
pub fn parse_rules(text: &str) -> Vec<DomainRule> {
    let mut out = Vec::new();
    for part in text.split([',', '\n', ';']) {
        let Some((origin, target)) = part.trim().split_once("->") else {
            continue;
        };
        let Some((origin, origin_port)) = parse_host(origin.trim()) else {
            continue;
        };
        let Some((target, target_port)) = parse_host(target.trim()) else {
            continue;
        };
        out.push(DomainRule {
            origin,
            origin_port,
            target,
            target_port,
        });
    }
    out
}

/// Split `host[:port]` (port optional, last colon wins).
fn parse_host(s: &str) -> Option<(String, Option<String>)> {
    if s.is_empty() {
        return None;
    }
    match s.rsplit_once(':') {
        Some((host, port)) if !host.is_empty() && !port.is_empty() => {
            Some((host.to_string(), Some(port.to_string())))
        }
        _ => Some((s.trim_end_matches(':').to_string(), None)),
    }
}
