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

/// `--page` → an absolute path. Absolute / `~`-prefixed paths are taken as-is; a bare relative
/// path resolves under the wiki root (so `--page concepts/compiler-optimization.md` works).
fn resolve_page(arg: &str) -> PathBuf {
    let p = store::expand_tilde(arg);
    if p.is_absolute() {
        p
    } else {
        store::wiki_root().join(arg)
    }
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
    let claims = store::load_claims(run)?;
    let audits = store::load_audits(run)?;
    let verdicts = store::load_verdicts(run)?;
    let manifest = store::load_manifest(run)?;

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
    let page = resolve_page(page_arg);

    // 4. read the primary page; recover the block's created date + operator My-read; build the
    //    new block; splice it in (or scaffold a fresh page).
    let existing = fs::read_to_string(&page).unwrap_or_default();
    fence::strip(&existing) // validate fences are balanced before we splice
        .with_context(|| format!("{}: malformed falsify fence", page.display()))?;

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
    let mut seen: BTreeSet<String> = BTreeSet::new();
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
        let new_mark_page = fence::upsert_block(&mtext, &format!("mark={slug}"), &mark);
        edits.push((src, new_mark_page));
    }

    // 6. propose-diff (write <file>.proposed); --apply installs in place.
    for (a, b, s) in &dups {
        eprintln!("near-dup: claims {a} ~ {b} (sim {s:.2}) — operator should merge");
    }
    if apply {
        for (path, content) in &edits {
            if let Some(parent) = path.parent() {
                fs::create_dir_all(parent)?;
            }
            fs::write(path, content).with_context(|| format!("write {}", path.display()))?;
            let _ = fs::remove_file(path.with_extension("md.proposed"));
            println!("installed {}", path.display());
        }
    } else {
        for (path, content) in &edits {
            let proposed = path.with_extension("md.proposed");
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

/// Enforce input-pinning: a verdict may be written only against the exact canon bytes it was
/// judged on. The load-bearing slice = every pin source ∪ every verified silence scope; each
/// must appear in the manifest's frozen set with a still-matching hash OVER THE CANON VIEW.
/// `verify-evidence` writes that frozen set; this refuses to write if it's missing (audit not
/// frozen) or stale (canon drifted).
fn input_pin_recheck(manifest: &RunManifest, audits: &[Audit], verdicts: &[Verdict]) -> Result<()> {
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
) -> String {
    let mut s = String::new();
    s.push_str(&fence::begin_line(&format!("topic={slug}"), created));
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
                    g.trim(),
                    p.quote.trim()
                ),
                None => format!(
                    "- Per {person} ({}){tag}: \"{}\"\n",
                    p.source_ref,
                    p.quote.trim()
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
                a.author, c.note
            ));
            s.push_str(&format!(
                "  - \"{}\" ({})\n",
                c.a.quote.trim(),
                c.a.source_ref
            ));
            s.push_str(&format!(
                "  - \"{}\" ({})\n",
                c.b.quote.trim(),
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
    s.push_str(my_read);
    s.push('\n');
    s.push_str(fence::MY_READ_END);
    s.push('\n');
    s.push_str(&fence::end_line(&format!("topic={slug}")));
    s.push('\n');
    s
}
