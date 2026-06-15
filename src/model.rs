//! The data contract: the JSON schemas every subcommand shares, the content-addressed
//! claim id, and the determinism-critical normalizers. This is the seam — the LLM
//! proposes these structures, Rust validates and pins them.

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

/// Bump on any breaking change to the on-disk JSON. Recorded in the run manifest.
/// v2: discovery left the substrate — `SearchEnvelope`/`SearchHit` and `Audit.envelope_ref`
/// removed; `SilenceFlag`'s scope/empty/replay fields are now computed by `verify-evidence`.
pub const SCHEMA_VERSION: u32 = 2;

// ---- pins ----------------------------------------------------------------

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "snake_case")]
pub enum PinKind {
    /// A book / clean text source — full-grade verbatim pin.
    Book,
    /// A transcript (VTT) source — searchable, but the pin is lower quality
    /// (auto-caption artifacts, no stable anchor). Reserved for the transcript
    /// fast-follow; flagged so it never masquerades as book-grade evidence.
    Transcript,
}

/// The load-bearing unit of evidence. The verbatim `quote` is the real pin
/// (per the wiki: "cite a quote, never a line number"); `source_path` is where
/// `verify-pins` deterministically confirms the quote exists.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct Pin {
    /// Attributed author (links to entities/<person>.md).
    pub person: String,
    /// Human-readable ref, e.g. "NHL ch.6".
    pub source_ref: String,
    /// File the quote lives in (e.g. <corpus>/<author>/books/txt/<book>.txt, or a wiki page).
    pub source_path: String,
    /// The verbatim quote — THE pin. Must exist in `source_path` or the write aborts.
    pub quote: String,
    pub kind: PinKind,
    /// Optional auditor gloss of what the quote says (the claim side of the wiki's
    /// `Per X (ref): <gloss> — "quote"` map line). Not pinned; the quote is.
    #[serde(default)]
    pub gloss: Option<String>,
}

// ---- claims --------------------------------------------------------------

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "snake_case")]
pub enum Falsifiability {
    /// Could in principle be shown false by evidence.
    Falsifiable,
    /// Rhetoric, value judgment, or unfalsifiable assertion — routed OUT of the
    /// MATCH/REFUTED rubric by the falsifiability gate.
    NotFalsifiable,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Claim {
    /// Content-addressed id = sha256(normalize_claim(claim))[..12]. Assigned by
    /// `validate`/loading, never emitted by the LLM. Stable across runs so
    /// `persist` merges in place instead of forking.
    #[serde(default)]
    pub id: String,
    /// The source's assertion, as extracted.
    pub claim: String,
    pub falsifiability: Falsifiability,
    /// ISO date the claim was made, when known — feeds the temporal guard.
    #[serde(default)]
    pub claim_date: Option<String>,
    /// Extraction's hint at where to pin; not load-bearing.
    #[serde(default)]
    pub suggested_pin: Option<String>,
}

// ---- audit ---------------------------------------------------------------

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ContradictionPair {
    pub a: Pin,
    pub b: Pin,
    pub note: String,
    /// v2 mechanical pre-filter verdict, if one fired (polarity/numeric conflict).
    #[serde(default)]
    pub mechanical: Option<String>,
}

/// Where a silence claim's absence is verified. falsify owns the file set for each scope —
/// the LLM never picks a narrow slice (that's the anti-gaming property). `author_books` scans
/// `<corpus_root>/**/<author>/books/txt` (recursive — corpora may nest under domain folders);
/// `wiki` scans the compiled canon under `<wiki_root>`
/// (minus comparisons/, sources/, _lint/). Defaults to `author_books` for back-compat.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Default)]
#[serde(rename_all = "snake_case")]
pub enum SilenceScopeKind {
    #[default]
    AuthorBooks,
    Wiki,
}

/// An UNADDRESSED flag: the canon appears not to engage the claim. Conservative — fires only
/// when BOTH a lexical-absence check AND a mechanism-level wiki-query pass show non-engagement.
/// An operator-attention flag, never a verdict.
///
/// The LLM supplies `author`, `terms_searched`, `scope`, and `mechanism_checked` (its own
/// wiki-query judgment). `verify-evidence` then *attempts to falsify* the absence by re-grepping
/// every term across the scope falsify enumerates; if nothing is found it fills the COMPUTED
/// fields below and the flag stands, otherwise the run fails (the absence claim was refuted). So
/// `corpus_scope`/`lexical_empty`/`replay_hash` are machine-written, never the model's word.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SilenceFlag {
    /// The subject of the silence — a book-corpus key when scope=author_books, else a display
    /// label (e.g. "wiki canon").
    pub author: String,
    pub terms_searched: Vec<String>,
    /// Which corpus absence is verified over. Defaults to author_books.
    #[serde(default)]
    pub scope: SilenceScopeKind,
    /// COMPUTED by verify-evidence: the files actually scanned.
    #[serde(default)]
    pub corpus_scope: Vec<String>,
    /// COMPUTED by verify-evidence: true once every term verified absent across the scope.
    #[serde(default)]
    pub lexical_empty: bool,
    /// LLM-supplied: a mechanism-level wiki-query pass also showed non-engagement.
    pub mechanism_checked: bool,
    /// COMPUTED by verify-evidence: sha256 over (sorted scope file hashes + sorted terms).
    #[serde(default)]
    pub replay_hash: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Audit {
    pub claim_id: String,
    pub author: String,
    /// Attributed verbatim map fragments ("Per X (ref): claim — quote").
    pub map_fragments: Vec<Pin>,
    #[serde(default)]
    pub contradictions: Vec<ContradictionPair>,
    #[serde(default)]
    pub silence: Option<SilenceFlag>,
}

// ---- verdict -------------------------------------------------------------

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "snake_case")]
pub enum Label {
    Match,
    Diverge,
    Refuted,
    /// Not enough info — first-class; never force a call.
    Nei,
    /// Routed out of the rubric by the falsifiability gate.
    NotFalsifiable,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "snake_case")]
