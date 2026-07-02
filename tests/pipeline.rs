//! End-to-end + determinism proof for the substrate. Drives the real CLI against a fixture
//! corpus + wiki (env-overridden), exercising every subcommand and asserting the load-bearing
//! invariants: the presence gate rejects fabrication, `verify-evidence` refutes a false silence
//! claim, `persist` writes the synthesis INLINE into a canon page (preserving everything outside
//! its fence), is idempotent, refuses to write against drifted canon, and never audits its own
//! fenced blocks. The worked compiler-optimization run is the integration test of judgment; this
//! is the determinism test of the machinery.

use std::fs;
use std::path::PathBuf;

mod common;
use common::{falsify, persist_apply};

#[test]
fn end_to_end_inline_persist_idempotency_and_input_pin() {
    let base = PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join("e2e");
    let _ = fs::remove_dir_all(&base);

    // fixture corpus: <corpus>/testauthor/books/txt/book.txt
    let corpus = base.join("corpus");
    let bookdir = corpus.join("testauthor/books/txt");
    fs::create_dir_all(&bookdir).unwrap();
    let book = bookdir.join("book.txt");
    fs::write(
        &book,
        "Chapter one.\nThe hot loop is vectorized by -O3 in tight numeric code.\nNothing here about that other thing.\n",
    )
    .unwrap();
    let book_s = book.to_str().unwrap().to_string();

    // fixture wiki + source doc; pre-create the host canon page with REAL canon content so we can
    // assert persist preserves everything outside its fenced block.
    let wiki = base.join("wiki");
    let page = wiki.join("concepts/compiler-optimization.md");
    fs::create_dir_all(page.parent().unwrap()).unwrap();
    fs::write(
        &page,
        "---\ntitle: Compiler optimization\ntype: concept\n---\n\n# Compiler optimization\n\nReal canon content about optimization passes.\n",
    )
    .unwrap();
    let source = base.join("source.md");
    fs::write(
        &source,
        "-O3 always makes programs faster; there is no downside.",
    )
    .unwrap();

    let run = base.join("run");
    let (corpus_s, wiki_s) = (corpus.to_str().unwrap(), wiki.to_str().unwrap());
    let runp = run.to_str().unwrap();

    // good audits/verdicts: a real verbatim pin + a silence flag whose terms are genuinely absent.
    let good_audits = format!(
        r#"[{{"claim_id":"{{id}}","author":"testauthor",
        "map_fragments":[{{"person":"Test Author","source_ref":"book","source_path":"{book_s}","quote":"The hot loop is vectorized by -O3","kind":"book","gloss":"-O3 vectorizes the hot loop"}}],
        "contradictions":[],
        "silence":{{"author":"testauthor","terms_searched":["profile-guided","autotuning"],"mechanism_checked":true}}}}]"#
    );

    // 1. new-run pins the source
    let o = falsify(
        &[
            "new-run",
            "--source",
            source.to_str().unwrap(),
            "--as-of",
            "2026-06-13",
            "--run-dir",
            runp,
        ],
        corpus_s,
        wiki_s,
    );
    assert!(
        o.status.success(),
        "new-run: {}",
        String::from_utf8_lossy(&o.stderr)
    );
    assert!(run.join("manifest.json").exists());

    // 2. claims + validate → learn the content-addressed id
    fs::write(
        run.join("claims.json"),
        r#"[{"claim":"-O3 always makes programs faster","falsifiability":"falsifiable","claim_date":null,"suggested_pin":null}]"#,
    )
    .unwrap();
    let o = falsify(&["validate", "--run-dir", runp], corpus_s, wiki_s);
    assert!(o.status.success());
    let out = String::from_utf8_lossy(&o.stdout);
    let id = out.split_whitespace().next().unwrap().to_string();

    // 3. write good audits + verdicts (with the real id)
    fs::write(run.join("audits.json"), good_audits.replace("{id}", &id)).unwrap();
    let verdicts = format!(
        r#"[{{"claim_id":"{id}","label":"match","confidence":"high","load_bearing_pin":{{"person":"Test Author","source_ref":"book","source_path":"{book_s}","quote":"vectorized by -O3","kind":"book","gloss":null}},"temporal_flag":null,"rationale":"canon agrees"}}]"#
    );
    fs::write(run.join("verdicts.json"), &verdicts).unwrap();

    // 4. verify-evidence: the silence claim survives falsification, and the slice freezes.
    let o = falsify(&["verify-evidence", "--run-dir", runp], corpus_s, wiki_s);
    assert!(
        o.status.success(),
        "verify-evidence: {}",
        String::from_utf8_lossy(&o.stdout)
    );
    assert!(
        String::from_utf8_lossy(&o.stdout).contains("absent"),
        "should report verified absence"
    );
    let manifest = fs::read_to_string(run.join("manifest.json")).unwrap();
    assert!(
        manifest.contains("book.txt"),
        "input slice must be frozen into the manifest"
    );

    // 5. verify-pins passes for real quotes
    let o = falsify(&["verify-pins", "--run-dir", runp], corpus_s, wiki_s);
    assert!(
        o.status.success(),
        "verify-pins should pass:\n{}",
        String::from_utf8_lossy(&o.stdout)
    );

    // 6. persist --apply writes the synthesis INLINE into the canon page, preserving the rest.
    let persist_args = [
        "persist",
        "--run-dir",
        runp,
        "--page",
        "concepts/compiler-optimization.md",
        "--topic",
        "O3 speedup evidence vs canon",
    ];
    let o = persist_apply(&persist_args, corpus_s, wiki_s);
    assert!(
        o.status.success(),
        "persist: {}",
        String::from_utf8_lossy(&o.stderr)
    );
    let p1 = fs::read_to_string(&page).unwrap();
    assert!(
        p1.contains("Real canon content about optimization passes."),
        "canon content outside the fence must be preserved"
    );
    assert!(
        p1.contains("# Compiler optimization"),
        "host H1 must be preserved"
    );
    assert!(
        p1.contains("<!-- falsify:begin topic=o3-speedup-evidence-vs-canon"),
        "fenced topic block missing"
    );
    assert!(
        p1.contains("## Falsified: O3 speedup evidence vs canon"),
        "block heading missing"
    );
    assert!(p1.contains("vectorized by -O3"), "pin missing");
    assert!(p1.contains("UNADDRESSED"), "silence flag missing");
    assert!(
        p1.contains("falsify:my-read:start"),
        "my-read markers missing"
    );
    assert!(
        !wiki
            .join("comparisons/o3-speedup-evidence-vs-canon.md")
            .exists(),
        "must NOT write a standalone comparisons/ page"
    );

    // 7. IDEMPOTENCY: re-persist the same run (propose+apply again) → byte-identical
    let o = persist_apply(&persist_args, corpus_s, wiki_s);
    assert!(o.status.success());
    let p2 = fs::read_to_string(&page).unwrap();
    assert_eq!(p1, p2, "persist must be idempotent (zero diff on re-run)");

    // 8. MY-READ protection: operator edits the block; re-persist preserves it
    let edited = p2.replace(
        "*Operator only.",
        "MY ACTUAL TAKE: -O3 is usually fine.\n\n*Operator only.",
    );
    fs::write(&page, &edited).unwrap();
    let _ = persist_apply(&persist_args, corpus_s, wiki_s);
    let p3 = fs::read_to_string(&page).unwrap();
    assert!(
        p3.contains("MY ACTUAL TAKE: -O3 is usually fine."),
        "my-read must survive re-persist"
    );
    assert!(
        p3.contains("Real canon content about optimization passes."),
        "canon must still survive"
    );

    // 8b. MULTI-TOPIC: a second topic on the same page coexists with the first.
    let topic2_args = [
        "persist",
        "--run-dir",
        runp,
        "--page",
        "concepts/compiler-optimization.md",
        "--topic",
        "Link-time optimization",
    ];
    let o = persist_apply(&topic2_args, corpus_s, wiki_s);
    assert!(
        o.status.success(),
        "second topic persist: {}",
        String::from_utf8_lossy(&o.stderr)
    );
    let p4 = fs::read_to_string(&page).unwrap();
    assert!(
        p4.contains("topic=o3-speedup-evidence-vs-canon"),
        "first topic block must remain"
    );
    assert!(
        p4.contains("topic=link-time-optimization"),
        "second topic block must be added"
    );
    assert!(
        p4.contains("MY ACTUAL TAKE: -O3 is usually fine."),
        "first topic's my-read must survive a second topic's write"
    );

    // 9. COVERAGE reports the silence flag + the audited-vs-discoverable scope
    let o = falsify(&["coverage", "--run-dir", runp], corpus_s, wiki_s);
    assert!(o.status.success());
    let cov = String::from_utf8_lossy(&o.stdout);
    assert!(
        cov.contains("UNADDRESSED (silence) flags: 1"),
        "coverage: {cov}"
    );
    assert!(
        cov.contains("authors audited: testauthor"),
        "coverage: {cov}"
    );

    // 10. PRESENCE GATE: a fabricated quote is rejected and persist aborts
    let bad = format!(
        r#"[{{"claim_id":"{id}","author":"testauthor",
        "map_fragments":[{{"person":"Test Author","source_ref":"book","source_path":"{book_s}","quote":"this sentence does not occur anywhere in the source","kind":"book","gloss":null}}],
        "contradictions":[],"silence":null}}]"#
    );
    fs::write(run.join("audits.json"), bad).unwrap();
    let o = falsify(&["verify-pins", "--run-dir", runp], corpus_s, wiki_s);
    assert!(!o.status.success(), "fabricated pin must fail verify-pins");
    let o = falsify(&persist_args, corpus_s, wiki_s);
    assert!(!o.status.success(), "persist must abort on a bad pin");

    // 11. PRESENCE GATE: an empty/whitespace quote must also fail (contains("") is always true)
    let empty = format!(
        r#"[{{"claim_id":"{id}","author":"testauthor",
        "map_fragments":[{{"person":"T","source_ref":"book","source_path":"{book_s}","quote":"   ","kind":"book","gloss":null}}],
        "contradictions":[],"silence":null}}]"#
    );
    fs::write(run.join("audits.json"), empty).unwrap();
    let o = falsify(&["verify-pins", "--run-dir", runp], corpus_s, wiki_s);
    assert!(
        !o.status.success(),
        "empty/whitespace quote must fail verify-pins"
    );

    // 12. ABSENCE GATE: a silence claim whose term actually appears is REFUTED (verify-evidence fails)
    let false_silence = format!(
        r#"[{{"claim_id":"{id}","author":"testauthor","map_fragments":[],"contradictions":[],
        "silence":{{"author":"testauthor","terms_searched":["vectorized"],"mechanism_checked":true}}}}]"#
    );
    fs::write(run.join("audits.json"), false_silence).unwrap();
    let o = falsify(&["verify-evidence", "--run-dir", runp], corpus_s, wiki_s);
    assert!(!o.status.success(), "a false silence claim must be refuted");
    assert!(
        String::from_utf8_lossy(&o.stdout).contains("REFUTED"),
        "should name the refutation"
    );

    // 13. INPUT-PIN: persist refuses to write when frozen canon drifts after the audit
    fs::write(run.join("audits.json"), good_audits.replace("{id}", &id)).unwrap();
    fs::write(run.join("verdicts.json"), &verdicts).unwrap();
    let o = falsify(&["verify-evidence", "--run-dir", runp], corpus_s, wiki_s);
    assert!(
        o.status.success(),
        "re-freeze: {}",
        String::from_utf8_lossy(&o.stdout)
    );
    // mutate a frozen corpus file (pins still match — the point is the hash drift)
    fs::write(
        &book,
        "Chapter one.\nThe hot loop is vectorized by -O3 in tight numeric code.\nNothing here about that other thing.\nAppended line changes the hash.\n",
    )
    .unwrap();
    let o = falsify(
        &[&persist_args[..], &["--apply"]].concat(),
        corpus_s,
        wiki_s,
    );
    assert!(
        !o.status.success(),
        "persist must abort when frozen canon drifts"
    );
    assert!(
        String::from_utf8_lossy(&o.stderr).contains("drifted"),
        "drift abort should say so: {}",
        String::from_utf8_lossy(&o.stderr)
    );

    // 14. ARTIFACT FREEZE (A2): editing audits.json AFTER verify-evidence must abort persist — the
    //     absence gate's validated fields are otherwise the model's word between verify-evidence and
    //     persist. Restore the corpus, re-freeze, then forge a "validated" silence flag.
    fs::write(
        &book,
        "Chapter one.\nThe hot loop is vectorized by -O3 in tight numeric code.\nNothing here about that other thing.\n",
    )
    .unwrap();
    fs::write(run.join("audits.json"), good_audits.replace("{id}", &id)).unwrap();
    fs::write(run.join("verdicts.json"), &verdicts).unwrap();
    let o = falsify(&["verify-evidence", "--run-dir", runp], corpus_s, wiki_s);
    assert!(
        o.status.success(),
        "re-freeze for artifact test: {}",
        String::from_utf8_lossy(&o.stdout)
    );
    // forge a silence flag with fake computed fields (the exact post-gate tamper A2 defends against)
    let tampered = format!(
        r#"[{{"claim_id":"{id}","author":"testauthor",
        "map_fragments":[{{"person":"Test Author","source_ref":"book","source_path":"{book_s}","quote":"The hot loop is vectorized by -O3","kind":"book","gloss":"-O3 vectorizes the hot loop"}}],
        "contradictions":[],
        "silence":{{"author":"testauthor","terms_searched":["profile-guided","autotuning"],"scope":"author_books","corpus_scope":["{book_s}"],"lexical_empty":true,"mechanism_checked":true,"replay_hash":"deadbeefdeadbeef"}}}}]"#
    );
    fs::write(run.join("audits.json"), tampered).unwrap();
    let o = falsify(&persist_args, corpus_s, wiki_s); // propose mode is enough — abort is pre-write
    assert!(
        !o.status.success(),
        "editing audits.json after verify-evidence must abort persist"
    );
    assert!(
        String::from_utf8_lossy(&o.stderr).contains("changed since verify-evidence"),
        "artifact-drift abort should say so: {}",
        String::from_utf8_lossy(&o.stderr)
    );
}

