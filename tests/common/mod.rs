//! Shared harness for the integration test binaries: drive the real CLI against an
//! env-overridden fixture corpus + wiki. Each `tests/*.rs` binary declares `mod common;`.

#![allow(dead_code)] // not every test binary uses every helper

use std::process::{Command, Output};

pub const BIN: &str = env!("CARGO_BIN_EXE_falsify");

pub fn falsify(args: &[&str], corpus: &str, wiki: &str) -> Output {
    Command::new(BIN)
        .args(args)
        .env("FALSIFY_CORPUS_ROOT", corpus)
        .env("FALSIFY_WIKI_ROOT", wiki)
        .output()
        .expect("run falsify")
}

/// Propose then --apply. A3: `--apply` installs only a reviewed proposal byte-identical to the
/// freshly-regenerated content, so a persist that expects to WRITE must propose first. `base` is the
/// persist argv WITHOUT `--apply`; returns the --apply Output.
pub fn persist_apply(base: &[&str], corpus: &str, wiki: &str) -> Output {
    let p = falsify(base, corpus, wiki); // propose: writes <page>.proposed
    assert!(
        p.status.success(),
        "propose step failed: {}",
        String::from_utf8_lossy(&p.stderr)
    );
    falsify(&[base, &["--apply"]].concat(), corpus, wiki) // apply: installs the reviewed proposal
}
