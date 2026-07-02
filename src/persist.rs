//! The sole writer of falsify-fenced blocks in ~/wiki. Renders the dispute synthesis INLINE
//! into an operator-chosen canon page (`--page`) as a topic-keyed `<!-- falsify:begin … -->`
//! block, drops a one-line backlink "mark" on every other audited wiki page, enforces the
//! verbatim-pin gate, re-checks the frozen input slice (abort on canon drift), protects the
//! operator-only `### My read`, warns on near-duplicate claims, and proposes a diff (never a
//! blind overwrite). Everything OUTSIDE falsify's fences is preserved untouched.
//!
//! Deterministic: dates come from the manifest's as-of (no clock) and the block's `created=`
//! attribute (preserved across runs); ordering is canonical; the block splice is convergent —
//! so re-persisting the same run is a zero-diff no-op.

use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::fs;
use std::path::{Path, PathBuf};

use anyhow::{bail, Context, Result};

use crate::fence;
use crate::model::*;
use crate::store;
use crate::verify;

const MY_READ_DEFAULT: &str = "*Operator only. \"Still learning — no formed view\" is a valid, permanent-until-you-decide state.*";
// Lexical-drift threshold (Jaccard token-set). Tuned to catch word-split/punctuation drift the
// content-addressed id missed, while staying quiet on distinct claims that merely share content
// words. A cheap operator warning, never a gate.
const NEAR_DUP_THRESHOLD: f64 = 0.6;

fn slugify(topic: &str) -> String {
    let mut out = String::new();
    let mut prev_dash = false;
    for c in topic.to_lowercase().chars() {
        if c.is_ascii_alphanumeric() {
            out.push(c);
            prev_dash = false;
        } else if !prev_dash && !out.is_empty() {
            out.push('-');
            prev_dash = true;
        }
    }
    out.trim_matches('-').to_string()
}

fn label_str(l: &Label) -> &'static str {
    match l {
        Label::Match => "MATCH",
        Label::Diverge => "DIVERGE",
        Label::Refuted => "REFUTED",
        Label::Nei => "NEI",
        Label::NotFalsifiable => "NOT-FALSIFIABLE",
    }
}
fn conf_str(c: &Confidence) -> &'static str {
    match c {
        Confidence::High => "High",
        Confidence::Medium => "Medium",
        Confidence::Low => "Low",
    }
}

fn esc(s: &str) -> String {
    s.replace('|', "\\|").replace('\n', " ")
}

/// Neutralize LLM-supplied content that could forge a falsify fence directive. Any line that, after
/// leading whitespace, opens with `<!-- falsify:begin`/`<!-- falsify:end` is escaped (`<!--` →
/// `&lt;!--`) so it can't desync the fence — `strip`/`find_block` match on exactly that prefix, and
/// a quote/note/My-read line carrying one would otherwise truncate or split the block. Everything
/// else (and the structural lines falsify emits itself) is untouched.
fn fence_safe(s: &str) -> String {
    s.split('\n')
        .map(|line| {
            let trimmed = line.trim_start();
            if trimmed.starts_with(fence::BEGIN) || trimmed.starts_with(fence::END) {
                let ws = &line[..line.len() - trimmed.len()];
                format!("{ws}&lt;!--{}", &trimmed["<!--".len()..])
            } else {
                line.to_string()
            }
        })
        .collect::<Vec<_>>()
        .join("\n")
}

/// Write `content` to `path` atomically: write a sibling temp file, then rename over the target so
/// a crash mid-write can never leave a half-written canon page (rename is atomic on the same fs).
fn write_atomic(path: &Path, content: &str) -> Result<()> {
    let tmp = path.with_extension("md.tmp");
    fs::write(&tmp, content).with_context(|| format!("write {}", tmp.display()))?;
    fs::rename(&tmp, path)
        .with_context(|| format!("rename {} -> {}", tmp.display(), path.display()))?;
    Ok(())
}

