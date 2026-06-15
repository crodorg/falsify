//! Computed coverage report — regenerated from the run's audits, verdicts, and the frozen
//! input slice (manifest), never stored. It reports the load-bearing scope and NAMES THE GAP
//! between what the LLM audited and the book corpora discoverable on disk. There is no "N of
//! M" completeness fraction: the numerator (what the LLM chose to audit) is LLM-controlled and
//! gameable, so it is reported as an honest count with that caveat, not dressed as a percentage.

use std::collections::BTreeSet;
use std::path::Path;

use anyhow::Result;

use crate::model::*;
use crate::store;

pub fn coverage(run: &Path) -> Result<String> {
    let claims = store::load_claims(run)?;
    let audits = store::load_audits(run)?;
    let verdicts = store::load_verdicts(run)?;
    let manifest = store::load_manifest(run)?;

    let audited_authors: BTreeSet<&str> = audits.iter().map(|a| a.author.as_str()).collect();
    let enumerable = store::enumerable_authors();
    let not_audited: Vec<&str> = enumerable
        .iter()
        .map(|(k, _)| k.as_str())
        .filter(|k| !audited_authors.contains(k))
        .collect();
    // K of M is over DISCOVERABLE book corpora only — audit authors that aren't a book corpus
    // (e.g. "blackpill thread (source)", the source self-contradiction) must not inflate K.
    let audited_corpora = enumerable
        .iter()
        .filter(|(k, _)| audited_authors.contains(k.as_str()))
        .count();

    let unaddressed: Vec<&Audit> = audits.iter().filter(|a| a.silence.is_some()).collect();
    let nei = verdicts.iter().filter(|v| v.label == Label::Nei).count();
    let audited_ids: BTreeSet<&str> = audits.iter().map(|a| a.claim_id.as_str()).collect();
    let no_audit: Vec<&Claim> = claims
        .iter()
        .filter(|c| !audited_ids.contains(c.id.as_str()))
        .collect();

    let mut s = String::new();
    s.push_str("# falsify coverage\n\n");
    s.push_str(&format!("claims: {}\n", claims.len()));
    s.push_str(&format!(
        "authors audited: {} ({} of {} discoverable book corpora — LLM-selected scope, not a completeness guarantee)\n",
        if audited_authors.is_empty() {
            "(none)".to_string()
        } else {
            audited_authors.iter().copied().collect::<Vec<_>>().join(", ")
        },
        audited_corpora,
        enumerable.len()
    ));
    if !not_audited.is_empty() {
        s.push_str(&format!(
            "discoverable but NOT audited: {}\n",
            not_audited.join(", ")
        ));
    }
    s.push_str(&format!(
        "load-bearing files frozen (verified pins + silence scope): {}\n",
        manifest.corpus_touched.len()
    ));
    s.push_str(&format!("verdicts: {} (NEI: {})\n", verdicts.len(), nei));
    s.push_str(&format!(
        "UNADDRESSED (silence) flags: {}\n",
        unaddressed.len()
    ));
    for a in &unaddressed {
        let (n, kind) = a
            .silence
            .as_ref()
            .map(|sf| {
                let kind = match sf.scope {
                    SilenceScopeKind::Wiki => "wiki canon file",
                    SilenceScopeKind::AuthorBooks => "book",
                };
                (sf.corpus_scope.len(), kind)
            })
            .unwrap_or((0, "book"));
        s.push_str(&format!(
            "  - claim {} : {} silent (verified absent across {} {}(s))\n",
            a.claim_id, a.author, n, kind
        ));
    }
    if !no_audit.is_empty() {
        s.push_str(&format!("claims with NO canon audit: {}\n", no_audit.len()));
        for c in &no_audit {
            let g: String = c.claim.chars().take(60).collect();
            s.push_str(&format!("  - {} : {}\n", c.id, g));
        }
    }
    Ok(s)
}
