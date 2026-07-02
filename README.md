# falsify — theory-falsification engine

Decompose a source document into falsifiable claims, audit each against the trust-tiered canon in
your `~/wiki` (intra-author self-contradiction + cross-author synthesis), run an adversarial
cross-vendor review, and persist ranked, **source-pinned** verdicts — MATCH / DIVERGE / REFUTED /
NEI — that compound back into the wiki as inline, citable blocks.

It is two pieces:

1. **`falsify`** — a small, dependency-light Rust binary: the deterministic substrate. It owns
   strict claim-schema validation and content-addressed claim IDs, a **verbatim-pin existence
   gate** (every quote a verdict cites must exist verbatim in its named source, or the write
   aborts), an **absence corroborator** (`verify-evidence` re-greps a declared silence claim's
   terms across the scope it enumerates; any hit fails the run), **input-pinning** (it
   content-hashes the source bytes and every canon slice consulted, and refuses to persist if any
   drifted), a coverage report, and it is the **sole writer** of falsify-fenced blocks into canon
   pages (a snapshot per topic — latest run wins, guarded so it can't silently drop a recorded
   claim — near-dup detection, propose-diff, idempotent splice). It is golden-tested:
   same inputs → same outputs.
2. **A Claude Code skill (`/falsify`)** — the reasoning layer. It extracts the claims, spawns a
   subagent to drill the wiki canon for self-contradictions and silences, runs an adversarial pass
   on pin-vs-label fit, renders per-claim verdicts under a fixed rubric, and drives the binary to
   persist. The judgment and synthesis run on **your** Claude Code subscription.

Two design principles hold the whole thing together:

- **The LLM proposes; Rust validates and pins.** No verdict touches the wiki without passing a
  deterministic verbatim-pin gate — and without your approval of the proposed diff.
- **falsify verifies, it does not discover.** It gathers no evidence of its own. The reasoning
  layer gathers evidence with the existing kit and *declares* what it used; Rust only re-checks
  that declaration. Absence can be corroborated but never certified.

---

## ⚠️ Read this first — what this actually is

**This is a highly personalized tool I built for my own daily workflow. It is not a polished
product.** It assumes you work the way I do: inside Claude Code, from a terminal, with a markdown +
git knowledge wiki you curate by hand. It makes opinionated choices and expects you to read the
code and bend it to your setup rather than configure it through a UI.