pub enum Confidence {
    High,
    Medium,
    Low,
}

/// One judge's vote — v2 multi-vote. Empty in v1 (schema-forward, no break).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Vote {
    pub voter: String,
    pub label: Label,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Verdict {
    pub claim_id: String,
    pub label: Label,
    pub confidence: Confidence,
    #[serde(default)]
    pub load_bearing_pin: Option<Pin>,
    /// Set when evidence postdates the claim (temporal guard) — a flag, not a suppression.
    #[serde(default)]
    pub temporal_flag: Option<String>,
    /// v2 multi-vote record; empty in v1.
    #[serde(default)]
    pub votes: Vec<Vote>,
    pub rationale: String,
}

// ---- run manifest --------------------------------------------------------

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FileHash {
    pub path: String,
    pub sha256: String,
}

/// Input-pinning: a run is only meaningful relative to the exact bytes it ran
/// against. Without this the pin seam and idempotent merge are non-reproducible.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RunManifest {
    pub run_id: String,
    pub created: String,
    pub schema_version: u32,
    pub as_of: String,
    /// Content-hash of the source document under examination.
    pub source: FileHash,
    /// The frozen input slice: content-hash of every load-bearing file (pin sources +
    /// silence scopes). Written by `verify-evidence`, re-checked by `persist` (abort on
    /// drift) — this is what makes a verdict replayable against a pinned corpus.
    #[serde(default)]
    pub corpus_touched: Vec<FileHash>,
    #[serde(default)]
    pub model_ids: Vec<String>,
    #[serde(default)]
    pub prompt_hashes: Vec<String>,
}

// ---- hashing + normalizers (the determinism linchpin) --------------------

/// sha256 hex of bytes.
pub fn sha256_hex(bytes: &[u8]) -> String {
    let mut h = Sha256::new();
    h.update(bytes);
    format!("{:x}", h.finalize())
}

/// Replace common unicode punctuation variants with ASCII so a quote copied from a
/// re-exported source still matches: curly quotes → straight, en/em/figure dash →
/// hyphen, ellipsis → "...", nbsp → space.
fn straighten(text: &str) -> String {
    let mapped: String = text
        .chars()
        .map(|c| match c {
            '\u{2018}' | '\u{2019}' | '\u{02BC}' => '\'',
            '\u{201C}' | '\u{201D}' => '"',
            '\u{2013}' | '\u{2014}' | '\u{2012}' => '-',
            '\u{00A0}' => ' ',
            other => other,
        })
        .collect();
    mapped.replace('\u{2026}', "...")
}