/// Wiki-scoped silence: verify-evidence falsifies absence over the compiled wiki canon
/// (concepts/entities/hubs). It MUST exclude comparisons/ + sources/, AND strip falsify's own
/// fenced blocks — so neither legacy output, documents under examination, nor a verdict falsify
/// itself wrote can refute a canon-silence claim.
#[test]
fn wiki_scoped_silence_exclusions_and_self_refutation() {
    let base = PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join("e2e-wiki");
    let _ = fs::remove_dir_all(&base);

    // a book corpus must exist so the run is well-formed (unused here)
    let corpus = base.join("corpus");
    fs::create_dir_all(corpus.join("testauthor/books/txt")).unwrap();
    fs::write(
        corpus.join("testauthor/books/txt/book.txt"),
        "registers spill onto the stack.\n",
    )
    .unwrap();

    // wiki: canon content in concepts/, plus the term planted ONLY in excluded dirs
    let wiki = base.join("wiki");
    fs::create_dir_all(wiki.join("concepts")).unwrap();
    fs::create_dir_all(wiki.join("comparisons")).unwrap();
    fs::create_dir_all(wiki.join("sources")).unwrap();
    fs::write(
        wiki.join("concepts/perf.md"),
        "Performance depends on cache locality and branch prediction.\n",
    )
    .unwrap();
    fs::write(
        wiki.join("sources/thread.md"),
        "Profile-guided builds always autotune the hot path.\n",
    )
    .unwrap();
    fs::write(
        wiki.join("comparisons/old.md"),
        "a prior page discussing profile-guided evidence.\n",
    )
    .unwrap();

    let source = base.join("source.md");
    fs::write(&source, "Profile-guided builds never autotune.").unwrap();
    let run = base.join("run");
    let (corpus_s, wiki_s) = (corpus.to_str().unwrap(), wiki.to_str().unwrap());
    let runp = run.to_str().unwrap();

    let o = falsify(
        &[
            "new-run",
            "--source",
            source.to_str().unwrap(),
            "--as-of",
            "2026-06-14",
            "--run-dir",
            runp,
        ],
        corpus_s,
        wiki_s,
    );
    assert!(
        o.status.success(),
        "new-run: {}",
        String::from_utf8_lossy(&o.stderr)
    );
    fs::write(run.join("claims.json"), r#"[{"claim":"Profile-guided builds never autotune","falsifiability":"falsifiable","claim_date":null,"suggested_pin":null}]"#).unwrap();
    let o = falsify(&["validate", "--run-dir", runp], corpus_s, wiki_s);
    assert!(o.status.success());
    let id = String::from_utf8_lossy(&o.stdout)
        .split_whitespace()
        .next()
        .unwrap()
        .to_string();

    // (A) profile-guided/autotune live ONLY in sources/ + comparisons/ → wiki CANON is silent → verified
    let ok = format!(
        r#"[{{"claim_id":"{id}","author":"wiki canon","map_fragments":[],"contradictions":[],
        "silence":{{"author":"wiki canon","scope":"wiki","terms_searched":["profile-guided","autotune"],"mechanism_checked":true}}}}]"#
    );
    fs::write(run.join("audits.json"), ok).unwrap();
    let o = falsify(&["verify-evidence", "--run-dir", runp], corpus_s, wiki_s);
    assert!(
        o.status.success(),
        "wiki silence should verify (excluded dirs ignored):\n{}",
        String::from_utf8_lossy(&o.stdout)
    );
    assert!(
        String::from_utf8_lossy(&o.stdout).contains("wiki file"),
        "should report wiki scope: {}",
        String::from_utf8_lossy(&o.stdout)
    );
    let aud = fs::read_to_string(run.join("audits.json")).unwrap();
    assert!(
        aud.contains("concepts/perf.md"),
        "wiki scope must include the canon concept file"
    );
    assert!(
        !aud.contains("sources/thread.md"),
        "sources/ must be excluded from wiki canon scope"
    );
    assert!(
        !aud.contains("comparisons/old.md"),
        "comparisons/ must be excluded from wiki canon scope"
    );

    // (B) a term present in the canon (concepts/perf.md) → absence REFUTED
    let bad = format!(
        r#"[{{"claim_id":"{id}","author":"wiki canon","map_fragments":[],"contradictions":[],
        "silence":{{"author":"wiki canon","scope":"wiki","terms_searched":["branch"],"mechanism_checked":true}}}}]"#
    );
    fs::write(run.join("audits.json"), bad).unwrap();
    let o = falsify(&["verify-evidence", "--run-dir", runp], corpus_s, wiki_s);
    assert!(
        !o.status.success(),
        "a term present in the wiki canon must refute the silence"
    );
    assert!(String::from_utf8_lossy(&o.stdout).contains("REFUTED"));

    // (C) SELF-REFUTATION GUARD: a term living ONLY inside a falsify-fenced block is stripped from
    //     the canon view → the canon is still silent → the silence VERIFIES. falsify never audits
    //     its own writing.
    fs::write(
        wiki.join("concepts/perf.md"),
        "Performance depends on cache locality.\n\n<!-- falsify:begin topic=old created=2026-06-01 -->\n## Falsified: Old\nThis section mentions profile-guided and autotuning heavily.\n<!-- falsify:end topic=old -->\n",
    )
    .unwrap();
    let c = format!(
        r#"[{{"claim_id":"{id}","author":"wiki canon","map_fragments":[],"contradictions":[],
        "silence":{{"author":"wiki canon","scope":"wiki","terms_searched":["profile-guided","autotuning"],"mechanism_checked":true}}}}]"#
    );
    fs::write(run.join("audits.json"), c).unwrap();
    let o = falsify(&["verify-evidence", "--run-dir", runp], corpus_s, wiki_s);
    assert!(
        o.status.success(),
        "a term only inside a falsify block must be stripped → canon still silent:\n{}",
        String::from_utf8_lossy(&o.stdout)
    );
}

