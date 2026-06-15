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
            s.push_str(&format!(
                "{mark} [{}] {} \u{2014} \"{}\"\n",
                c.person, c.source_path, q
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

/// Does the pin's quote exist in its source? Caches normalized file contents so each
/// source is read and normalized once per run.
fn quote_exists(cache: &mut HashMap<String, String>, pin: &Pin) -> bool {
    let path = store::expand_tilde(&pin.source_path);
    let key = path.display().to_string();
    if !cache.contains_key(&key) {
        // Read the canon view (falsify blocks stripped): a pin must match the REAL source text,
        // never falsify's own rendered quote on a wiki page. Books have no fences → verbatim.
        let content = store::canon_bytes(&path).unwrap_or_default();
        cache.insert(key.clone(), normalize_for_match(&content));
    }
    let norm = &cache[&key];
    let needle = normalize_for_match(&pin.quote);
    // An empty quote is not a pin — and `str::contains("")` is always true, so guard it
    // explicitly or an empty-quoted pin would silently pass the gate.
    if norm.is_empty() || needle.is_empty() {
        return false;
    }
    norm.contains(&needle)
}

pub fn verify_pins(run: &Path) -> Result<PinReport> {
    let pins = collect_pins(run)?;
    let mut cache: HashMap<String, String> = HashMap::new();
    let mut checks = vec![];
    let mut failed = 0;
    for pin in &pins {
        let ok = quote_exists(&mut cache, pin);
        if !ok {
            failed += 1;
        }
        checks.push(PinCheck {
            person: pin.person.clone(),
            source_path: pin.source_path.clone(),
            quote: pin.quote.clone(),
            ok,
        });
    }
    Ok(PinReport { checks, failed })
}
