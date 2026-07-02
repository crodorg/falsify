//! Phase 3 — compounding semantics: A1 (snapshot + guard — a re-persist that would drop claims the
//! existing block records WARNS loudly instead of losing them silently) and A7 (stale backlink
//! marks are GC'd when a page no longer contributes to a topic).

use std::fs;
use std::path::PathBuf;

mod common;
use common::{falsify, persist_apply};

fn new_run(runp: &str, source: &str, corpus: &str, wiki: &str) {
    let o = falsify(
        &[
            "new-run",
            "--source",
            source,
            "--as-of",
            "2026-06-28",
            "--run-dir",
            runp,
        ],
        corpus,
        wiki,
    );
    assert!(
        o.status.success(),
        "new-run: {}",
        String::from_utf8_lossy(&o.stderr)
    );
}

/// A1: the topic block records its claim ids in a `claims=` fence attribute; re-persisting the same
/// topic from a run that OMITS a previously-recorded claim warns that the claim's verdict is being
/// dropped (snapshot semantics — latest run wins), instead of silently losing it.
#[test]
fn snapshot_guard_warns_on_dropped_claims() {
    let base = PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join("snap-guard");
    let _ = fs::remove_dir_all(&base);

    let corpus = base.join("corpus");
    let bookdir = corpus.join("a/books/txt");
    fs::create_dir_all(&bookdir).unwrap();
    let book = bookdir.join("b.txt");
    fs::write(&book, "Claim one text here.\nClaim two text here.\n").unwrap();
    let book_s = book.to_str().unwrap().to_string();
    let wiki = base.join("wiki");
    fs::create_dir_all(wiki.join("concepts")).unwrap();
    let source = base.join("s.md");
    fs::write(&source, "x").unwrap();
    let run = base.join("run");
    let (corpus_s, wiki_s) = (corpus.to_str().unwrap(), wiki.to_str().unwrap());
    let runp = run.to_str().unwrap();

    new_run(runp, source.to_str().unwrap(), corpus_s, wiki_s);
    fs::write(
        run.join("claims.json"),
        r#"[{"claim":"claim one","falsifiability":"falsifiable","claim_date":null,"suggested_pin":null},
            {"claim":"claim two","falsifiability":"falsifiable","claim_date":null,"suggested_pin":null}]"#,
    )
    .unwrap();
    let o = falsify(&["validate", "--run-dir", runp], corpus_s, wiki_s);
    assert!(o.status.success());
    let out = String::from_utf8_lossy(&o.stdout);
    let ids: Vec<String> = out
        .lines()
        .map(|l| l.split_whitespace().next().unwrap().to_string())
        .collect();
    let (id1, id2) = (ids[0].clone(), ids[1].clone());

    let audits_both = format!(
        r#"[{{"claim_id":"{id1}","author":"a","map_fragments":[{{"person":"P","source_ref":"b","source_path":"{book_s}","quote":"Claim one text here","kind":"book","gloss":null}}],"contradictions":[],"silence":null}},
            {{"claim_id":"{id2}","author":"a","map_fragments":[{{"person":"P","source_ref":"b","source_path":"{book_s}","quote":"Claim two text here","kind":"book","gloss":null}}],"contradictions":[],"silence":null}}]"#
    );
    let verdicts_both = format!(
        r#"[{{"claim_id":"{id1}","label":"match","confidence":"low","load_bearing_pin":null,"temporal_flag":null,"votes":[],"rationale":"x"}},
            {{"claim_id":"{id2}","label":"match","confidence":"low","load_bearing_pin":null,"temporal_flag":null,"votes":[],"rationale":"x"}}]"#
    );
    fs::write(run.join("audits.json"), &audits_both).unwrap();
    fs::write(run.join("verdicts.json"), &verdicts_both).unwrap();
    assert!(
        falsify(&["verify-evidence", "--run-dir", runp], corpus_s, wiki_s)
            .status
            .success()
    );

    let pargs = [
        "persist",
        "--run-dir",
        runp,
        "--page",
        "concepts/topic.md",
        "--topic",
        "T",
    ];
    let o = persist_apply(&pargs, corpus_s, wiki_s);
    assert!(
        o.status.success(),
        "persist both: {}",
        String::from_utf8_lossy(&o.stderr)
    );

    // the block records both claim ids in the fence attribute
    let page = wiki.join("concepts/topic.md");
    let body = fs::read_to_string(&page).unwrap();
    assert!(
        body.contains("claims="),
        "block must record its claim ids:\n{body}"
    );

    // now re-persist the SAME topic from a run that dropped claim two
    let audits_one = format!(
        r#"[{{"claim_id":"{id1}","author":"a","map_fragments":[{{"person":"P","source_ref":"b","source_path":"{book_s}","quote":"Claim one text here","kind":"book","gloss":null}}],"contradictions":[],"silence":null}}]"#
    );
    let verdicts_one = format!(
        r#"[{{"claim_id":"{id1}","label":"match","confidence":"low","load_bearing_pin":null,"temporal_flag":null,"votes":[],"rationale":"x"}}]"#
    );
    fs::write(run.join("audits.json"), &audits_one).unwrap();
    fs::write(run.join("verdicts.json"), &verdicts_one).unwrap();
    assert!(
        falsify(&["verify-evidence", "--run-dir", runp], corpus_s, wiki_s)
            .status
            .success()
    );

    let o = falsify(&pargs, corpus_s, wiki_s); // propose mode — the guard warns during render
    assert!(
        o.status.success(),
        "re-persist proposes: {}",
        String::from_utf8_lossy(&o.stderr)
    );
    let err = String::from_utf8_lossy(&o.stderr);
    assert!(
        err.contains("DROPS") && err.contains(&id2),
        "must warn that claim {id2} is dropped:\n{err}"
    );
}