/// The presence gate reads the CANON VIEW: a pin may not validate against text that exists ONLY
/// inside a falsify block (falsify's own rendered quote) — only against real canon. Closes the
/// circular-validation hole opened by writing verdicts inline.
#[test]
fn pin_cannot_validate_against_falsify_own_block() {
    let base = PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join("e2e-selfpin");
    let _ = fs::remove_dir_all(&base);

    let corpus = base.join("corpus");
    fs::create_dir_all(corpus.join("a/books/txt")).unwrap();
    fs::write(corpus.join("a/books/txt/b.txt"), "unused.\n").unwrap();

    let wiki = base.join("wiki");
    fs::create_dir_all(wiki.join("concepts")).unwrap();
    let pg = wiki.join("concepts/page.md");
    // the unique phrase exists in the file, but ONLY inside a falsify block
    fs::write(
        &pg,
        "# Page\n\nReal canon: registers matter.\n\n<!-- falsify:begin topic=old created=2026-06-01 -->\n## Falsified: Old\nThe phrase xylophone-marker appears only inside this block.\n<!-- falsify:end topic=old -->\n",
    )
    .unwrap();
    let pg_s = pg.to_str().unwrap().to_string();

    let source = base.join("source.md");
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
            "2026-06-14",
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

    // a pin to text living ONLY in the block → must FAIL (canon view strips it)
    let audits = format!(
        r#"[{{"claim_id":"{id}","author":"a","map_fragments":[{{"person":"P","source_ref":"r","source_path":"{pg_s}","quote":"xylophone-marker appears only inside this block","kind":"book","gloss":null}}],"contradictions":[],"silence":null}}]"#
    );
    fs::write(run.join("audits.json"), audits).unwrap();
    let o = falsify(&["verify-pins", "--run-dir", runp], corpus_s, wiki_s);
    assert!(
        !o.status.success(),
        "a pin matching only falsify's own block must FAIL the presence gate"
    );

    // control: a pin to REAL canon on the same page passes
    let audits = format!(
        r#"[{{"claim_id":"{id}","author":"a","map_fragments":[{{"person":"P","source_ref":"r","source_path":"{pg_s}","quote":"registers matter","kind":"book","gloss":null}}],"contradictions":[],"silence":null}}]"#
    );
    fs::write(run.join("audits.json"), audits).unwrap();
    let o = falsify(&["verify-pins", "--run-dir", runp], corpus_s, wiki_s);
    assert!(
        o.status.success(),
        "a pin to real canon must still pass: {}",
        String::from_utf8_lossy(&o.stdout)
    );
}

