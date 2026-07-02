//! Mechanical contradiction pre-filter — the cheap, HIGH-PRECISION subset: flag pin
//! pairs that assert DIFFERENT NUMBERS about a shared subject (e.g. "the inliner cuts
//! runtime ~5%" vs "cuts it only 2%"). These are mechanically detectable
//! and rarely false. Semantic / polarity contradictions (negation, antonyms) are
//! deliberately NOT done here — a free-text polarity detector is too imprecise to
//! trust, so that stays the LLM auditor's job. Output is SUGGESTIONS the auditor
//! confirms, never an autonomous finding.

use std::collections::{BTreeMap, BTreeSet};
use std::path::Path;

use anyhow::Result;
use regex::Regex;

use crate::model::*;
use crate::store;

/// A suggested mechanical contradiction between two pins (by index into the slice).
pub struct Suggestion {
    pub a: usize,
    pub b: usize,
    pub reason: String,
}

/// Integers, decimals, and percentages as normalized tokens.
fn numbers(text: &str) -> Vec<String> {
    let re = Regex::new(r"\d+(?:\.\d+)?%?").unwrap();
    re.find_iter(text).map(|m| m.as_str().to_string()).collect()
}

/// Significant content tokens (drop stopwords + short words) for subject overlap.
fn content_tokens(text: &str) -> BTreeSet<String> {
    const STOP: &[&str] = &[
        "the", "and", "for", "are", "was", "were", "that", "this", "with", "from", "have", "has",
        "not", "but", "you", "your", "all", "any", "can", "will", "into", "than", "then", "more",
        "most", "some", "about", "over", "only", "also", "very", "much",
    ];
    normalize_claim(text)
        .split(' ')
        .filter(|t| t.len() > 3 && !STOP.contains(t))
        .map(|t| t.to_string())
        .collect()
}

/// Suggest numeric-conflict pairs: pins sharing ≥2 content tokens but disjoint numbers.
pub fn suggest(pins: &[Pin]) -> Vec<Suggestion> {
    let mut out = vec![];
    for i in 0..pins.len() {
        for j in (i + 1)..pins.len() {
            let shared = content_tokens(&pins[i].quote)
                .intersection(&content_tokens(&pins[j].quote))
                .count();
            if shared < 2 {
                continue;
            }
            let ni = numbers(&pins[i].quote);
            let nj = numbers(&pins[j].quote);
            if ni.is_empty() || nj.is_empty() {
                continue;
            }
            // disjoint number sets about a shared subject → candidate conflict
            if ni.iter().all(|n| !nj.contains(n)) {
                out.push(Suggestion {
                    a: i,
                    b: j,
                    reason: format!("numeric: {ni:?} vs {nj:?}"),
                });
            }
        }
    }
    out
}

/// Group each author's map-fragment pins and format the mechanical numeric-conflict suggestions as
/// lines the auditor confirms — enumeration over the run's audits, never an autonomous finding.
pub fn report(run: &Path) -> Result<String> {
    let audits = store::load_audits(run)?;
    let mut by_author: BTreeMap<String, Vec<Pin>> = BTreeMap::new();
    for a in audits {
        by_author
            .entry(a.author)
            .or_default()
            .extend(a.map_fragments);
    }
    let mut out = String::new();
    for (author, pins) in &by_author {
        for s in suggest(pins) {
            let qa: String = pins[s.a].quote.chars().take(50).collect();
            let qb: String = pins[s.b].quote.chars().take(50).collect();
            out.push_str(&format!(
                "[{author}] {} | a: \"{qa}\" | b: \"{qb}\"\n",
                s.reason
            ));
        }
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn pin(q: &str) -> Pin {
        Pin {
            person: "X".into(),
            source_ref: "r".into(),
            source_path: "p".into(),
            quote: q.into(),
            kind: PinKind::Book,
            gloss: None,
        }
    }

    #[test]
    fn flags_disjoint_numbers_about_shared_subject() {
        let pins = vec![
            pin("the inliner cuts runtime by about 5 percent"),
            pin("the inliner cuts runtime by only 2 percent"),
        ];
        let s = suggest(&pins);
        assert_eq!(s.len(), 1, "expected one numeric conflict");
    }

    #[test]
    fn no_flag_when_numbers_agree_or_subject_differs() {
        // same numbers → not a conflict
        let agree = vec![
            pin("the build takes 8 seconds"),
            pin("the build needs 8 seconds"),
        ];
        assert!(suggest(&agree).is_empty());
        // different subjects → not a conflict even with different numbers
        let unrelated = vec![
            pin("the linker runs in 40 ms"),
            pin("the parser uses 12 threads"),
        ];
        assert!(suggest(&unrelated).is_empty());
    }
}
