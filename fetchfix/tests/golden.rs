//! Golden tests: byte-exact behavior lock for the fetchfix CLI.
//!
//! Cases 1-2 were captured from the pre-port (std-only) binary — the port must
//! reproduce them byte-identically (stdout, stderr, exit code). Case 3 is the one
//! deliberate capability the port ADDS (verify-core reflow recovery): a quote that
//! line-wraps differently than the source used to be flagged UNLOCATABLE; now it
//! is located and re-anchored. Fixture paths use an @FIX@ placeholder so goldens
//! stay hermetic.

use std::path::Path;
use std::process::Command;

// File-arg invocation, not a piped stdin: tarpaulin's ptrace engine (--follow-exec)
// segfaults tracing children fed via stdin pipes, and falsify's own subprocess tests
// use arg mode throughout. The stdin branch has its own test below (shell redirect).
fn run_case(n: u32) {
    let fix = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures");
    let fixs = fix.to_str().unwrap();
    let read = |name: String| std::fs::read_to_string(fix.join(&name)).unwrap();
    let input = read(format!("input-{n}.txt")).replace("@FIX@", fixs);
    let tmp = std::env::temp_dir().join(format!("fetchfix-golden-{n}.txt"));
    std::fs::write(&tmp, &input).unwrap();
    let out = Command::new(env!("CARGO_BIN_EXE_fetchfix"))
        .arg(&tmp)
        .output()
        .unwrap();
    std::fs::remove_file(&tmp).ok();

    let stdout = String::from_utf8(out.stdout)
        .unwrap()
        .replace(fixs, "@FIX@");
    let stderr = String::from_utf8(out.stderr)
        .unwrap()
        .replace(fixs, "@FIX@");
    assert_eq!(stdout, read(format!("golden-{n}.out")), "stdout, case {n}");
    assert_eq!(stderr, read(format!("golden-{n}.err")), "stderr, case {n}");
    let code: i32 = read(format!("golden-{n}.code")).trim().parse().unwrap();
    assert_eq!(out.status.code(), Some(code), "exit code, case {n}");
}

#[test]
fn golden_happy_paths() {
    // ok / anchor-corrected / cosmetic-slip->source-bytes / split-multibullet /
    // repaired-truncated / multiline-span — exit 0
    run_case(1);
}

#[test]
fn golden_fabrication_and_nofile() {
    // unlocatable fabrication + missing file — model text kept, flagged, exit 2
    run_case(2);
}

#[test]
fn reflow_recovery_new_capability() {
    // NEW vs pre-port: body reflowed across source lines is located, not flagged
    run_case(3);
}

#[test]
fn golden_quote_partial_and_lastline_fallback() {
    // tier-2 *"quote"*-locate despite a slip elsewhere / tier-3 partial-line
    // contains / multi-line item whose last line mismatches (span collapses to
    // the first line's anchor) — exit 0 (old==new verified at capture)
    run_case(4);
}

#[test]
fn golden_ambiguous_truncation_and_empty_body() {
    // truncated fragment matching TWO source lines — refuse to guess — and an
    // empty block body; both flagged, exit 2 (old==new verified at capture)
    run_case(5);
}

#[test]
fn stdin_mode_matches_file_arg_mode() {
    // shell redirect (not a pipe) so the ptrace coverage engine can follow it
    let fix = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures");
    let fixs = fix.to_str().unwrap();
    let input = std::fs::read_to_string(fix.join("input-1.txt"))
        .unwrap()
        .replace("@FIX@", fixs);
    let tmp = std::env::temp_dir().join("fetchfix-golden-stdinmode.txt");
    std::fs::write(&tmp, &input).unwrap();
    let out = Command::new("sh")
        .arg("-c")
        .arg(format!(
            "exec '{}' < '{}'",
            env!("CARGO_BIN_EXE_fetchfix"),
            tmp.display()
        ))
        .output()
        .unwrap();
    std::fs::remove_file(&tmp).ok();
    let stdout = String::from_utf8(out.stdout)
        .unwrap()
        .replace(fixs, "@FIX@");
    let golden = std::fs::read_to_string(fix.join("golden-1.out")).unwrap();
    assert_eq!(stdout, golden, "stdin mode must equal file-arg mode");
    assert_eq!(out.status.code(), Some(0));
}

#[test]
fn unreadable_input_file_exits_1() {
    let out = Command::new(env!("CARGO_BIN_EXE_fetchfix"))
        .arg("/nonexistent/fetchfix-input.txt")
        .output()
        .unwrap();
    assert_eq!(out.status.code(), Some(1));
    assert!(String::from_utf8(out.stderr)
        .unwrap()
        .starts_with("fetchfix: cannot read"));
}
