//! verify-core — the shared verbatim-verification kernel (falsify + fetchfix).
//!
//! One question, answered deterministically: is this quoted text really in that
//! source — and where? Normalization tolerates exactly the cosmetic drift LLMs
//! introduce when copying text (curly quotes, unicode dashes, ellipsis, collapsed
//! whitespace); it never tolerates wording changes.

/// Map unicode punctuation to ASCII equivalents so quotes survive smart-quote /
/// dash / ellipsis drift between a source and an LLM's copy of it.
pub fn straighten(text: &str) -> String {
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

/// Normalize for verbatim existence matching: straighten unicode punctuation and
/// collapse all whitespace runs (including newlines) to single spaces.
pub fn normalize_for_match(text: &str) -> String {
    straighten(text)
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
}

/// A source text indexed for normalized quote → line-anchor lookup.
///
/// The haystack is the whole text normalized with newlines collapsed, so a quote
/// that wraps across source lines (reflow) still matches; a byte→line map recovers
/// the 1-based source line span of every hit.
pub struct NormText {
    norm: String,
    line_of: Vec<u32>, // line_of[byte index into norm] = 1-based source line
}

impl NormText {
    pub fn new(raw: &str) -> Self {
        let mut norm = String::new();
        let mut line_of = Vec::new();
        for (i, line) in raw.lines().enumerate() {
            let n = normalize_for_match(line);
            if n.is_empty() {
                continue;
            }
            if !norm.is_empty() {
                // separator between lines; attribute it to the previous line
                norm.push(' ');
                line_of.push(*line_of.last().unwrap());
            }
            line_of.resize(line_of.len() + n.len(), (i + 1) as u32);
            norm.push_str(&n);
        }
        NormText { norm, line_of }
    }

    /// Every non-overlapping occurrence of `quote` (normalized), as 1-based
    /// (start_line, end_line) source spans. Empty quote matches nothing.
    pub fn find(&self, quote: &str) -> Vec<(usize, usize)> {
        let needle = normalize_for_match(quote);
        let mut out = Vec::new();
        if needle.is_empty() {
            return out;
        }
        let mut from = 0;
        while let Some(p) = self.norm[from..].find(&needle) {
            let at = from + p;
            out.push((
                self.line_of[at] as usize,
                self.line_of[at + needle.len() - 1] as usize,
            ));
            from = at + needle.len();
        }
        out
    }

    pub fn contains(&self, quote: &str) -> bool {
        !self.find(quote).is_empty()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn straighten_maps_unicode_punctuation() {
        assert_eq!(
            straighten("\u{2018}a\u{2019} \u{201C}b\u{201D}"),
            "'a' \"b\""
        );
        assert_eq!(straighten("x\u{2013}y\u{2014}z\u{2012}w"), "x-y-z-w");
        assert_eq!(straighten("wait\u{2026}"), "wait...");
        assert_eq!(straighten("a\u{00A0}b"), "a b");
    }

    #[test]
    fn normalize_collapses_whitespace_and_trims() {
        assert_eq!(normalize_for_match("  a \t b\n\nc  "), "a b c");
    }

    #[test]
    fn find_single_line_anchor() {
        let t = NormText::new("alpha\nthe hot loop is vectorized\nomega");
        assert_eq!(t.find("hot loop is"), vec![(2, 2)]);
    }

    #[test]
    fn find_survives_reflow_across_lines() {
        let t = NormText::new("the hot loop\nis vectorized by -O3\ntail");
        // quote as one line, source wrapped across two
        assert_eq!(
            t.find("hot loop is \u{201C}vectorized\u{201D} by \u{2013}O3"),
            vec![(1, 2)]
        );
    }

    #[test]
    fn find_skips_blank_lines_inside_span() {
        let t = NormText::new("first part\n\nsecond part");
        assert_eq!(t.find("first part second part"), vec![(1, 3)]);
    }

    #[test]
    fn find_all_occurrences_non_overlapping() {
        let t = NormText::new("dup line\nother\ndup line");
        assert_eq!(t.find("dup line"), vec![(1, 1), (3, 3)]);
        assert!(t.contains("dup line"));
        assert!(!t.contains("absent"));
    }

    #[test]
    fn empty_quote_matches_nothing() {
        let t = NormText::new("anything");
        assert!(t.find("   ").is_empty());
        assert!(!t.contains(""));
    }
}