/// Inline persist over WIKI pins: the synthesis lands on the operator's `--page` (auto-created),
/// every other contributing wiki page gets a backlink mark, and writing those falsify blocks into
/// frozen canon is NOT seen as drift (the freeze + re-check run over the canon view).
#[test]
fn inline_marks_and_frozen_canon_not_drift() {
    let base = PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join("e2e-marks");
    let _ = fs::remove_dir_all(&base);

    let corpus = base.join("corpus");
    fs::create_dir_all(corpus.join("testauthor/books/txt")).unwrap();
    fs::write(
        corpus.join("testauthor/books/txt/book.txt"),
        "unused book corpus.\n",
    )
    .unwrap();

    let wiki = base.join("wiki");
    fs::create_dir_all(wiki.join("concepts")).unwrap();
    // a wiki canon page that holds the pinned quote (the pin SOURCE is a wiki page, not a book)
    let evidence = wiki.join("concepts/evidence.md");
    fs::write(&evidence, "---\ntitle: Evidence\ntype: concept\n---\n\n# Evidence\n\nThe hot loop is vectorized by -O3.\n").unwrap();
    let evidence_s = evidence.to_str().unwrap().to_string();
    // the primary --page does NOT exist yet → exercises scaffold/auto-create
    let primary = wiki.join("concepts/compiler-optimization.md");

    let source = base.join("source.md");
    fs::write(&source, "-O3 always makes programs faster.").unwrap();
    let run = base.join("run");
    let (corpus_s, wiki_s) = (corpus.to_str().unwrap(), wiki.to_str().unwrap());
    let runp = run.to_str().unwrap();

    let o = falsify(
        &[
            "new-run",
            "--source",
            source.to_str().unwrap(),
            "--as-of",
            "2026-06-14",
            "--run-dir",
            runp,
        ],
        corpus_s,
        wiki_s,
    );
    assert!(
        o.status.success(),
        "new-run: {}",
        String::from_utf8_lossy(&o.stderr)
    );
    fs::write(run.join("claims.json"), r#"[{"claim":"-O3 always makes programs faster","falsifiability":"falsifiable","claim_date":null,"suggested_pin":null}]"#).unwrap();
    let o = falsify(&["validate", "--run-dir", runp], corpus_s, wiki_s);
    assert!(o.status.success());
    let id = String::from_utf8_lossy(&o.stdout)
        .split_whitespace()
        .next()
        .unwrap()
        .to_string();

    let audits = format!(
        r#"[{{"claim_id":"{id}","author":"testauthor",
        "map_fragments":[{{"person":"Test Author","source_ref":"evidence","source_path":"{evidence_s}","quote":"The hot loop is vectorized by -O3","kind":"book","gloss":"-O3 vectorizes the hot loop"}}],
        "contradictions":[],"silence":null}}]"#
    );
    fs::write(run.join("audits.json"), audits).unwrap();
    let verdicts = format!(
        r#"[{{"claim_id":"{id}","label":"diverge","confidence":"high","load_bearing_pin":{{"person":"Test Author","source_ref":"evidence","source_path":"{evidence_s}","quote":"vectorized by -O3","kind":"book","gloss":null}},"temporal_flag":null,"rationale":"x"}}]"#
    );
    fs::write(run.join("verdicts.json"), &verdicts).unwrap();

    // verify-evidence freezes the wiki pin source (over its canon view); verify-pins passes.
    let o = falsify(&["verify-evidence", "--run-dir", runp], corpus_s, wiki_s);
    assert!(
        o.status.success(),
        "verify-evidence: {}",
        String::from_utf8_lossy(&o.stdout)
    );
    let o = falsify(&["verify-pins", "--run-dir", runp], corpus_s, wiki_s);
    assert!(
        o.status.success(),
        "verify-pins: {}",
        String::from_utf8_lossy(&o.stdout)
    );

    // persist: synthesis → primary (auto-created); backlink mark → the evidence page.
    let pargs = [
        "persist",
        "--run-dir",
        runp,
        "--page",
        "concepts/compiler-optimization.md",
        "--topic",
        "O3 speedup",
    ];
    let o = persist_apply(&pargs, corpus_s, wiki_s);
    assert!(
        o.status.success(),
        "persist: {}",
        String::from_utf8_lossy(&o.stderr)
    );

    let prim = fs::read_to_string(&primary).unwrap();
    assert!(
        prim.contains("type: comparison"),
        "auto-created page should carry falsify frontmatter"
    );
    assert!(
        prim.contains("# O3 speedup"),
        "auto-created page H1 missing"
    );
    assert!(
        prim.contains("<!-- falsify:begin topic=o3-speedup"),
        "topic block missing on primary"
    );

    let ev = fs::read_to_string(&evidence).unwrap();
    assert!(
        ev.contains("The hot loop is vectorized by -O3."),
        "evidence canon must be preserved"
    );
    assert!(
        ev.contains("<!-- falsify:begin mark=o3-speedup"),
        "backlink mark missing on the pin source"
    );
    assert!(ev.contains("**Falsified** re:"), "mark text missing");

    // FROZEN-CANON-NOT-DRIFT: evidence.md was frozen, now carries a mark fence. A second persist
    // must NOT abort — the input-pin re-check reads the canon view (mark stripped) and still matches.
    let o = persist_apply(&pargs, corpus_s, wiki_s);
    assert!(
        o.status.success(),
        "writing a falsify block into frozen canon must not register as drift:\n{}",
        String::from_utf8_lossy(&o.stderr)
    );
    let ev2 = fs::read_to_string(&evidence).unwrap();
    assert_eq!(ev, ev2, "mark write must be idempotent");

    // MALFORMED FENCE GUARD: a hand-mangled unbalanced fence on the primary aborts persist.
    fs::write(
        &primary,
        "# O3 speedup\n\n<!-- falsify:begin topic=o3-speedup created=d -->\nnever closed\n",
    )
    .unwrap();
    let o = falsify(&pargs, corpus_s, wiki_s);
    assert!(
        !o.status.success(),
        "persist must abort on a malformed falsify fence"
    );
    assert!(
        String::from_utf8_lossy(&o.stderr).contains("malformed")
            || String::from_utf8_lossy(&o.stderr).contains("unbalanced"),
        "malformed-fence abort should say so: {}",
        String::from_utf8_lossy(&o.stderr)
    );
}