/// A7: a page that carried a backlink mark for a topic in one run, but contributes no pin in the
/// next, has its stale mark GC'd (proposed for removal) — its canon prose untouched.
#[test]
fn stale_marks_are_gced() {
    let base = PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join("mark-gc");
    let _ = fs::remove_dir_all(&base);

    let corpus = base.join("corpus");
    fs::create_dir_all(corpus.join("a/books/txt")).unwrap();
    fs::write(corpus.join("a/books/txt/b.txt"), "unused.\n").unwrap();
    let wiki = base.join("wiki");
    fs::create_dir_all(wiki.join("concepts")).unwrap();
    let x = wiki.join("concepts/x.md");
    let y = wiki.join("concepts/y.md");
    fs::write(&x, "# X\n\nThe x quote lives here.\n").unwrap();
    fs::write(&y, "# Y\n\nThe y quote lives here.\n").unwrap();
    let (x_s, y_s) = (
        x.to_str().unwrap().to_string(),
        y.to_str().unwrap().to_string(),
    );
    let source = base.join("s.md");
    fs::write(&source, "x").unwrap();
    let run = base.join("run");
    let (corpus_s, wiki_s) = (corpus.to_str().unwrap(), wiki.to_str().unwrap());
    let runp = run.to_str().unwrap();

    new_run(runp, source.to_str().unwrap(), corpus_s, wiki_s);
    fs::write(
        run.join("claims.json"),
        r#"[{"claim":"the claim","falsifiability":"falsifiable","claim_date":null,"suggested_pin":null}]"#,
    )
    .unwrap();
    let o = falsify(&["validate", "--run-dir", runp], corpus_s, wiki_s);
    let id = String::from_utf8_lossy(&o.stdout)
        .split_whitespace()
        .next()
        .unwrap()
        .to_string();
    fs::write(run.join("verdicts.json"), "[]").unwrap();

    let pargs = [
        "persist",
        "--run-dir",
        runp,
        "--page",
        "concepts/topic.md",
        "--topic",
        "T",
    ];

    // run 1: the pin comes from x.md → x.md gets a backlink mark
    fs::write(
        run.join("audits.json"),
        format!(r#"[{{"claim_id":"{id}","author":"a","map_fragments":[{{"person":"P","source_ref":"x","source_path":"{x_s}","quote":"The x quote lives here","kind":"book","gloss":null}}],"contradictions":[],"silence":null}}]"#),
    )
    .unwrap();
    assert!(
        falsify(&["verify-evidence", "--run-dir", runp], corpus_s, wiki_s)
            .status
            .success()
    );
    assert!(persist_apply(&pargs, corpus_s, wiki_s).status.success());
    assert!(
        fs::read_to_string(&x).unwrap().contains("mark=t"),
        "x.md should carry a mark after run 1"
    );

    // run 2: the pin now comes from y.md → y.md gets a mark, x.md's stale mark is GC'd
    fs::write(
        run.join("audits.json"),
        format!(r#"[{{"claim_id":"{id}","author":"a","map_fragments":[{{"person":"P","source_ref":"y","source_path":"{y_s}","quote":"The y quote lives here","kind":"book","gloss":null}}],"contradictions":[],"silence":null}}]"#),
    )
    .unwrap();
    assert!(
        falsify(&["verify-evidence", "--run-dir", runp], corpus_s, wiki_s)
            .status
            .success()
    );
    assert!(persist_apply(&pargs, corpus_s, wiki_s).status.success());

    let xbody = fs::read_to_string(&x).unwrap();
    let ybody = fs::read_to_string(&y).unwrap();
    assert!(
        !xbody.contains("mark=t"),
        "x.md's stale mark must be GC'd:\n{xbody}"
    );
    assert!(
        xbody.contains("The x quote lives here."),
        "x.md canon prose must be preserved:\n{xbody}"
    );
    assert!(
        ybody.contains("mark=t"),
        "y.md should carry the mark after run 2:\n{ybody}"
    );
}