/// `--page` → an absolute path CONFINED to the wiki. Absolute / `~`-prefixed paths are taken as-is;
/// a bare relative path resolves under the wiki root (so `--page concepts/foo.md` works). The
/// resolved path may not contain a `..` component and must live under the wiki root — v1 writes
/// only inside `~/wiki`, and `--apply` will otherwise install a block into any writable file.
fn resolve_page(arg: &str) -> Result<PathBuf> {
    let expanded = store::expand_tilde(arg);
    let p = if expanded.is_absolute() {
        expanded
    } else {
        store::wiki_root().join(arg)
    };
    if p.components().any(|c| c == std::path::Component::ParentDir) {
        bail!("--page {arg} contains a `..` component — refusing to resolve outside the wiki");
    }
    let root = store::wiki_root();
    if !p.starts_with(&root) {
        bail!(
            "--page {} resolves outside the wiki root {} — v1 writes only inside the wiki",
            p.display(),
            root.display()
        );
    }
    Ok(p)
}

/// Wiki-root-relative, forward-slashed form of an absolute path under the wiki, else `None`.
fn wiki_rel(path: &Path) -> Option<String> {
    path.strip_prefix(store::wiki_root())
        .ok()
        .map(|r| r.to_string_lossy().replace('\\', "/"))
}

/// A relative markdown link from a page at `from` (wiki-rel) to `to` (wiki-rel).
fn rel_link(from: &str, to: &str) -> String {
    let depth = from.matches('/').count();
    format!("{}{}", "../".repeat(depth), to)
}

/// Compact verdict-label summary for a backlink mark, e.g. "3 DIVERGE · 1 NEI".
fn label_summary(verdicts: &[Verdict]) -> String {
    let order = ["DIVERGE", "REFUTED", "MATCH", "NEI", "NOT-FALSIFIABLE"];
    let mut counts: BTreeMap<&str, usize> = BTreeMap::new();
    for v in verdicts {
        *counts.entry(label_str(&v.label)).or_default() += 1;
    }
    let parts: Vec<String> = order
        .iter()
        .filter_map(|l| counts.get(l).map(|n| format!("{n} {l}")))
        .collect();
    if parts.is_empty() {
        "no verdicts".to_string()
    } else {
        parts.join(" · ")
    }
}