/// Recursive corpus discovery: an author's books nested under a domain folder
/// (<root>/library/<author>/books/txt) are found without moving anything, so existing wiki pins
/// keep resolving. Proves both that the nested file is enumerated (an absent-term silence verifies
/// over a non-empty scope) and that its content is actually read (a present term refutes silence).
#[test]
fn corpus_discovery_is_recursive() {
    let base = PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join("recursive-corpus");
    let _ = fs::remove_dir_all(&base);

    // author nested a domain folder deep: <corpus>/library/testauthor/books/txt/book.txt
    let corpus = base.join("data");
    let bookdir = corpus.join("library/testauthor/books/txt");
    fs::create_dir_all(&bookdir).unwrap();
    fs::write(
        bookdir.join("book.txt"),
        "Chapter one.\nThe hot loop is vectorized by -O3 in tight numeric code.\n",
    )
    .unwrap();

    let wiki = base.join("wiki");
    fs::create_dir_all(&wiki).unwrap();
    let source = base.join("source.md");
    fs::write(&source, "-O3 always makes programs faster.").unwrap();
    let run = base.join("run");
    let (corpus_s, wiki_s) = (corpus.to_str().unwrap(), wiki.to_str().unwrap());
    let runp = run.to_str().unwrap();

    let o = falsify(
        &[
            "new-run",
            "--source",
            source.to_str().unwrap(),
            "--as-of",
            "2026-06-15",
            "--run-dir",
            runp,
        ],
        corpus_s,
        wiki_s,
    );
    assert!(
        o.status.success(),
        "new-run: {}",
        String::from_utf8_lossy(&o.stderr)
    );
    fs::write(run.join("verdicts.json"), "[]").unwrap();

    // absent term over the NESTED corpus → silence verifies (the file was found: scope non-empty).
    fs::write(
        run.join("audits.json"),
        r#"[{"claim_id":"c1","author":"testauthor","map_fragments":[],"contradictions":[],"silence":{"author":"testauthor","terms_searched":["profile-guided","autotuning"],"mechanism_checked":true}}]"#,
    )
    .unwrap();
    let o = falsify(&["verify-evidence", "--run-dir", runp], corpus_s, wiki_s);
    assert!(
        o.status.success(),
        "nested corpus must be discovered + the absent-term silence verify:\n{}",
        String::from_utf8_lossy(&o.stdout)
    );
    let manifest = fs::read_to_string(run.join("manifest.json")).unwrap();
    assert!(
        manifest.contains("library/testauthor/books/txt/book.txt"),
        "the nested file must be frozen into the manifest"
    );

    // present term over the SAME nested corpus → silence refuted (its content was actually read).
    fs::write(
        run.join("audits.json"),
        r#"[{"claim_id":"c1","author":"testauthor","map_fragments":[],"contradictions":[],"silence":{"author":"testauthor","terms_searched":["vectorized"],"mechanism_checked":true}}]"#,
    )
    .unwrap();
    let o = falsify(&["verify-evidence", "--run-dir", runp], corpus_s, wiki_s);
    assert!(
        !o.status.success(),
        "a present term in the nested corpus must refute the silence"
    );
}
