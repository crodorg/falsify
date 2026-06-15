//! Run-dir layout, corpus/wiki location, and (de)serialization. The binary owns the
//! run dir under ~/.local/share/falsify/runs/<ts>/. Roots are env-overridable so
//! golden tests can point at fixtures.

use std::fs;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};

use crate::model::*;

/// Expand a leading `~/` to $HOME.
pub fn expand_tilde(p: &str) -> PathBuf {
    if let Some(rest) = p.strip_prefix("~/") {
        if let Some(home) = std::env::var_os("HOME") {
            return PathBuf::from(home).join(rest);
        }
    }
    PathBuf::from(p)
}

fn home() -> PathBuf {
    std::env::var_os("HOME")
        .map(PathBuf::from)
        .unwrap_or_default()
}

/// Root of all run dirs: $XDG_DATA_HOME/falsify/runs or ~/.local/share/falsify/runs.
pub fn runs_root() -> PathBuf {
    std::env::var_os("XDG_DATA_HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|| home().join(".local/share"))
        .join("falsify/runs")
}

/// Canon corpus root (each author at <root>/<author>/books/txt). Precedence:
/// FALSIFY_CORPUS_ROOT (tests point it at a fixture) > $PLAINBRAIN_DATA (the kit
/// convention) > ~/data.
pub fn corpus_root() -> PathBuf {
    std::env::var_os("FALSIFY_CORPUS_ROOT")
        .or_else(|| std::env::var_os("PLAINBRAIN_DATA"))
        .map(PathBuf::from)
        .unwrap_or_else(|| home().join("data"))
}

/// Wiki root. Precedence: FALSIFY_WIKI_ROOT (tests point it at a fixture) > $PLAINBRAIN_WIKI
/// (the kit convention) > ~/wiki.
pub fn wiki_root() -> PathBuf {
    std::env::var_os("FALSIFY_WIKI_ROOT")
        .or_else(|| std::env::var_os("PLAINBRAIN_WIKI"))
        .map(PathBuf::from)
        .unwrap_or_else(|| home().join("wiki"))
}

pub fn read_json<T: serde::de::DeserializeOwned>(path: &Path) -> Result<T> {
    let bytes = fs::read(path).with_context(|| format!("read {}", path.display()))?;
    serde_json::from_slice(&bytes).with_context(|| format!("parse {}", path.display()))
}

pub fn write_json<T: serde::Serialize>(path: &Path, val: &T) -> Result<()> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    let s = serde_json::to_string_pretty(val)?;
    fs::write(path, format!("{s}\n")).with_context(|| format!("write {}", path.display()))?;
    Ok(())
}

pub fn file_hash(path: &Path) -> Result<FileHash> {
    let bytes = fs::read(path).with_context(|| format!("hash {}", path.display()))?;
    Ok(FileHash {
        path: path.display().to_string(),
        sha256: sha256_hex(&bytes),
    })
}

/// A canon file's content with falsify's own fenced blocks stripped (see `fence`). This is the
/// view falsify audits against — absence-grep, freeze hash, drift re-check, and the presence
/// gate all read through it, so falsify never audits its own writing and a block write is never
/// canon drift. Book corpora (no fences) return verbatim. Errors on a malformed fence.
pub fn canon_bytes(path: &Path) -> Result<String> {
    let raw = fs::read_to_string(path).with_context(|| format!("read {}", path.display()))?;
    let stripped = crate::fence::strip(&raw)?;
    // Trailing-normalize: the only non-fence bytes a block write touches are EOF separators
    // (`upsert_block` adds a blank line before an appended block). Collapsing trailing whitespace
    // to a single newline makes the canon view invariant under block writes, so freeze/re-check
    // never see a self-inflicted drift. Grep + presence collapse whitespace anyway; only the
    // frozen hash needs this.
    let trimmed = stripped.trim_end();
    Ok(if trimmed.is_empty() {
        String::new()
    } else {
        format!("{trimmed}\n")
    })
}

/// `file_hash` over the canon view (`canon_bytes`) — the hash that freezes/re-checks a canon
/// file independent of any falsify block written into it.
pub fn canon_file_hash(path: &Path) -> Result<FileHash> {
    let canon = canon_bytes(path)?;
    Ok(FileHash {
        path: path.display().to_string(),
        sha256: sha256_hex(canon.as_bytes()),
    })
}

pub fn manifest_path(run: &Path) -> PathBuf {
    run.join("manifest.json")
}
pub fn claims_path(run: &Path) -> PathBuf {
    run.join("claims.json")
}
pub fn audits_path(run: &Path) -> PathBuf {
    run.join("audits.json")
}
pub fn verdicts_path(run: &Path) -> PathBuf {
    run.join("verdicts.json")
}

