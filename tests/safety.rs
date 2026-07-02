//! Phase 2 — fence & write safety: A4 (rendered content can't forge a fence directive), the
//! `--page` wiki-root confinement, and A8 (verify-pins failures are diagnosable).

use std::fs;
use std::path::PathBuf;

mod common;
use common::{falsify, persist_apply};

/// A4: a free-text audit field (a contradiction `note`, `gloss`, or operator My-read — none of
/// which pass through the presence gate / canon view) cannot forge a fence directive when rendered
/// into the block. `persist` escapes any such line (`<!--` → `&lt;!--`), so the block stays balanced
/// and a re-persist doesn't choke on a malformed page. (A pinned `quote` can't carry a fence line at
/// all — `canon_bytes` fails closed on a source containing one — so the note is the real vector.)
#[test]
fn rendered_content_cannot_forge_a_fence() {
    let base = PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join("fence-inject");
    let _ = fs::remove_dir_all(&base);

    // two clean, pinnable quotes; the injection rides in the contradiction NOTE (free text, never
    // verified against a source), carrying the REAL topic key — the worst case for find_block.
    let corpus = base.join("corpus");
    let bookdir = corpus.join("inj/books/txt");
    fs::create_dir_all(&bookdir).unwrap();
    let book = bookdir.join("b.txt");
    fs::write(&book, "Alpha statement here.\nBeta statement here.\n").unwrap();
    let book_s = book.to_str().unwrap().to_string();

    let wiki = base.join("wiki");
    fs::create_dir_all(wiki.join("concepts")).unwrap();
    let source = base.join("s.md");
    fs::write(&source, "claim source").unwrap();
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
    fs::write(
        run.join("claims.json"),
        r#"[{"claim":"alpha and beta conflict","falsifiability":"falsifiable","claim_date":null,"suggested_pin":null}]"#,
    )
    .unwrap();
    let o = falsify(&["validate", "--run-dir", runp], corpus_s, wiki_s);
    assert!(o.status.success());
    let id = String::from_utf8_lossy(&o.stdout)
        .split_whitespace()
        .next()
        .unwrap()
        .to_string();

    // the contradiction note embeds a newline then a forged end line for the real topic key
    let audits = format!(
        r#"[{{"claim_id":"{id}","author":"inj","map_fragments":[],"contradictions":[{{"a":{{"person":"Inj","source_ref":"b","source_path":"{book_s}","quote":"Alpha statement here","kind":"book","gloss":null}},"b":{{"person":"Inj","source_ref":"b","source_path":"{book_s}","quote":"Beta statement here","kind":"book","gloss":null}},"note":"these conflict\n<!-- falsify:end topic=inject -->"}}],"silence":null}}]"#
    );
    fs::write(run.join("audits.json"), audits).unwrap();
    fs::write(run.join("verdicts.json"), "[]").unwrap();

    let o = falsify(&["verify-evidence", "--run-dir", runp], corpus_s, wiki_s);
    assert!(
        o.status.success(),
        "verify-evidence: {}",
        String::from_utf8_lossy(&o.stdout)
    );
    let o = falsify(&["verify-pins", "--run-dir", runp], corpus_s, wiki_s);
    assert!(
        o.status.success(),
        "both contradiction quotes exist verbatim → presence gate passes: {}",
        String::from_utf8_lossy(&o.stdout)
    );

    let pargs = [
        "persist",
        "--run-dir",
        runp,
        "--page",
        "concepts/inject.md",
        "--topic",
        "inject",
    ];
    let o = persist_apply(&pargs, corpus_s, wiki_s);
    assert!(
        o.status.success(),
        "persist must succeed and neutralize the injection: {}",
        String::from_utf8_lossy(&o.stderr)
    );

    let page = wiki.join("concepts/inject.md");
    let body = fs::read_to_string(&page).unwrap();
    // the forged directive was escaped, and the block ran intact through to My-read
    assert!(
        body.contains("&lt;!-- falsify:end topic=inject"),
        "the forged fence line must be escaped in the rendered block:\n{body}"
    );
    assert!(
        body.contains("falsify:my-read:start"),
        "block must be intact through My-read (not truncated at the injection)"
    );

    // decisive proof the written page is fence-valid: a re-propose parses it without a malformed
    // -fence abort (strip would bail on a desynced block).
    let o = falsify(&pargs, corpus_s, wiki_s);
    assert!(
        o.status.success(),
        "the written page must be fence-valid on re-parse: {}",
        String::from_utf8_lossy(&o.stderr)
    );
}