pub fn persist(run: &Path, page_arg: &str, topic: &str, apply: bool) -> Result<()> {
    // Resolve + confine the target page BEFORE any work — a `--page` that escapes the wiki should
    // fail fast, never after the gates, and never write a block into an arbitrary file.
    let page = resolve_page(page_arg)?;

    let claims = store::load_claims(run)?;
    let audits = store::load_audits(run)?;
    let verdicts = store::load_verdicts(run)?;
    let manifest = store::load_manifest(run)?;

    // 0. deterministic schema integrity (cheap, no IO): every audit/verdict must reference a real
    //    claim, and the falsifiability gate (claim.falsifiability × verdict.label) must hold. Abort
    //    on drift rather than render "(unknown claim)" or a mis-routed label into canon.
    check_claim_refs(&claims, &audits, &verdicts)?;
    check_falsifiability_gate(&claims, &verdicts)?;

    // 0b. artifact freeze re-check: audits.json + verdicts.json must hash to exactly what
    //     `verify-evidence` froze — so an edit after the gates ran can't smuggle an unverified
    //     silence flag or a swapped pin past them.
    artifact_recheck(&manifest)?;

    // 1. input-pin re-check: every load-bearing file must be FROZEN in the manifest and UNCHANGED
    //    (over the canon view) since the audit. Missing ⇒ verify-evidence wasn't run; mismatch ⇒
    //    the canon drifted and the verdict no longer rests on what it was judged against.
    input_pin_recheck(&manifest, &audits, &verdicts)?;

    // 2. pin-gate (PRESENCE) over audit artifacts + verdict pins — abort on any failure.
    let report = verify::verify_pins(run)?;
    if report.failed > 0 {
        eprint!("{}", report.render());
        bail!(
            "pin-gate: {} pin(s) failed verbatim-existence — refusing to write",
            report.failed
        );
    }

    // 3. near-dup detector within the run's claims (extraction reword check).
    let mut dups = vec![];
    for i in 0..claims.len() {
        for j in (i + 1)..claims.len() {
            if claims[i].id != claims[j].id {
                let s = claim_similarity(&claims[i].claim, &claims[j].claim);
                if s >= NEAR_DUP_THRESHOLD {
                    dups.push((claims[i].id.clone(), claims[j].id.clone(), s));
                }
            }
        }
    }

    let slug = slugify(topic);
    let key = format!("topic={slug}");

    // 4. read the primary page; recover the block's created date + operator My-read; build the
    //    new block; splice it in (or scaffold a fresh page).
    let existing = fs::read_to_string(&page).unwrap_or_default();
    fence::strip(&existing) // validate fences are balanced before we splice
        .with_context(|| format!("{}: malformed falsify fence", page.display()))?;

    // The claim ids this run's block records (everything it audits or verdicts on), sorted.
    let new_ids = block_claim_ids(&audits, &verdicts);

    let (created, my_read) = match fence::find_block(&existing, &key) {
        Some((s, e)) => {
            let region = &existing[s..e];
            if region.contains(fence::MY_READ_START) && !region.contains(fence::MY_READ_END) {
                bail!(
                    "{}: `### My read` start without its end inside the falsify block — refusing \
                     to clobber operator content",
                    page.display()
                );
            }
            let begin = region.lines().next().unwrap_or("");
            // A1 snapshot guard: this block replaces the existing one wholesale (latest run wins).
            // If the existing block records claims this run does NOT, warn loudly — those verdicts
            // are about to be dropped. Non-silent, so the operator sees it before --apply.
            let new_set: BTreeSet<&str> = new_ids.iter().map(|s| s.as_str()).collect();
            let dropped: Vec<String> = fence::existing_claims(begin)
                .into_iter()
                .filter(|id| !new_set.contains(id.as_str()))
                .collect();
            if !dropped.is_empty() {
                eprintln!(
                    "WARNING: re-persisting topic '{}' replaces the existing block (snapshot — \
                     latest run wins) and DROPS {} claim(s) it recorded but this run does not: {}. \
                     Re-run with those claims to keep their verdicts.",
                    topic,
                    dropped.len(),
                    dropped.join(", ")
                );
            }
            (
                fence::existing_created(begin).unwrap_or_else(|| manifest.as_of.clone()),
                fence::existing_my_read(region).unwrap_or_else(|| MY_READ_DEFAULT.to_string()),
            )
        }
        None => (manifest.as_of.clone(), MY_READ_DEFAULT.to_string()),
    };

    let block = render_block(
        topic,
        &slug,
        &created,
        &manifest.as_of,
        &claims,
        &audits,
        &verdicts,
        &my_read,
        &new_ids,
    );
    let new_page = if existing.is_empty() {
        fence::upsert_block(&scaffold(topic, &created, &manifest.as_of), &key, &block)
    } else {
        fence::upsert_block(&existing, &key, &block)
    };

    // 5. backlink marks — one per distinct OTHER wiki page that contributed a pin.
    let mut edits: Vec<(PathBuf, String)> = vec![(page.clone(), new_page)];
    let page_rel = wiki_rel(&page).unwrap_or_else(|| format!("{slug}.md"));
    let summary = label_summary(&verdicts);
    let mark_key = format!("mark={slug}");
    let mut seen: BTreeSet<String> = BTreeSet::new();
    let mut marked: BTreeSet<PathBuf> = BTreeSet::new(); // pages that carry a mark THIS run
    for p in verify::pins_of(&audits, &verdicts) {
        let src = store::expand_tilde(&p.source_path);
        if src == page {
            continue;
        }
        let Some(rel) = wiki_rel(&src) else { continue }; // only wiki pages get marks
        if rel == page_rel || !seen.insert(rel.clone()) {
            continue;
        }
        let mark = render_mark(
            &slug,
            topic,
            &rel_link(&rel, &page_rel),
            &manifest.as_of,
            &summary,
        );
        let mtext = fs::read_to_string(&src).unwrap_or_default();
        fence::strip(&mtext)
            .with_context(|| format!("{}: malformed falsify fence", src.display()))?;
        let new_mark_page = fence::upsert_block(&mtext, &mark_key, &mark);
        marked.insert(src.clone());
        edits.push((src, new_mark_page));
    }

    // 5b. mark GC (A7): a page that carried a mark for THIS topic in a prior run but contributes no
    //     pin now keeps a stale backlink with a frozen label summary. Propose removing it.
    for f in store::wiki_canon_files()? {
        if f == page || marked.contains(&f) {
            continue;
        }
        let Ok(txt) = fs::read_to_string(&f) else {
            continue;
        };
        if fence::find_block(&txt, &mark_key).is_none() {
            continue;
        }
        fence::strip(&txt).with_context(|| format!("{}: malformed falsify fence", f.display()))?;
        if let Some(pruned) = fence::remove_block(&txt, &mark_key) {
            edits.push((f, pruned));
        }
    }

    // 6. propose-diff / apply. Propose writes <page>.proposed (never touches canon). --apply
    //    installs ONLY the reviewed proposal: for every target it requires a <page>.proposed that
    //    is byte-identical to the freshly-regenerated content, else it aborts. So the reviewed
    //    bytes are the installed bytes, a one-shot --apply that skipped propose+review is refused,
    //    and a proposal gone stale (run artifacts or the host page changed since) fails loudly.
    for (a, b, s) in &dups {
        eprintln!("near-dup: claims {a} ~ {b} (sim {s:.2}) — operator should merge");
    }
    if apply {
        // First pass: every target must carry a matching reviewed proposal (validate before any
        // write, so a stale target can't leave the wiki half-installed).
        for (path, content) in &edits {
            let proposed = proposed_path(path);
            let reviewed = match fs::read_to_string(&proposed) {
                Ok(s) => s,
                Err(_) => bail!(
                    "no proposal for {} — run persist without --apply first, review the diff, then --apply",
                    path.display()
                ),
            };
            if &reviewed != content {
                bail!(
                    "proposal for {} is stale (regenerated content differs from {}) — the run \
                     artifacts or the host page changed since; re-propose (run without --apply), \
                     review, then --apply",
                    path.display(),
                    proposed.display()
                );
            }
        }
        // Second pass: install (atomic write-then-rename per file).
        for (path, content) in &edits {
            if let Some(parent) = path.parent() {
                fs::create_dir_all(parent)?;
            }
            write_atomic(path, content)?;
            let _ = fs::remove_file(proposed_path(path));
            println!("installed {}", path.display());
        }
    } else {
        for (path, content) in &edits {
            let proposed = proposed_path(path);
            if let Some(parent) = proposed.parent() {
                fs::create_dir_all(parent)?;
            }
            fs::write(&proposed, content)
                .with_context(|| format!("write {}", proposed.display()))?;
            println!("proposed {}", proposed.display());
            println!(
                "  review:  diff -u '{}' '{}'",
                path.display(),
                proposed.display()
            );
        }
        println!(
            "install: falsify persist --run-dir '{}' --page '{}' --topic '{}' --apply",
            run.display(),
            page.display(),
            topic
        );
    }
    Ok(())
}

