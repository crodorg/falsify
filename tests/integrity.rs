//! Deterministic schema-integrity gates (A5 + P1): cross-referential integrity, the falsifiability
//! gate, and `deny_unknown_fields`. These are the "deterministic, golden-tested" gates the README
//! lists — this binary proves they actually fire, rather than being trusted from the LLM.

use std::fs;
use std::path::PathBuf;

mod common;
use common::falsify;

/// `validate` enforces schema integrity — the falsifiability gate (a not_falsifiable claim gets no
/// rubric call; a falsifiable claim isn't labeled not_falsifiable), cross-referential integrity (no
/// verdict may reference a claim that doesn't exist), and `deny_unknown_fields` (a bogus field in a
/// run artifact is rejected, not silently ignored).
#[test]
fn falsifiability_gate_and_cross_ref_integrity() {
    let base = PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join("gate");
    let _ = fs::remove_dir_all(&base);

    let corpus = base.join("corpus");
    fs::create_dir_all(corpus.join("a/books/txt")).unwrap();
    fs::write(corpus.join("a/books/txt/b.txt"), "unused.\n").unwrap();
    let wiki = base.join("wiki");
    fs::create_dir_all(&wiki).unwrap();
    let source = base.join("s.md");
    fs::write(&source, "x").unwrap();
    let run = base.join("run");
    let (corpus_s, wiki_s) = (corpus.to_str().unwrap(), wiki.to_str().unwrap());
    let runp = run.to_str().unwrap();

    let o = falsify(
        &[
            "new-run",
            "--source",
            source.to_str().unwrap(),
            "--as-of",
            "2026-06-20",
            "--run-dir",
            runp,
        ],
        corpus_s,
        wiki_s,
    );
    assert!(o.status.success());

    // a not_falsifiable claim
    fs::write(
        run.join("claims.json"),
        r#"[{"claim":"beauty matters most","falsifiability":"not_falsifiable","claim_date":null,"suggested_pin":null}]"#,
    )
    .unwrap();
    let o = falsify(&["validate", "--run-dir", runp], corpus_s, wiki_s);
    assert!(o.status.success());
    let id = String::from_utf8_lossy(&o.stdout)
        .split_whitespace()
        .next()
        .unwrap()
        .to_string();

    // (a) not_falsifiable claim + a match verdict → the falsifiability gate aborts validate
    fs::write(
        run.join("audits.json"),
        format!(r#"[{{"claim_id":"{id}","author":"a","map_fragments":[],"contradictions":[],"silence":null}}]"#),
    )
    .unwrap();
    fs::write(
        run.join("verdicts.json"),
        format!(r#"[{{"claim_id":"{id}","label":"match","confidence":"low","load_bearing_pin":null,"temporal_flag":null,"rationale":"x"}}]"#),
    )
    .unwrap();
    let o = falsify(&["validate", "--run-dir", runp], corpus_s, wiki_s);
    assert!(
        !o.status.success(),
        "a not_falsifiable claim with a match verdict must fail the falsifiability gate"
    );
    assert!(
        String::from_utf8_lossy(&o.stderr).contains("falsifiability"),
        "gate err: {}",
        String::from_utf8_lossy(&o.stderr)
    );

    // (b) the correct verdict (not_falsifiable) passes
    fs::write(
        run.join("verdicts.json"),
        format!(r#"[{{"claim_id":"{id}","label":"not_falsifiable","confidence":"low","load_bearing_pin":null,"temporal_flag":null,"rationale":"routed out"}}]"#),
    )
    .unwrap();
    let o = falsify(&["validate", "--run-dir", runp], corpus_s, wiki_s);
    assert!(
        o.status.success(),
        "not_falsifiable claim + not_falsifiable verdict must pass: {}",
        String::from_utf8_lossy(&o.stderr)
    );

    // (c) a verdict referencing an unknown claim id → cross-ref integrity aborts
    fs::write(
        run.join("verdicts.json"),
        r#"[{"claim_id":"deadbeef0000","label":"not_falsifiable","confidence":"low","load_bearing_pin":null,"temporal_flag":null,"rationale":"x"}]"#,
    )
    .unwrap();
    let o = falsify(&["validate", "--run-dir", runp], corpus_s, wiki_s);
    assert!(
        !o.status.success(),
        "a verdict with a dangling claim_id must fail cross-ref integrity"
    );
    assert!(
        String::from_utf8_lossy(&o.stderr).contains("unknown claim"),
        "cross-ref err: {}",
        String::from_utf8_lossy(&o.stderr)
    );

    // (d) deny_unknown_fields: a bogus field in claims.json is rejected, not silently dropped
    fs::write(
        run.join("claims.json"),
        r#"[{"claim":"x","falsifiability":"falsifiable","claim_date":null,"suggested_pin":null,"bogus_field":true}]"#,
    )
    .unwrap();
    let o = falsify(&["validate", "--run-dir", runp], corpus_s, wiki_s);
    assert!(
        !o.status.success(),
        "an unknown field in claims.json must be rejected by deny_unknown_fields"
    );
}

/// The `suggest-contradictions` mechanical pre-filter flags two pins that assert disjoint numbers
/// about a shared subject (high-precision, cheap) — a suggestion the auditor confirms.
#[test]
fn suggest_contradictions_flags_numeric_conflict() {
    let base = PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join("suggest");
    let _ = fs::remove_dir_all(&base);
    let corpus = base.join("corpus");
    fs::create_dir_all(corpus.join("a/books/txt")).unwrap();
    fs::write(corpus.join("a/books/txt/b.txt"), "unused.\n").unwrap();
    let wiki = base.join("wiki");
    fs::create_dir_all(&wiki).unwrap();
    let source = base.join("s.md");
    fs::write(&source, "x").unwrap();
    let run = base.join("run");
    let (corpus_s, wiki_s) = (corpus.to_str().unwrap(), wiki.to_str().unwrap());
    let runp = run.to_str().unwrap();

    let o = falsify(
        &[
            "new-run",
            "--source",
            source.to_str().unwrap(),
            "--as-of",
            "2026-06-25",
            "--run-dir",
            runp,
        ],
        corpus_s,
        wiki_s,
    );
    assert!(o.status.success());
    // two pins, same author, disjoint numbers about the shared subject "inliner runtime"
    fs::write(
        run.join("audits.json"),
        r#"[{"claim_id":"c","author":"a","map_fragments":[
            {"person":"P","source_ref":"r","source_path":"p","quote":"the inliner cuts runtime by about 5 percent","kind":"book","gloss":null},
            {"person":"P","source_ref":"r","source_path":"p","quote":"the inliner cuts runtime by only 2 percent","kind":"book","gloss":null}
        ],"contradictions":[],"silence":null}]"#,
    )
    .unwrap();
    let o = falsify(
        &["suggest-contradictions", "--run-dir", runp],
        corpus_s,
        wiki_s,
    );
    assert!(o.status.success());
    assert!(
        String::from_utf8_lossy(&o.stdout).contains("numeric:"),
        "should flag the numeric conflict: {}",
        String::from_utf8_lossy(&o.stdout)
    );
}
