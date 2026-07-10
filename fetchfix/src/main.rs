// fetchfix — deterministic anchor-corrector + fabrication detector for LLM retrieval output.
//
// A retrieval agent (the pi harness's `fetcher`) returns verbatim `%%% FILE .. LINES a-b %%%`
// blocks, but on dense files its LINES anchors drift (it counts line numbers inside unnumbered
// read windows). The block CONTENT is reliably verbatim, so the true line span can be recovered
// deterministically by locating each block's text back in the source file. This tool does that:
// it rewrites every LINES anchor to the truth, RE-EMITS THE SOURCE FILE'S OWN BYTES for the
// located span (so output is guaranteed byte-verbatim, not the model's copy), and flags any
// block whose quote is NOT in the file (fabrication — that block keeps the model's text, flagged).
//
// Usage:  fetchfix [file]         (reads stdin if no file)
//   stdout: located blocks re-emitted verbatim from source at corrected anchors; unlocatable
//           blocks kept as-is and flagged
//   stderr: a per-block report + summary (OK / CORRECTED a-b->c-d / UNLOCATABLE)
//   exit 0 always unless a fabrication (unlocatable quote) is found -> exit 2
//
// Robust to the model's only cosmetic drift: curly<->straight quotes/apostrophes, unicode
// dashes/ellipsis, and collapsed whitespace (via verify-core's normalizer) — plus line-wrap
// reflow (via verify-core's NormText whole-file locator, as a last-resort tier).
//
// build:  cargo build --release   (workspace member of the falsify repo)

use std::collections::HashMap;
use std::env;
use std::fs;
use std::io::{self, Read, Write};

use verify_core::{normalize_for_match as normalize, NormText};

struct Src {
    raw: Vec<String>,  // original source lines, 0-indexed
    norm: Vec<String>, // normalized source lines, 0-indexed
    text: NormText,    // whole-file normalized index (reflow-robust fallback)
}

fn load<'a>(path: &str, cache: &'a mut HashMap<String, Option<Src>>) -> Option<&'a Src> {
    if !cache.contains_key(path) {
        let v = fs::read_to_string(path).ok().map(|t| {
            let raw: Vec<String> = t.lines().map(str::to_string).collect();
            let norm = raw.iter().map(|l| normalize(l)).collect();
            let text = NormText::new(&t);
            Src { raw, norm, text }
        });
        cache.insert(path.to_string(), v);
    }
    cache.get(path).unwrap().as_ref()
}

// longest verbatim quote payload  *"..."*  in a string
fn longest_quote(s: &str) -> Option<String> {
    let mut best: Option<String> = None;
    let mut start = 0usize;
    while let Some(p) = s[start..].find("*\"") {
        let qs = start + p + 2;
        if let Some(q) = s[qs..].find("\"*") {
            let quote = &s[qs..qs + q];
            if best.as_ref().is_none_or(|b| quote.len() > b.len()) {
                best = Some(quote.to_string());
            }
            start = qs + q + 2;
        } else {
            break;
        }
    }
    best
}

// find the source line index for one body line
fn match_line(bl: &str, src: &Src) -> Option<usize> {
    let n = normalize(bl);
    if n.is_empty() {
        return None;
    }
    // 1) exact normalized line equality (unique even among near-duplicate bullets)
    if let Some(i) = src.norm.iter().position(|l| *l == n) {
        return Some(i);
    }
    // 2) locate by the verbatim quote (survives a minor word/punctuation slip elsewhere in the line)
    if let Some(q) = longest_quote(bl) {
        let qn = normalize(&q);
        if qn.len() >= 20 {
            if let Some(i) = src.norm.iter().position(|l| l.contains(&qn)) {
                return Some(i);
            }
        }
    }
    // 3) body line is a partial line: source line contains it (guard length to avoid collisions)
    if n.len() >= 30 {
        if let Some(i) = src.norm.iter().position(|l| l.contains(&n)) {
            return Some(i);
        }
    }
    None
}