/// The `<page>.proposed` path for a target page (e.g. `foo.md` → `foo.md.proposed`).
fn proposed_path(path: &Path) -> PathBuf {
    path.with_extension("md.proposed")
}

/// A2: the run's own decision artifacts (audits.json, verdicts.json) must hash to exactly what
/// `verify-evidence` froze into the manifest. An empty frozen set means verify-evidence never ran;
/// a mismatch means audits/verdicts were edited after the gates — either way the verdict about to
/// be written is not the one that was validated, so refuse.
fn artifact_recheck(manifest: &RunManifest) -> Result<()> {
    if manifest.artifacts.is_empty() {
        bail!(
            "artifacts not frozen — run `falsify verify-evidence` before persist (it validates \
             silence AND freezes audits/verdicts so a later edit can't slip past the gates)"
        );
    }
    for fh in &manifest.artifacts {
        let now = store::file_hash(Path::new(&fh.path))
            .with_context(|| format!("re-hash artifact {}", fh.path))?
            .sha256;
        if now != fh.sha256 {
            bail!(
                "artifact {} changed since verify-evidence (frozen {}…, now {}…) — audits/verdicts \
                 were edited after the gates ran; re-run `falsify verify-evidence`",
                fh.path,
                &fh.sha256[..8.min(fh.sha256.len())],
                &now[..8.min(now.len())]
            );
        }
    }
    Ok(())
}

