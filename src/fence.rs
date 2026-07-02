//! falsify-owned fenced blocks inside wiki canon pages.
//!
//! The output model writes the synthesis INLINE into a real canon page (operator-chosen via
//! `--page`) rather than a standalone `comparisons/` file, plus a one-line backlink "mark" on
//! every other audited wiki page. Each region is delimited by HTML comments so it round-trips
//! through markdown untouched:
//!
//! ```text
//! <!-- falsify:begin topic=<slug> created=<date> -->
//! ## Falsified: <topic>
//! ... ### Positions / ### Status / ... / ### My read ...
//! <!-- falsify:end topic=<slug> -->
//! ```
//!
//! Marks reuse the same begin/end grammar with key `mark=<slug>`.
//!
//! THE SELF-REFUTATION INVARIANT: falsify must never audit its own writing. [`strip`] removes
//! every fenced region so the absence-grep, the freeze hash, the drift re-check, and the
//! presence gate all see ONLY the real canon (the non-fence content). Writing or updating a
//! block therefore never looks like canon drift, and a verdict can neither refute nor validate
//! itself. `strip` errors on an unbalanced fence (a hand-mangled page) rather than silently
//! eating canon to EOF.

use anyhow::{bail, Result};

/// Prefix of a block-open comment. Full line: `<!-- falsify:begin <key> [created=<date>] -->`.
pub const BEGIN: &str = "<!-- falsify:begin";
/// Prefix of a block-close comment. Full line: `<!-- falsify:end <key> -->`.
pub const END: &str = "<!-- falsify:end";
/// Operator-only My-read sub-markers (nested inside a topic block).
pub const MY_READ_START: &str = "<!-- falsify:my-read:start -->";
pub const MY_READ_END: &str = "<!-- falsify:my-read:end -->";

/// Remove every falsify-fenced region (begin..end inclusive) from `text`, returning only the
/// real canon. Errors on an unbalanced fence so a malformed page fails loudly instead of
/// silently dropping canon. A page with no fences returns unchanged — book corpora (`*.txt`)
/// hit this path as a pure no-op.
pub fn strip(text: &str) -> Result<String> {
    let mut out = String::with_capacity(text.len());
    let mut in_block = false;
    for line in text.split_inclusive('\n') {
        let t = line.trim_start();
        if !in_block {
            if t.starts_with(BEGIN) {
                in_block = true;
            } else if t.starts_with(END) {
                // An `end` with no open `begin` is as malformed as an unclosed `begin`: keeping it
                // would leak a fence directive (and the block tail after a desync) into the canon
                // view. Fail closed rather than silently pass it through.
                bail!(
                    "unbalanced falsify fence: a `{END} ...` line has no matching `{BEGIN} ...` — \
                     refusing to parse a malformed page"
                );
            } else {
                out.push_str(line);
            }
        } else if t.starts_with(END) {
            in_block = false;
        }
        // lines inside a block (incl. the begin/end lines) are dropped.
    }
    if in_block {
        bail!(
            "unbalanced falsify fence: a `{BEGIN} ...` line has no matching `{END} ...` — \
             refusing to parse a malformed page"
        );
    }
    Ok(out)
}

/// True if a fence line carries exactly `key` (e.g. `topic=o3-speedup`), delimited on BOTH sides so
/// neither `topic=o3` (right side — `topic=o3-speedup`) nor `subtopic=x` (left side — an attribute
/// that merely ends in `topic=x`) matches. A key is bounded by whitespace or the end of the line.
fn has_key(fence_line: &str, key: &str) -> bool {
    let mut from = 0;
    while let Some(rel) = fence_line[from..].find(key) {
        let i = from + rel;
        let before_ok = i == 0 || fence_line[..i].ends_with([' ', '\t']);
        let after_ok = matches!(
            fence_line[i + key.len()..].chars().next(),
            Some(' ') | Some('\t') | None
        );
        if before_ok && after_ok {
            return true;
        }
        from = i + 1;
    }
    false
}

/// Byte range `[start, end)` of the fenced block whose begin AND end lines carry `key`,
/// spanning from the first byte of the begin line through the newline after the end line.
/// `None` if there is no such complete block.
pub fn find_block(text: &str, key: &str) -> Option<(usize, usize)> {
    let mut off = 0usize;
    let mut begin_at: Option<usize> = None;
    for line in text.split_inclusive('\n') {
        let t = line.trim_start();
        let line_start = off;
        off += line.len();
        match begin_at {
            None => {
                if t.starts_with(BEGIN) && has_key(t, key) {
                    begin_at = Some(line_start);
                }
            }
            Some(s) => {
                if t.starts_with(END) && has_key(t, key) {
                    return Some((s, off));
                }
            }
        }
    }
    None
}

/// Replace the block keyed by `key` with `block` in place, or append `block` (after a blank
/// line) if absent. `block` must end with a single newline. Convergent: appending then
/// re-replacing yields a byte-identical result, so `persist` stays idempotent.
pub fn upsert_block(text: &str, key: &str, block: &str) -> String {
    if let Some((s, e)) = find_block(text, key) {
        let mut out = String::with_capacity(text.len() + block.len());
        out.push_str(&text[..s]);
        out.push_str(block);
        out.push_str(&text[e..]);
        out
    } else {
        let mut base = text.to_string();
        if !base.is_empty() {
            if !base.ends_with('\n') {
                base.push('\n');
            }
            if !base.ends_with("\n\n") {
                base.push('\n');
            }
        }
        base.push_str(block);
        base
    }
}

/// Build the opening fence line for `key` (no trailing newline).
pub fn begin_line(key: &str, created: &str) -> String {
    format!("{BEGIN} {key} created={created} -->")
}