// If `bl` is a truncated line — either pi's grep `[truncated]` marker (copied verbatim) or the
// model's own `(truncated)` self-truncation note — recover the full source line by uniquely
// matching the surviving quote fragment.
// The line-start citation prefix is shared across bullets from the same source, so we key off the
// quote text (after the last `*"`), falling back to the whole survivor. Returns (index, full_raw
// line) ONLY when the fragment matches exactly one source line — otherwise refuse to guess.
fn repair_truncated(bl: &str, src: &Src) -> Option<(usize, String)> {
    let cut = ["[truncated]", "(truncated)"]
        .iter()
        .filter_map(|m| bl.find(m))
        .min()?;
    let before = bl[..cut].trim_end_matches(|c: char| " .*\"\u{2026}".contains(c));
    let frag_src = match before.rfind("*\"") {
        Some(p) => &before[p + 2..],
        None => before,
    };
    let mut frag = normalize(frag_src);
    if frag.len() < 20 {
        frag = normalize(before);
    }
    if frag.len() < 20 {
        return None;
    }
    let mut hit: Option<usize> = None;
    for (i, l) in src.norm.iter().enumerate() {
        if l.contains(&frag) {
            if hit.is_some() {
                return None; // ambiguous — refuse to guess
            }
            hit = Some(i);
        }
    }
    hit.map(|i| (i, src.raw[i].clone()))
}

struct Report {
    total: usize,
    ok: usize,
    corrected: usize,
    repaired: usize,
    unlocatable: usize,
}

fn parse_header(line: &str) -> Option<(String, usize, usize)> {
    // %%% FILE: <path> LINES: <s>-<e> %%%
    let t = line.trim();
    let inner = t.strip_prefix("%%%")?.strip_suffix("%%%")?.trim();
    let inner = inner.strip_prefix("FILE:")?.trim();
    let (path, rest) = inner.split_once(" LINES:")?;
    let (s, e) = rest.trim().split_once('-')?;
    Some((
        path.trim().to_string(),
        s.trim().parse().ok()?,
        e.trim().parse().ok()?,
    ))
}

