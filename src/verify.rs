//! The hard gate (PRESENCE half of the contract): every pin's verbatim quote must exist
//! (whitespace/punctuation-normalized) in its named source. Runs over the audit artifacts
//! — map fragments, both sides of every contradiction pair, and load-bearing verdict pins
//! — not just the rendered page. Any failure aborts the write: the LLM cannot fabricate
//! evidence.
//!
//! Presence is checked CASE-SENSITIVELY (a verbatim quote must match its source faithfully,
//! tolerant only of line-wrap and quote-char drift). This is deliberately asymmetric with
//! the ABSENCE half (`verify-evidence`), which is case-INsensitive — a capitalized
//! occurrence must still refute a silence claim. Two questions, two correct case policies.

use std::collections::HashMap;
use std::path::Path;

use anyhow::Result;

use crate::model::*;
use crate::store;

pub struct PinCheck {
    pub person: String,
    pub source_path: String,
    pub quote: String,
    pub ok: bool,
    /// Empty when `ok`; else why the pin failed (source missing / unreadable / empty quote / quote
    /// absent) — so a FAIL is diagnosable instead of one undifferentiated bare failure.
    pub detail: String,
}

pub struct PinReport {
    pub checks: Vec<PinCheck>,
    pub failed: usize,
}

impl PinReport {
    pub fn render(&self) -> String {
        let mut s = String::new();
        for c in &self.checks {
            let mark = if c.ok { "OK  " } else { "FAIL" };
            let q: String = c.quote.chars().take(60).collect();
            let why = if c.ok {
                String::new()
            } else {
                format!("  ({})", c.detail)
            };
            s.push_str(&format!(
                "{mark} [{}] {} \u{2014} \"{}\"{}\n",
                c.person, c.source_path, q, why
            ));
        }
        s.push_str(&format!(
            "\n{} pins, {} failed\n",
            self.checks.len(),
            self.failed
        ));
        s
    }
}

/// Every pin in a run's audits + verdicts: map fragments, both sides of each contradiction,
/// and load-bearing verdict pins. Pure over already-loaded data so callers (`verify-pins`,
/// `verify-evidence`'s slice-freeze, `persist`'s input-pin re-check) share one definition of
/// "the load-bearing pins."
pub fn pins_of(audits: &[Audit], verdicts: &[Verdict]) -> Vec<Pin> {
    let mut pins = vec![];
    for a in audits {
        pins.extend(a.map_fragments.iter().cloned());
        for c in &a.contradictions {
            pins.push(c.a.clone());
            pins.push(c.b.clone());
        }
    }
    for v in verdicts {
        if let Some(p) = &v.load_bearing_pin {
            pins.push(p.clone());
        }
    }
    pins
}

fn collect_pins(run: &Path) -> Result<Vec<Pin>> {
    Ok(pins_of(
        &store::load_audits(run)?,
        &store::load_verdicts(run)?,
    ))
}

/// Does the pin's quote exist in its source? Returns `(ok, detail)` — `detail` names WHY a check
/// failed (source missing / unreadable / empty quote / quote absent) instead of collapsing all four
/// into one bare FAIL. Caches normalized file contents so each readable source is read once per run.
fn quote_status(cache: &mut HashMap<String, Option<String>>, pin: &Pin) -> (bool, String) {
    let path = store::expand_tilde(&pin.source_path);
    let key = path.display().to_string();
    if !cache.contains_key(&key) {
        if !path.exists() {
            cache.insert(key.clone(), None);
        } else {
            // Read the canon view (falsify blocks stripped): a pin must match the REAL source text,
            // never falsify's own rendered quote on a wiki page. Books have no fences → verbatim.
            // A read/parse error (non-UTF8, malformed fence) is distinct from "absent" — record it.
            match store::canon_bytes(&path) {
                Ok(content) => cache.insert(key.clone(), Some(normalize_for_match(&content))),
                Err(_) => cache.insert(key.clone(), None),
            };
        }
    }
    let needle = normalize_for_match(&pin.quote);
    // An empty quote is not a pin — and `str::contains("")` is always true, so guard it explicitly
    // or an empty-quoted pin would silently pass the gate.
    if needle.is_empty() {
        return (false, "empty quote (not a pin)".to_string());
    }
    let Some(norm) = &cache[&key] else {
        // None means the source could not be read as canon text.
        let why = if path.exists() {
            "source unreadable (non-UTF8 or malformed fence)"
        } else {
            "source file not found"
        };
        return (false, why.to_string());
    };
    if norm.is_empty() {
        return (false, "source has no canon text".to_string());
    }
    if norm.contains(&needle) {
        (true, String::new())
    } else {
        (false, "quote absent from source".to_string())
    }
}

pub fn verify_pins(run: &Path) -> Result<PinReport> {
    let pins = collect_pins(run)?;
    let mut cache: HashMap<String, Option<String>> = HashMap::new();
    let mut checks = vec![];
    let mut failed = 0;
    for pin in &pins {
        let (ok, detail) = quote_status(&mut cache, pin);
        if !ok {
            failed += 1;
        }
        checks.push(PinCheck {
            person: pin.person.clone(),
            source_path: pin.source_path.clone(),
            quote: pin.quote.clone(),
            ok,
            detail,
        });
    }
    Ok(PinReport { checks, failed })
}