Concretely, it **requires [Claude Code](https://claude.com/claude-code)** — the entire reasoning
layer (claim extraction, the canon audit, the adversarial pass, the verdict) *is* a Claude Code
skill. Without Claude Code you have a deterministic validation/persistence binary and nothing to
drive it. It also assumes a **plainbrain-style `~/wiki`**: a corpus of trust-tiered canon to audit
against and compound into. Point it at an empty directory and there is nothing to falsify.

---

## How it fits with recon and plainbrain

falsify is the third piece of a personal knowledge stack. Each part does one job, and all three
share one wiki:

- **[plainbrain](https://github.com/crodorg/plainbrain)** — *declarative memory.* A markdown + git
  wiki of trust-tiered canon (`~/wiki`), every write operator-approved. This is the corpus falsify
  audits *against*, and the place verdicts compound *back into* — written inline as
  `<!-- falsify:begin … -->` blocks on the relevant canon page. Without a plainbrain-style wiki,
  falsify has nothing to work on.
- **[recon](https://github.com/crodorg/recon)** — *retrieval.* A terminal deep-research engine
  that fans out across the web (Perplexity), X/social (Grok), and free sources, then reads and
  verifies locally. recon brings the world *in*, cited. falsify deliberately does no retrieval of
  its own; its optional external-literature pass (`--with-dataset`) delegates to recon.
- **falsify** — *adversarial audit.* It takes claims and stress-tests them against the canon: where
  an author contradicts himself, where authors diverge, what the evidence actually supports, what
  is conspicuously unaddressed. The output is a ranked, source-pinned verdict that becomes new
  canon.

The loop: **recon** gathers evidence → **plainbrain** holds what you trust → **falsify** tests
claims against it and writes the verdict back. Retrieval, memory, and falsification — three small
terminal tools, one wiki. All three are personal tools, shared as-is.

## Requirements

- **Claude Code** (required — the `/falsify` skill is the reasoning layer).
- **Rust toolchain** (`cargo`) to build the binary.
- **A markdown wiki** at `~/wiki` — the canon corpus. Override with `FALSIFY_WIKI_ROOT`, or set
  `$PLAINBRAIN_WIKI` if you use [plainbrain](https://github.com/crodorg/plainbrain). The skill
  expects a plainbrain-style layout (`concepts/`, `entities/`, hubs; `sources/` excluded from the
  canon scan).
- *Optional:* a local `grok` CLI for the cross-vendor adversarial pass; the `/recon` skill for the
  v2 external-literature pass.

## Install

```sh
git clone https://github.com/crodorg/falsify && cd falsify
./install.sh
```

`install.sh` builds the release binary, symlinks it to `~/.local/bin/falsify`, and installs the
skill into `~/.claude/skills/falsify/` (honoring `CLAUDE_CONFIG_DIR`). Make sure `~/.local/bin` is
on your `PATH`. Then, in Claude Code:

```
/falsify <source-path> [--as-of YYYY-MM-DD]
```

## How it works

A source document moves through five stages. The LLM stages run in your Claude Code session; the
deterministic stages run in the binary.

1. **Claim extraction.** The source is decomposed into falsifiable claims under a strict JSON
   schema; assertions judged *non*-falsifiable are recorded separately, never silently dropped.
   Rust assigns each claim a content-addressed ID from its normalized text.
2. **Local canon audit.** A subagent drills your wiki for each claim — verbatim self-contradictions
   within an author's corpus, divergences across authors, and conspicuous silences. Every position
   is pinned to a verbatim quote in a named source.
3. **Adversarial pass.** A cross-vendor review (Grok — handed a self-contained bundle staged into
   the working directory so the sandboxed reviewer can actually read it; falls back to a same-vendor
   pass, labeled as such, if none is available) checks whether each pin actually justifies its
   proposed label or is cherry-picked / out of context. This defends label-correctness, which the
   pin-gate alone cannot.
4. **Verdict.** A fixed rubric assigns each claim MATCH / DIVERGE / REFUTED / NEI with a confidence
   and a load-bearing pin. A non-falsifiable claim is dispositioned out of the rubric, not scored;
   a refutation whose evidence postdates the claim is flagged, never silently applied.
5. **Persist.** `falsify persist --page <canon-page>` writes the synthesis inline into the named
   wiki page as a `<!-- falsify:begin topic=… -->` block, drops one-line backlink marks on the
   other audited pages, and emits a **proposed diff** (`<page>.proposed`) for you to approve —
   never a blind overwrite. Re-running a topic is a convergent, zero-diff splice. The block is a
   **snapshot** — regenerated from the current run, so a re-persist replaces it wholesale (latest
   run wins); a `claims=` fence attribute lets `persist` warn if a re-run would drop a claim the
   prior block recorded. Machine-readable merge-by-id across runs is v2.

## What the binary guarantees (and what it doesn't)

The determinism is scoped honestly. The Rust substrate is reproducible; the LLM judgment is
auditable and pinned, not bit-reproducible — and the README says so plainly rather than claiming
otherwise.

- **Deterministic, golden-tested:** pin existence-verification, the declared-slice re-grep,
  claim-ID normalization, cross-reference + schema integrity (`deny_unknown_fields`), the
  falsifiability gate, snapshot persist, coverage, near-dup detection. Same inputs → same outputs.
  (The *temporal flag* is an LLM-supplied annotation the binary only renders — not a computed gate —
  so it is not on this list.)
- **The pin-gate guards fabrication, not correctness.** Every quote a verdict cites must exist
  verbatim in its named source or the write aborts — so the LLM cannot invent an exhibit. It does
  *not* make the label right; a real quote can be cherry-picked under the wrong label. That is
  defended separately, by the adversarial pin-vs-label audit and the first-class NEI verdict.
- **Input-pinning is enforced, not just recorded.** Each run content-hashes the source bytes and
  every canon slice actually consulted; `persist` re-checks those hashes and aborts if anything
  drifted since the run — a verdict can never silently rest on canon that changed underneath it.
  The run's own audits and verdicts are frozen the same way at `verify-evidence`, so a post-gate
  edit that would slip an unverified silence flag or a swapped pin past the checks aborts `persist`
  too. And `--apply` installs only the reviewed `.proposed` byte-for-byte: a one-shot apply that
  skipped review, or a proposal gone stale, is refused — the bytes you approved are the bytes written.
- **Silence is corroborated, never confirmed.** Absence is a universal claim Rust cannot certify.
  An UNADDRESSED flag fires only when a declared lexical slice re-greps empty *and* a
  mechanism-level pass shows non-engagement. It is a flag, never an autonomous verdict.
- **falsify never audits its own writing.** Before every grep, hash, and pin check, the canon view
  strips falsify's own blocks — so a verdict can neither refute a silence claim nor self-validate a
  pin, and re-persisting is never mistaken for canon drift.

## Limitations (honest list)

- Requires Claude Code; there is no standalone CLI that produces verdicts.
- Requires a curated markdown wiki — it audits what you already trust; it does not build the corpus.
- v1 runs the local-canon and adversarial passes on a single source into one topic, and each topic
  block is a per-run **snapshot** (re-persisting replaces it — guarded against silent claim loss).
  The external-literature pass (`--with-dataset`, via recon), per-claim multi-vote council, a
  cross-topic claims ledger, and machine-readable **merge-by-id across runs** are v2.
- The trust tiers and canon layout reflect my wiki's conventions; adapt them to yours.
- This is a personal tool shared as-is — issues and PRs welcome, but support is best-effort.

## License

[MIT](LICENSE).