/// The determinism linchpin: canonicalize claim text so the same assertion always
/// hashes to the same id. Lowercase, straighten punctuation, drop everything but
/// `[a-z0-9 ]`, collapse whitespace. Robust to casing/punctuation/whitespace drift;
/// rewording still changes the id — that residual is caught by the near-dup detector.
pub fn normalize_claim(text: &str) -> String {
    let lowered = straighten(text).to_lowercase();
    let mut out = String::with_capacity(lowered.len());
    let mut prev_space = false;
    for c in lowered.chars() {
        if c.is_ascii_alphanumeric() {
            out.push(c);
            prev_space = false;
        } else if !prev_space && !out.is_empty() {
            out.push(' ');
            prev_space = true;
        }
    }
    out.trim().to_string()
}

/// Content-addressed claim id: first 12 hex chars of sha256(normalize_claim(text)).
pub fn claim_id(text: &str) -> String {
    sha256_hex(normalize_claim(text).as_bytes())[..12].to_string()
}

/// Normalize for verbatim-pin existence matching: straighten unicode punctuation and
/// collapse all whitespace to single spaces, but PRESERVE case and words — a pin must
/// match faithfully, tolerant only of line-wrapping and quote-char drift.
pub fn normalize_for_match(text: &str) -> String {
    straighten(text)
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
}

/// Token-set (Jaccard) similarity over normalized claim tokens, 0.0–1.0. The
/// near-dup detector: rewording flips the content-addressed id, so before `persist`
/// forks a "new" claim it checks whether an existing claim is a near-duplicate and
/// surfaces it for operator merge. **Lexical only** — it catches punctuation /
/// word-split / word-order drift (where the id should have matched but didn't);
/// semantic paraphrase (different words, same meaning) is the auditor LLM's job,
/// not this. So this favors recall as a cheap stderr warning, never a gate.
pub fn claim_similarity(a: &str, b: &str) -> f64 {
    use std::collections::BTreeSet;
    let sa = normalize_claim(a);
    let sb = normalize_claim(b);
    let ta: BTreeSet<&str> = sa.split(' ').filter(|s| !s.is_empty()).collect();
    let tb: BTreeSet<&str> = sb.split(' ').filter(|s| !s.is_empty()).collect();
    if ta.is_empty() && tb.is_empty() {
        return 1.0;
    }
    let inter = ta.intersection(&tb).count() as f64;
    let union = ta.union(&tb).count() as f64;
    if union == 0.0 {
        0.0
    } else {
        inter / union
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn id_stable_across_punctuation_and_case() {
        assert_eq!(
            claim_id("Enabling -O3 speeds up code."),
            claim_id("enabling  -o3 SPEEDS-UP code")
        );
    }

    #[test]
    fn id_changes_on_reword() {
        assert_ne!(
            claim_id("-O3 boosts throughput"),
            claim_id("inlining boosts throughput")
        );
    }

    #[test]
    fn match_norm_tolerates_linewrap_and_curly_quotes() {
        let src = normalize_for_match("the hot loop\nis  \u{201C}vectorized\u{201D} by  -O3");
        let quote = normalize_for_match("is \"vectorized\" by -O3");
        assert!(src.contains(&quote));
    }

    #[test]
    fn similarity_flags_lexical_drift_only() {
        // punctuation / word-split drift the id missed → flagged (high overlap)
        let s = claim_similarity(
            "enabling -O3 speeds up code",
            "Enabling -O3 speeds-up code!",
        );
        assert!(s > 0.8, "expected lexical near-dup, got {s}");
        // distinct claims → low overlap
        let d = claim_similarity(
            "profile-guided builds autotune",
            "garbage collection pauses threads",
        );
        assert!(d < 0.2, "expected distinct, got {d}");
        // semantic paraphrase is NOT this function's job — it scores low, by design
        let p = claim_similarity(
            "-O3 causes speedups",
            "aggressive inlining is responsible for faster code",
        );
        assert!(p < 0.5, "semantic paraphrase is the LLM's job, got {p}");
    }
}