/// The compiled wiki canon: every `*.md` under the wiki root EXCEPT falsify's own output
/// (`comparisons/`), the documents under examination (`sources/`), meta (`_lint/`), and any
/// dot-dir. Used to verify a wiki-scoped silence claim — falsify owns this file set so the LLM
/// can't narrow it (enumeration, not discovery).
pub fn wiki_canon_files() -> Result<Vec<PathBuf>> {
    fn walk(dir: &Path, out: &mut Vec<PathBuf>) -> Result<()> {
        for e in fs::read_dir(dir).with_context(|| format!("read_dir {}", dir.display()))? {
            let p = e?.path();
            if p.is_dir() {
                let name = p.file_name().and_then(|n| n.to_str()).unwrap_or("");
                if matches!(name, "comparisons" | "sources" | "_lint") || name.starts_with('.') {
                    continue;
                }
                walk(&p, out)?;
            } else if p.extension().map(|x| x == "md").unwrap_or(false) {
                out.push(p);
            }
        }
        Ok(())
    }
    let root = wiki_root();
    let mut files = vec![];
    if root.is_dir() {
        walk(&root, &mut files)?;
    }
    files.sort();
    Ok(files)
}

/// Max directory depth searched below the corpus root for a `<author>/books/txt` dir. Bounded
/// so a large data root never triggers a runaway walk; nesting an author a domain folder or two
/// deep (e.g. ~/data/<domain>/<author>/books/txt) is well within it.
const MAX_CORPUS_DEPTH: usize = 6;

/// Every `<author>/books/txt` directory discoverable under the corpus root, as (author_key,
/// txt_dir), sorted+deduped. The author key is the directory directly containing `books/`, so
/// corpora may sit at any depth — `~/data/<author>/…` or `~/data/<domain>/<author>/…` — without the
/// operator restructuring the tree or rewriting a single pin. Symlinks are not followed; hidden
/// and build dirs are skipped.
pub fn author_book_dirs() -> Vec<(String, PathBuf)> {
    let mut out = vec![];
    collect_book_dirs(&corpus_root(), 0, &mut out);
    out.sort();
    out.dedup();
    out
}

fn collect_book_dirs(dir: &Path, depth: usize, out: &mut Vec<(String, PathBuf)>) {
    if depth > MAX_CORPUS_DEPTH {
        return;
    }
    if let Ok(rd) = fs::read_dir(dir) {
        for e in rd.flatten() {
            // file_type() reflects the entry itself, so symlinked dirs are not followed.
            if !e.file_type().map(|t| t.is_dir()).unwrap_or(false) {
                continue;
            }
            let name = e.file_name().to_string_lossy().into_owned();
            if name.starts_with('.') || name == "node_modules" || name == "target" {
                continue;
            }
            let p = e.path();
            let txt = p.join("books/txt");
            if txt.is_dir() {
                out.push((name, txt));
            }
            collect_book_dirs(&p, depth + 1, out);
        }
    }
}

/// Author book corpora discoverable on disk: (author_key, book_file_count), sorted, summed across
/// every matching dir for an author. Lets `coverage` name the gap between what the LLM audited and
/// what was available — without searching content (this is enumeration, not discovery).
pub fn enumerable_authors() -> Vec<(String, usize)> {
    let mut counts: std::collections::BTreeMap<String, usize> = std::collections::BTreeMap::new();
    for (author, txt) in author_book_dirs() {
        let n = fs::read_dir(&txt)
            .map(|d| {
                d.flatten()
                    .filter(|f| f.path().extension().map(|x| x == "txt").unwrap_or(false))
                    .count()
            })
            .unwrap_or(0);
        if n > 0 {
            *counts.entry(author).or_default() += n;
        }
    }
    counts.into_iter().collect()
}

pub fn load_manifest(run: &Path) -> Result<RunManifest> {
    read_json(&manifest_path(run))
}
pub fn save_manifest(run: &Path, m: &RunManifest) -> Result<()> {
    write_json(&manifest_path(run), m)
}

/// Load claims and assign content-addressed ids (the LLM never emits them).
pub fn load_claims(run: &Path) -> Result<Vec<Claim>> {
    let mut claims: Vec<Claim> = read_json(&claims_path(run))?;
    for c in &mut claims {
        c.id = claim_id(&c.claim);
    }
    Ok(claims)
}
pub fn load_audits(run: &Path) -> Result<Vec<Audit>> {
    let p = audits_path(run);
    if p.exists() {
        read_json(&p)
    } else {
        Ok(vec![])
    }
}
pub fn load_verdicts(run: &Path) -> Result<Vec<Verdict>> {
    let p = verdicts_path(run);
    if p.exists() {
        read_json(&p)
    } else {
        Ok(vec![])
    }
}