fn main() {
    let args: Vec<String> = env::args().collect();
    let input = if args.len() > 1 {
        fs::read_to_string(&args[1]).unwrap_or_else(|e| {
            eprintln!("fetchfix: cannot read {}: {}", args[1], e);
            std::process::exit(1);
        })
    } else {
        let mut s = String::new();
        io::stdin().read_to_string(&mut s).ok();
        s
    };

    let mut cache: HashMap<String, Option<Src>> = HashMap::new();
    let mut out = String::with_capacity(input.len());
    let mut rep = Report {
        total: 0,
        ok: 0,
        corrected: 0,
        repaired: 0,
        unlocatable: 0,
    };

    let lines: Vec<&str> = input.split('\n').collect();
    let mut i = 0;
    while i < lines.len() {
        let line = lines[i];
        if let Some((path, s, e)) = parse_header(line) {
            // collect body until %%% END %%%
            let mut body: Vec<&str> = Vec::new();
            let mut j = i + 1;
            while j < lines.len() && lines[j].trim() != "%%% END %%%" {
                body.push(lines[j]);
                j += 1;
            }
            // Split a multi-bullet body into one item per `- ` bullet (with its continuation
            // lines); prose/single-bullet bodies stay as one item. Each item is independently
            // re-anchored, deterministically enforcing one-bullet-per-block + correct LINES.
            let bullet_count = body
                .iter()
                .filter(|b| b.trim_start().starts_with("- "))
                .count();
            let items: Vec<Vec<&str>> = if bullet_count <= 1 {
                vec![body.clone()]
            } else {
                let mut v: Vec<Vec<&str>> = Vec::new();
                for &bl in &body {
                    if bl.trim_start().starts_with("- ") || v.is_empty() {
                        v.push(vec![bl]);
                    } else {
                        v.last_mut().unwrap().push(bl);
                    }
                }
                v
            };
            let split = items.len() > 1;
            let src_opt = load(&path, &mut cache);
            for item in &items {
                rep.total += 1;
                let ne: Vec<&str> = item
                    .iter()
                    .copied()
                    .filter(|b| !b.trim().is_empty())
                    .collect();
                let (mut cs, mut ce, mut located, mut was_repaired) = (s, e, false, false);
                if let (Some(src), Some(&f)) = (src_opt, ne.first()) {
                    if let Some(fi) = match_line(f, src) {
                        let li = ne.last().and_then(|l| match_line(l, src)).unwrap_or(fi);
                        cs = fi + 1;
                        ce = if li >= fi { li + 1 } else { fi + 1 };
                        located = true;
                    } else if ne.len() == 1 {
                        // grep-truncated copy: recover the full source line from the surviving quote
                        if let Some((fi, _full)) = repair_truncated(f, src) {
                            cs = fi + 1;
                            ce = fi + 1;
                            located = true;
                            was_repaired = true;
                        }
                    }
                    // 4) last resort — line-wrap reflow: the item's text exists in the file but
                    // wrapped differently, so no per-line match. The whole-file normalized index
                    // finds it and recovers the true line span. Length-guarded like tier 3;
                    // first occurrence wins (fires only where the block was headed UNLOCATABLE).
                    if !located {
                        let joined = ne.join(" ");
                        if normalize(&joined).len() >= 30 {
                            if let Some(&(a, b)) = src.text.find(&joined).first() {
                                cs = a;
                                ce = b;
                                located = true;
                            }
                        }
                    }
                }
                out.push_str(&format!("%%% FILE: {} LINES: {}-{} %%%\n", path, cs, ce));
                // Located: re-emit the SOURCE FILE'S OWN BYTES for the span (guaranteed
                // byte-verbatim, not the model's copy which may carry cosmetic slips or a
                // grep-truncation). Unlocatable: keep the model's body so the flagged possible
                // fabrication stays visible to the caller.
                if let (true, Some(src)) = (located, src_opt) {
                    for ln in (cs - 1)..=(ce - 1) {
                        out.push_str(&src.raw[ln]);
                        out.push('\n');
                    }
                } else {
                    for b in item {
                        out.push_str(b);
                        out.push('\n');
                    }
                }
                out.push_str("%%% END %%%\n");
                if !located {
                    rep.unlocatable += 1;
                    let why = if src_opt.is_none() {
                        "NOFILE"
                    } else {
                        "UNLOCATABLE"
                    };
                    eprintln!(
                        "{} {} (orig {}-{}) — quote not in file; possible fabrication",
                        why, path, s, e
                    );
                } else if was_repaired {
                    rep.repaired += 1;
                    eprintln!(
                        "REPAIRED {} {}-{} -> {}-{} (completed truncated quote)",
                        path, s, e, cs, ce
                    );
                } else if split {
                    rep.corrected += 1;
                    eprintln!("SPLIT {} {}-{} -> {}-{}", path, s, e, cs, ce);
                } else if (cs, ce) == (s, e) {
                    rep.ok += 1;
                } else {
                    rep.corrected += 1;
                    eprintln!("CORRECTED {} {}-{} -> {}-{}", path, s, e, cs, ce);
                }
            }
            i = if j < lines.len() { j + 1 } else { j };
        } else {
            out.push_str(line);
            if i + 1 < lines.len() {
                out.push('\n');
            }
            i += 1;
        }
    }

    io::stdout().write_all(out.as_bytes()).ok();
    eprintln!(
        "fetchfix: {} blocks | {} ok | {} corrected | {} repaired | {} unlocatable",
        rep.total, rep.ok, rep.corrected, rep.repaired, rep.unlocatable
    );
    if rep.unlocatable > 0 {
        std::process::exit(2);
    }
}