/// Build the closing fence line for `key` (no trailing newline).
pub fn end_line(key: &str) -> String {
    format!("{END} {key} -->")
}

/// The operator's My-read text from a block region (between the My-read sub-markers), if present.
pub fn existing_my_read(region: &str) -> Option<String> {
    let start = region.find(MY_READ_START)? + MY_READ_START.len();
    let rest = &region[start..];
    let end = rest.find(MY_READ_END)?;
    Some(rest[..end].trim_matches('\n').to_string())
}

/// The `created=<token>` attribute from a begin fence line, if present.
pub fn existing_created(begin_line: &str) -> Option<String> {
    let i = begin_line.find("created=")? + "created=".len();
    let tok: String = begin_line[i..]
        .chars()
        .take_while(|c| !c.is_whitespace())
        .collect();
    if tok.is_empty() {
        None
    } else {
        Some(tok)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn strip_removes_blocks_and_keeps_canon() {
        let page = "# Perf\n\nreal canon line.\n\n<!-- falsify:begin topic=x created=2026-06-14 -->\n## Falsified: X\n-O3 inside the block.\n<!-- falsify:end topic=x -->\n\ntrailing canon.\n";
        let s = strip(page).unwrap();
        assert!(s.contains("real canon line."));
        assert!(s.contains("trailing canon."));
        assert!(!s.contains("-O3 inside the block."));
        assert!(!s.contains("falsify:begin"));
    }

    #[test]
    fn strip_no_fence_is_noop() {
        let txt = "plain book text\nno fences here\n";
        assert_eq!(strip(txt).unwrap(), txt);
    }

    #[test]
    fn strip_errors_on_unbalanced() {
        let bad = "intro\n<!-- falsify:begin topic=x created=d -->\nbody never closed\n";
        assert!(strip(bad).is_err());
    }

    #[test]
    fn strip_errors_on_orphan_end() {
        // an `end` with no matching `begin` is malformed — fail closed, don't leak it as canon
        let bad = "intro\n<!-- falsify:end topic=x -->\ntrailing\n";
        assert!(strip(bad).is_err());
    }

    #[test]
    fn has_key_is_anchored_on_both_sides() {
        // right side: topic=o3 must not match topic=o3-speedup (existing guard)
        let line = format!("{BEGIN} topic=o3-speedup created=d -->");
        assert!(has_key(&line, "topic=o3-speedup"));
        assert!(!has_key(&line, "topic=o3"));
        // left side: topic=x must not match an attribute that merely ends in it (subtopic=x)
        let line2 = format!("{BEGIN} subtopic=x created=d -->");
        assert!(!has_key(&line2, "topic=x"));
        // but a genuine topic=x with a subtopic sibling still matches
        let line3 = format!("{BEGIN} topic=x subtopic=y -->");
        assert!(has_key(&line3, "topic=x"));
    }

    #[test]
    fn upsert_is_idempotent_append_then_replace() {
        let page = "# Perf\n\ncanon.\n";
        let block = format!(
            "{}\n## Falsified: X\nbody v1\n{}\n",
            begin_line("topic=x", "2026-06-14"),
            end_line("topic=x")
        );
        let once = upsert_block(page, "topic=x", &block);
        let twice = upsert_block(&once, "topic=x", &block);
        assert_eq!(once, twice, "append then replace must converge");
        assert!(once.starts_with("# Perf\n\ncanon.\n"));
        // a new render of the same block replaces in place
        let block2 = format!(
            "{}\n## Falsified: X\nbody v2\n{}\n",
            begin_line("topic=x", "2026-06-14"),
            end_line("topic=x")
        );
        let updated = upsert_block(&once, "topic=x", &block2);
        assert!(updated.contains("body v2"));
        assert!(!updated.contains("body v1"));
        assert!(updated.starts_with("# Perf\n\ncanon.\n"));
    }

    #[test]
    fn keys_are_delimited() {
        let page = format!(
            "{}\nbody\n{}\n",
            begin_line("topic=o3-speedup", "d"),
            end_line("topic=o3-speedup")
        );
        assert!(find_block(&page, "topic=o3-speedup").is_some());
        assert!(
            find_block(&page, "topic=o3").is_none(),
            "prefix must not match"
        );
    }

    #[test]
    fn two_topics_coexist() {
        let mut page = "# Page\n".to_string();
        let a = format!(
            "{}\nA\n{}\n",
            begin_line("topic=a", "d"),
            end_line("topic=a")
        );
        let b = format!(
            "{}\nB\n{}\n",
            begin_line("topic=b", "d"),
            end_line("topic=b")
        );
        page = upsert_block(&page, "topic=a", &a);
        page = upsert_block(&page, "topic=b", &b);
        assert!(find_block(&page, "topic=a").is_some());
        assert!(find_block(&page, "topic=b").is_some());
        // updating a leaves b intact
        let a2 = format!(
            "{}\nA2\n{}\n",
            begin_line("topic=a", "d"),
            end_line("topic=a")
        );
        page = upsert_block(&page, "topic=a", &a2);
        assert!(page.contains("A2") && page.contains("\nB\n"));
    }

    #[test]
    fn recover_created_and_my_read() {
        let region = format!(
            "{}\n### My read\n{}\nmy actual take\n{}\n{}\n",
            begin_line("topic=x", "2026-06-01"),
            MY_READ_START,
            MY_READ_END,
            end_line("topic=x")
        );
        let begin = region.lines().next().unwrap();
        assert_eq!(existing_created(begin).as_deref(), Some("2026-06-01"));
        assert_eq!(existing_my_read(&region).as_deref(), Some("my actual take"));
    }
}
