//! verify-evidence — falsify validates the LLM's DECLARED evidence; it never discovers.
//! The two halves of the contract are two commands: `verify-pins` confirms PRESENCE (a
//! quote exists — case-sensitive, verbatim); this confirms ABSENCE (a silence claim) by
//! *attempting to falsify it* — re-grepping every lexical variant across the author's whole
//! book corpus and failing the run if any occurrence turns up (case-INsensitive: a
//! capitalized hit still refutes "the author never says X").
//!
//! This is verification, not discovery: it returns pass/fail + the counterexample, never a
//! hit-list for the LLM to pin from. Discovery is the orchestrator's job (wiki-query, grep,
//! reads). Because falsify (not the LLM) enumerates the absence scope — the author's *full*
//! `books/txt` corpus — the silence claim can't be gamed by a narrow declared slice.
//!
//! It also freezes the run's input slice: every load-bearing file (pin sources + silence
//! scopes), content-hashed into the manifest, so `persist` can refuse to write against canon
//! that drifted since the audit. v1.1 absence is books-only (transcripts are a fast-follow).

use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};

use anyhow::{bail, Context, Result};

use crate::model::*;
use crate::store;
use crate::verify;

const CONTEXT_PAD: usize = 90; // chars of context each side of a refuting hit

/// The author's full book corpus — every `<author>/books/txt/*.txt` discoverable under the corpus
/// root at any depth, unioned and sorted. Recursive (via `store::author_book_dirs`) so corpora may
/// nest under domain folders (e.g. ~/data/<domain>/<author>/books/txt) without restructuring.
fn book_files(author: &str) -> Result<Vec<PathBuf>> {
    let mut files = vec![];
    for (key, dir) in store::author_book_dirs() {
        if key != author {
            continue;
        }
        for entry in fs::read_dir(&dir).with_context(|| format!("read_dir {}", dir.display()))? {
            let p = entry?.path();
            if p.extension().map(|e| e == "txt").unwrap_or(false) {
                files.push(p);
            }
        }
    }
    files.sort();
    files.dedup();
    Ok(files)
}

/// Case-INsensitive, non-overlapping char-index matches of `needle` in `hay`. Absence is
/// checked case-insensitively on purpose (see module doc); presence pins stay case-sensitive
/// in `verify-pins`.
fn find_ci(hay: &[char], needle: &[char]) -> Vec<usize> {
    let mut out = vec![];
    if needle.is_empty() || needle.len() > hay.len() {
        return out;
    }
    let mut i = 0;
    while i + needle.len() <= hay.len() {
        if (0..needle.len()).all(|j| hay[i + j].eq_ignore_ascii_case(&needle[j])) {
            out.push(i);
            i += needle.len();
        } else {
            i += 1;
        }
    }
    out
}

fn window(chars: &[char], at: usize, mlen: usize, pad: usize) -> String {
    let start = at.saturating_sub(pad);
    let end = (at + mlen + pad).min(chars.len());
    chars[start..end]
        .iter()
        .collect::<String>()
        .replace('\n', " ")
}

pub struct EvidenceReport {
    pub lines: Vec<String>,
    pub failed: usize,
}

impl EvidenceReport {
    pub fn render(&self) -> String {
        let mut s = String::new();
        for l in &self.lines {
            s.push_str(l);
            s.push('\n');
        }
        s
    }
}

