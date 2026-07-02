//! falsify — theory-falsification engine: the deterministic substrate. The /falsify
//! skill (LLM orchestrator) drives these subcommands; each is a deterministic function
//! of its inputs + the input-pinned corpus. The LLM proposes; Rust validates and pins.

mod contradict;
mod coverage;
mod fence;
mod model;
mod persist;
mod store;
mod verify;
mod verify_evidence;

use std::path::PathBuf;

use anyhow::{Context, Result};
use clap::{Parser, Subcommand};

use model::*;

#[derive(Parser)]
#[command(
    name = "falsify",
    version,
    about = "Theory-falsification engine: deterministic substrate"
)]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    /// Create a run dir + input-pinned manifest (hashes the source document).
    NewRun {
        /// The source document under examination.
        #[arg(long)]
        source: String,
        /// As-of date (YYYY-MM-DD). The orchestrator passes today; the binary can't read intent.
        #[arg(long)]
        as_of: String,
        /// Override the run dir (else ~/.local/share/falsify/runs/<ts>/).
        #[arg(long)]
        run_dir: Option<String>,
    },
    /// Validate claims.json + assign content-addressed ids (written back + printed).
    /// The orchestrator runs this to learn each claim's id before writing audits/verdicts.
    Validate {
        #[arg(long)]
        run_dir: String,
    },
    /// Validate the LLM's DECLARED evidence: attempt to falsify every silence claim over the
    /// author's full book corpus (it does not discover), and freeze the run's input slice
    /// (pin sources + silence scopes) into the manifest. Exit 1 if any silence claim is refuted.
    VerifyEvidence {
        #[arg(long)]
        run_dir: String,
    },
    /// Verify every pin's verbatim quote exists in its source. Exit 1 on any failure.
    VerifyPins {
        #[arg(long)]
        run_dir: String,
    },
    /// Render the synthesis INLINE into a canon page as a falsify-fenced block + backlink marks
    /// on other audited wiki pages (propose-diff; sole writer of falsify blocks). --apply installs.
    Persist {
        #[arg(long)]
        run_dir: String,
        /// The primary canon page to host the synthesis (absolute, ~/, or wiki-relative). Created
        /// if it does not exist; an existing page keeps everything outside the falsify block.
        #[arg(long)]
        page: String,
        #[arg(long)]
        topic: String,
        #[arg(long)]
        apply: bool,
    },
    /// Computed coverage/gap report from the run's audits, verdicts, and frozen slice.
    Coverage {
        #[arg(long)]
        run_dir: String,
    },
    /// Mechanical numeric-conflict pre-filter over audit pins (suggestions to confirm).
    SuggestContradictions {
        #[arg(long)]
        run_dir: String,
    },
}

fn main() -> Result<()> {
    match Cli::parse().command {
        Command::NewRun {
            source,
            as_of,
            run_dir,
        } => new_run(&source, &as_of, run_dir),

        Command::Validate { run_dir } => {
            let run = PathBuf::from(&run_dir);
            let claims = store::load_claims(&run)?; // assigns content-addressed ids
            store::write_json(&store::claims_path(&run), &claims)?; // write back with ids

            // If audits/verdicts already exist (a re-validate), enforce cross-ref integrity + the
            // falsifiability gate now too — a dangling id or mis-routed label should fail as early
            // as possible, not silently ride to persist. Both no-op before those files exist.
            let audits = store::load_audits(&run)?;
            let verdicts = store::load_verdicts(&run)?;
            check_claim_refs(&claims, &audits, &verdicts)?;
            check_falsifiability_gate(&claims, &verdicts)?;
            for c in &claims {
                let g: String = c.claim.chars().take(70).collect();
                println!("{}  {}", c.id, g);
            }
            Ok(())
        }

        Command::VerifyEvidence { run_dir } => {
            let report = verify_evidence::verify_evidence(&PathBuf::from(&run_dir))?;
            print!("{}", report.render());
            if report.failed > 0 {
                std::process::exit(1);
            }
            Ok(())
        }

        Command::VerifyPins { run_dir } => {
            let report = verify::verify_pins(&PathBuf::from(&run_dir))?;
            print!("{}", report.render());
            if report.failed > 0 {
                std::process::exit(1);
            }
            Ok(())
        }

        Command::Persist {
            run_dir,
            page,
            topic,
            apply,
        } => persist::persist(&PathBuf::from(&run_dir), &page, &topic, apply),

        Command::Coverage { run_dir } => {
            print!("{}", coverage::coverage(&PathBuf::from(&run_dir))?);
            Ok(())
        }

        Command::SuggestContradictions { run_dir } => {
            let audits = store::load_audits(&PathBuf::from(&run_dir))?;
            let mut by_author: std::collections::BTreeMap<String, Vec<Pin>> =
                std::collections::BTreeMap::new();
            for a in audits {
                by_author
                    .entry(a.author)
                    .or_default()
                    .extend(a.map_fragments);
            }
            for (author, pins) in &by_author {
                for s in contradict::suggest(pins) {
                    let qa: String = pins[s.a].quote.chars().take(50).collect();
                    let qb: String = pins[s.b].quote.chars().take(50).collect();
                    println!("[{author}] {} | a: \"{qa}\" | b: \"{qb}\"", s.reason);
                }
            }
            Ok(())
        }
    }
}

fn new_run(source: &str, as_of: &str, run_dir: Option<String>) -> Result<()> {
    let src_path = store::expand_tilde(source);
    let source_hash = store::file_hash(&src_path)
        .with_context(|| format!("source not found: {}", src_path.display()))?;
    let created = chrono::Local::now()
        .format("%Y-%m-%dT%H:%M:%S%z")
        .to_string();
    let ts = chrono::Local::now().format("%Y%m%d-%H%M%S").to_string();
    let run = run_dir
        .map(PathBuf::from)
        .unwrap_or_else(|| store::runs_root().join(&ts));
    std::fs::create_dir_all(&run)?;
    let manifest = RunManifest {
        run_id: ts,
        created,
        schema_version: SCHEMA_VERSION,
        as_of: as_of.to_string(),
        source: source_hash,
        corpus_touched: vec![],
        artifacts: vec![],
        model_ids: vec![],
        prompt_hashes: vec![],
    };
    store::save_manifest(&run, &manifest)?;
    println!("{}", run.display());
    Ok(())
}