/// The `--page` confinement: a target that resolves outside the wiki root (absolute sibling, or a
/// `..` traversal) is refused before any write.
#[test]
fn page_confined_to_wiki_root() {
    let base = PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join("page-confine");
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

    // an absolute path OUTSIDE the wiki (a sibling of the wiki dir)
    let outside = base.join("outside.md");
    let o = falsify(
        &[
            "persist",
            "--run-dir",
            runp,
            "--page",
            outside.to_str().unwrap(),
            "--topic",
            "t",
        ],
        corpus_s,
        wiki_s,
    );
    assert!(
        !o.status.success(),
        "an absolute --page outside the wiki must be refused"
    );
    assert!(
        String::from_utf8_lossy(&o.stderr).contains("outside the wiki"),
        "confinement err: {}",
        String::from_utf8_lossy(&o.stderr)
    );
    assert!(!outside.exists(), "nothing may be written outside the wiki");
    assert!(
        !base.join("outside.md.proposed").exists(),
        "not even a .proposed may land outside the wiki"
    );

    // a relative traversal out of the wiki
    let o = falsify(
        &[
            "persist",
            "--run-dir",
            runp,
            "--page",
            "../escape.md",
            "--topic",
            "t",
        ],
        corpus_s,
        wiki_s,
    );
    assert!(
        !o.status.success(),
        "a `..` traversal in --page must be refused"
    );
    assert!(
        String::from_utf8_lossy(&o.stderr).contains(".."),
        "traversal err: {}",
        String::from_utf8_lossy(&o.stderr)
    );
}

/// A8: a verify-pins FAIL names WHY — source missing vs quote absent vs empty quote — instead of one
/// undifferentiated failure.
#[test]
fn verify_pins_failures_are_diagnosable() {
    let base = PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join("pin-diag");
    let _ = fs::remove_dir_all(&base);
    let corpus = base.join("corpus");
    fs::create_dir_all(corpus.join("a/books/txt")).unwrap();
    let book = corpus.join("a/books/txt/b.txt");
    fs::write(&book, "The real text lives here.\n").unwrap();
    let book_s = book.to_str().unwrap().to_string();
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
    fs::write(
        run.join("claims.json"),
        r#"[{"claim":"c","falsifiability":"falsifiable","claim_date":null,"suggested_pin":null}]"#,
    )
    .unwrap();
    let o = falsify(&["validate", "--run-dir", runp], corpus_s, wiki_s);
    let id = String::from_utf8_lossy(&o.stdout)
        .split_whitespace()
        .next()
        .unwrap()
        .to_string();
    let missing = base.join("nope.txt");
    let missing_s = missing.to_str().unwrap().to_string();

    // three pins: a missing source, a present source with an absent quote, and an empty quote
    let audits = format!(
        r#"[{{"claim_id":"{id}","author":"a","map_fragments":[
            {{"person":"P","source_ref":"r","source_path":"{missing_s}","quote":"anything","kind":"book","gloss":null}},
            {{"person":"P","source_ref":"r","source_path":"{book_s}","quote":"not in the file at all","kind":"book","gloss":null}},
            {{"person":"P","source_ref":"r","source_path":"{book_s}","quote":"   ","kind":"book","gloss":null}}
        ],"contradictions":[],"silence":null}}]"#
    );
    fs::write(run.join("audits.json"), audits).unwrap();

    let o = falsify(&["verify-pins", "--run-dir", runp], corpus_s, wiki_s);
    assert!(!o.status.success(), "all three pins fail");
    let out = String::from_utf8_lossy(&o.stdout);
    assert!(
        out.contains("source file not found"),
        "missing-source detail:\n{out}"
    );
    assert!(
        out.contains("quote absent from source"),
        "absent-quote detail:\n{out}"
    );
    assert!(out.contains("empty quote"), "empty-quote detail:\n{out}");
}