/// Enforce input-pinning: a verdict may be written only against the exact canon bytes it was
/// judged on. The load-bearing slice = every pin source ∪ every verified silence scope; each
/// must appear in the manifest's frozen set with a still-matching hash OVER THE CANON VIEW.
/// `verify-evidence` writes that frozen set; this refuses to write if it's missing (audit not
/// frozen) or stale (canon drifted).
fn input_pin_recheck(manifest: &RunManifest, audits: &[Audit], verdicts: &[Verdict]) -> Result<()> {
    // The source document under examination is part of the pinned slice (new-run hashed its raw
    // bytes). If it still exists and drifted since the run, the claims no longer match what was
    // analyzed — abort. If it's gone (a transient input), there is nothing to re-check.
    let src = store::expand_tilde(&manifest.source.path);
    if src.is_file() {
        let now = store::file_hash(&src)
            .with_context(|| format!("re-hash source {}", src.display()))?
            .sha256;
        if now != manifest.source.sha256 {
            bail!(
                "input-pin: source {} changed since new-run (pinned {}…, now {}…) — the claims were \
                 extracted from different bytes; re-run from new-run",
                src.display(),
                &manifest.source.sha256[..8.min(manifest.source.sha256.len())],
                &now[..8.min(now.len())]
            );
        }
    }

    let frozen: HashMap<&str, &str> = manifest
        .corpus_touched
        .iter()
        .map(|h| (h.path.as_str(), h.sha256.as_str()))
        .collect();

    let mut slice: BTreeSet<String> = BTreeSet::new();
    for p in verify::pins_of(audits, verdicts) {
        slice.insert(store::expand_tilde(&p.source_path).display().to_string());
    }
    for a in audits {
        if let Some(sf) = &a.silence {
            for f in &sf.corpus_scope {
                slice.insert(store::expand_tilde(f).display().to_string());
            }
        }
    }

    for f in &slice {
        match frozen.get(f.as_str()) {
            None => bail!(
                "input-pin: {f} is load-bearing but not frozen in the manifest — run \
                 `falsify verify-evidence` before persist"
            ),
            Some(&want) => {
                let now = store::canon_file_hash(Path::new(f))
                    .with_context(|| format!("re-hash {f}"))?
                    .sha256;
                if now != want {
                    bail!(
                        "input-pin: {f} drifted since the run (frozen {}…, now {}…) — the verdict \
                         rests on changed canon; re-run the audit",
                        &want[..8.min(want.len())],
                        &now[..8.min(now.len())]
                    );
                }
            }
        }
    }
    Ok(())
}

/// Frontmatter + H1 for a page falsify creates from scratch (when `--page` does not yet exist).
/// Existing pages are never reframed — only their fenced block is touched.
fn scaffold(topic: &str, created: &str, updated: &str) -> String {
    format!(
        "---\ntitle: {topic}\ntype: comparison\ncreated: {created}\nupdated: {updated}\ntags: [falsify, comparison]\n---\n\n# {topic}\n\n"
    )
}

/// One-line backlink mark for a contributing wiki page.
fn render_mark(slug: &str, topic: &str, link: &str, updated: &str, summary: &str) -> String {
    format!(
        "{}\n> **Falsified** re: [{topic}]({link}) — {summary}. *(falsify {updated})*\n{}\n",
        fence::begin_line(&format!("mark={slug}"), updated),
        fence::end_line(&format!("mark={slug}"))
    )
}

/// The inline synthesis block: a `## Falsified: <topic>` section with `###` subsections, wrapped
/// in the topic fence. No frontmatter, no H1 — the host page owns those.
/// The claim ids a block records — everything it audits or verdicts on, sorted + deduped. Emitted
/// as the block's `claims=` fence attribute and used by the snapshot guard to detect dropped claims.
fn block_claim_ids(audits: &[Audit], verdicts: &[Verdict]) -> Vec<String> {
    let mut ids: BTreeSet<String> = BTreeSet::new();
    for a in audits {
        ids.insert(a.claim_id.clone());
    }
    for v in verdicts {
        ids.insert(v.claim_id.clone());
    }
    ids.into_iter().collect()
}