/// Validate every silence flag (absence) and freeze the load-bearing slice. On success,
/// writes the validated scope back into `audits.json` and the frozen hashes into the
/// manifest. On any failure (a silence claim refuted), nothing is written — the operator
/// fixes the audit and re-runs. Exit-1 is the caller's job when `failed > 0`.
pub fn verify_evidence(run: &Path) -> Result<EvidenceReport> {
    let mut audits = store::load_audits(run)?;
    let verdicts = store::load_verdicts(run)?;
    let mut lines = vec![];
    let mut failed = 0usize;

    // freeze: expanded path -> sha256. BTreeMap so the manifest order is deterministic.
    let mut frozen: BTreeMap<String, String> = BTreeMap::new();

    // (1) presence-pin source files — any path (book, wiki page, research report). Frozen over
    //     the canon view so a falsify block written into a wiki pin source is never seen as drift.
    for p in verify::pins_of(&audits, &verdicts) {
        let path = store::expand_tilde(&p.source_path);
        if path.is_file() {
            let h = store::canon_file_hash(&path)?;
            frozen.insert(h.path, h.sha256);
        }
    }

    // (2) absence: attempt to falsify each silence claim over the author's full book corpus.
    let silence_count = audits.iter().filter(|a| a.silence.is_some()).count();
    for a in &mut audits {
        let claim_id = a.claim_id.clone();
        let Some(sf) = &mut a.silence else { continue };

        if !sf.mechanism_checked {
            bail!(
                "silence flag for '{}' (claim {}) is not mechanism_checked — the dual-gate \
                 requires a wiki-query non-engagement pass before a lexical-absence claim is verifiable",
                sf.author,
                claim_id
            );
        }
        let (files, scope_desc): (Vec<PathBuf>, String) = match sf.scope {
            SilenceScopeKind::AuthorBooks => (
                book_files(&sf.author)?,
                format!(
                    "{}/**/{}/books/txt",
                    store::corpus_root().display(),
                    sf.author
                ),
            ),
            SilenceScopeKind::Wiki => (
                store::wiki_canon_files()?,
                format!("{} (wiki canon)", store::wiki_root().display()),
            ),
        };
        if files.is_empty() {
            bail!(
                "silence claimed for '{}' (claim {}) but the scope is empty: {} — cannot verify \
                 absence over an empty corpus",
                sf.author,
                claim_id,
                scope_desc
            );
        }

        let mut scope = vec![];
        let mut hashes = vec![];
        let mut refutations = vec![];
        for path in &files {
            // Grep + freeze the CANON view (falsify's own blocks stripped) so a verdict written
            // into a canon page can never refute itself, and the freeze is stable across writes.
            let canon = store::canon_bytes(path)?;
            let chars: Vec<char> = normalize_for_match(&canon).chars().collect();
            scope.push(path.display().to_string());
            let h = FileHash {
                path: path.display().to_string(),
                sha256: sha256_hex(canon.as_bytes()),
            };
            frozen.insert(h.path.clone(), h.sha256.clone());
            hashes.push(h);
            for term in &sf.terms_searched {
                let needle: Vec<char> = normalize_for_match(term).chars().collect();
                if needle.is_empty() {
                    continue;
                }
                for at in find_ci(&chars, &needle) {
                    refutations.push(format!(
                        "term '{}' FOUND in {} — \"{}\"",
                        term,
                        path.display(),
                        window(&chars, at, needle.len(), CONTEXT_PAD)
                    ));
                }
            }
        }

        if refutations.is_empty() {
            // validated absence → fill the computed fields (replay = sorted hashes + sorted terms)
            let mut basis: Vec<String> = hashes
                .iter()
                .map(|h| format!("{}:{}", h.path, h.sha256))
                .collect();
            basis.sort();
            let mut terms = sf.terms_searched.clone();
            terms.sort();
            basis.extend(terms);
            sf.replay_hash = sha256_hex(basis.join("\n").as_bytes());
            sf.corpus_scope = scope;
            sf.lexical_empty = true;
            let scope_word = match sf.scope {
                SilenceScopeKind::AuthorBooks => "book",
                SilenceScopeKind::Wiki => "wiki",
            };
            lines.push(format!(
                "OK   silence[{}] claim {} — {} term(s) absent across {} {} file(s) (replay {})",
                sf.author,
                claim_id,
                sf.terms_searched.len(),
                files.len(),
                scope_word,
                &sf.replay_hash[..8]
            ));
        } else {
            failed += 1;
            lines.push(format!(
                "FAIL silence[{}] claim {} — absence REFUTED ({} occurrence(s)):",
                sf.author,
                claim_id,
                refutations.len()
            ));
            for r in refutations.iter().take(5) {
                lines.push(format!("       {r}"));
            }
        }
    }

    let frozen_n = frozen.len();
    lines.push(format!(
        "\n{silence_count} silence flag(s) checked, {failed} failed; {frozen_n} file(s) frozen"
    ));

    // Atomic: only commit the manifest freeze + validated audits when nothing was refuted.
    if failed == 0 {
        let mut m = store::load_manifest(run)?;
        m.corpus_touched = frozen
            .into_iter()
            .map(|(path, sha256)| FileHash { path, sha256 })
            .collect();
        m.corpus_touched.sort_by(|a, b| a.path.cmp(&b.path));
        // Write the validated audits back FIRST, so the frozen artifact hash covers the exact
        // bytes on disk (verify-evidence fills in each silence flag's computed scope/replay fields).
        store::write_json(&store::audits_path(run), &audits)?;
        // A2: freeze the run's own decision artifacts. `persist` refuses to write if audits.json or
        // verdicts.json changed after this point — closing the seam where a post-gate edit could
        // slip an unverified flag or a swapped pin past the checks.
        let mut artifacts = vec![store::file_hash(&store::audits_path(run))?];
        let vp = store::verdicts_path(run);
        if vp.exists() {
            artifacts.push(store::file_hash(&vp)?);
        }
        m.artifacts = artifacts;
        store::save_manifest(run, &m)?;
    }

    Ok(EvidenceReport { lines, failed })
}