#[allow(clippy::too_many_arguments)]
fn render_block(
    topic: &str,
    slug: &str,
    created: &str,
    updated: &str,
    claims: &[Claim],
    audits: &[Audit],
    verdicts: &[Verdict],
    my_read: &str,
    claim_ids: &[String],
) -> String {
    let mut s = String::new();
    let attrs = if claim_ids.is_empty() {
        String::new()
    } else {
        format!("claims={}", claim_ids.join(","))
    };
    s.push_str(&fence::begin_line_with(
        &format!("topic={slug}"),
        created,
        &attrs,
    ));
    s.push('\n');
    s.push_str(&format!("## Falsified: {topic}\n\n"));
    s.push_str(&format!(
        "*Generated by `falsify` (created {created} · updated {updated}). This section is regenerated each run; only `### My read` is preserved. Positions are an unweighted attributed map; verdicts are pinned.*\n\n"
    ));

    // ### Positions — map fragments grouped by person (canonical order)
    s.push_str("### Positions\n\n");
    s.push_str("> Unweighted collection of claims from the sources below. No synthesis or weighting yet.\n\n");
    let mut by_person: BTreeMap<String, Vec<&Pin>> = BTreeMap::new();
    for a in audits {
        for p in &a.map_fragments {
            by_person.entry(p.person.clone()).or_default().push(p);
        }
    }
    for (person, pins) in &by_person {
        for p in pins {
            let tag = if p.kind == PinKind::Transcript {
                " *(transcript)*"
            } else {
                ""
            };
            let line = match &p.gloss {
                Some(g) => format!(
                    "- Per {person} ({}){tag}: {} \u{2014} \"{}\"\n",
                    p.source_ref,
                    fence_safe(g.trim()),
                    fence_safe(p.quote.trim())
                ),
                None => format!(
                    "- Per {person} ({}){tag}: \"{}\"\n",
                    p.source_ref,
                    fence_safe(p.quote.trim())
                ),
            };
            s.push_str(&line);
        }
    }
    s.push('\n');

    // ### Nature of the disagreement — contradiction notes
    s.push_str("### Nature of the disagreement\n\n");
    let mut any = false;
    for a in audits {
        for c in &a.contradictions {
            any = true;
            let mech = c
                .mechanical
                .as_ref()
                .map(|m| format!(" [{m}]"))
                .unwrap_or_default();
            s.push_str(&format!(
                "- **{}** self-contradiction{mech}: {}\n",
                a.author,
                fence_safe(&c.note)
            ));
            s.push_str(&format!(
                "  - \"{}\" ({})\n",
                fence_safe(c.a.quote.trim()),
                c.a.source_ref
            ));
            s.push_str(&format!(
                "  - \"{}\" ({})\n",
                fence_safe(c.b.quote.trim()),
                c.b.source_ref
            ));
        }
    }
    if !any {
        s.push_str("*(no self-contradictions surfaced this run)*\n");
    }
    s.push('\n');

    // ### Status — per-claim verdict table
    s.push_str("### Status\n\n");
    s.push_str("| claim | verdict | confidence | load-bearing pin |\n");
    s.push_str("|---|---|---|---|\n");
    let claim_text: HashMap<&str, &str> = claims
        .iter()
        .map(|c| (c.id.as_str(), c.claim.as_str()))
        .collect();
    for v in verdicts {
        let ct = claim_text
            .get(v.claim_id.as_str())
            .copied()
            .unwrap_or("(unknown claim)");
        let pin = v
            .load_bearing_pin
            .as_ref()
            .map(|p| format!("\"{}\"", p.quote.trim()))
            .unwrap_or_default();
        let temporal = if v.temporal_flag.is_some() {
            " ⏱"
        } else {
            ""
        };
        s.push_str(&format!(
            "| {} | {}{} | {} | {} |\n",
            esc(ct),
            label_str(&v.label),
            temporal,
            conf_str(&v.confidence),
            esc(&pin)
        ));
    }
    s.push('\n');

    // ### UNADDRESSED — silence flags (operator-attention, never verdicts)
    let silences: Vec<&Audit> = audits.iter().filter(|a| a.silence.is_some()).collect();
    if !silences.is_empty() {
        s.push_str("### UNADDRESSED (silence flags — operator-attention, not verdicts)\n\n");
        for a in &silences {
            if let Some(sf) = &a.silence {
                let rh: String = sf.replay_hash.chars().take(8).collect();
                let scope_kind = match sf.scope {
                    SilenceScopeKind::AuthorBooks => "books",
                    SilenceScopeKind::Wiki => "wiki",
                };
                s.push_str(&format!(
                    "- **{}** appears not to engage claim `{}` — searched: {} (scope: {} · {} files, replay {})\n",
                    sf.author,
                    a.claim_id,
                    sf.terms_searched.join(", "),
                    scope_kind,
                    sf.corpus_scope.len(),
                    rh
                ));
            }
        }
        s.push('\n');
    }

    // ### My read — protected operator-only block
    s.push_str("### My read\n\n");
    s.push_str(fence::MY_READ_START);
    s.push('\n');
    s.push_str(&fence_safe(my_read));
    s.push('\n');
    s.push_str(fence::MY_READ_END);
    s.push('\n');
    s.push_str(&fence::end_line(&format!("topic={slug}")));
    s.push('\n');
    s
}
