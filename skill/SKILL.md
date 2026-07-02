---
name: falsify
description: "Theory-falsification engine: decompose a source into falsifiable claims → audit each against the wiki canon (intra-author self-contradiction + cross-author synthesis) → adversarial review → produce source-pinned verdicts that compound back into ~/wiki. Invoke: '/falsify <source-path> [--as-of YYYY-MM-DD]'."
---

# falsify — theory-falsification engine

Decomposes a source document into falsifiable claims, audits each against the canon corpus,
runs an adversarial label review, produces pinned verdicts, and proposes an **inline dispute
block** that compounds back into a canon page in `~/wiki` (plus backlink marks on the other
audited pages). **The LLM proposes; Rust validates and pins. No verdict writes wiki without
operator approval.**

---

> **MAP-DISCIPLINE** (binding for every stage below)
> Every claim bullet in the audit and dispute page MUST be attributed and pinned:
> `Per <Person> ([<ref>](<source_path>)): <gloss> — "<verbatim quote>"`
> The quote is the real pin — greppable, human-verifiable, must exist verbatim in
> `source_path` or `verify-pins` aborts the write. Never paraphrase a pin. Never
> synthesize across sources in the map zone. The agent NEVER writes `## My read`.

---

## Step 0 — preflight

1. **Binary.** Confirm the binary is present:
   ```sh
   command -v falsify || echo "MISSING: run ./install.sh in the falsify repo (or cargo build --release)"
   ```
2. **Source path.** Must be an absolute path to a readable file. Resolve `~` manually.
3. **As-of date.** Default to today's date. Pass explicitly if the source is dated.
4. **Author keys.** The slugified corpus directory names (e.g. `jane-doe`) — each author's books
   live at `<corpus>/…/<author-key>/books/txt` (found recursively). Discover the available keys
   from the wiki canon and the on-disk corpus; don't hard-code a roster.

---

## Step 1 — new-run

```sh
RUN=$(falsify new-run --source <abs-path> --as-of <YYYY-MM-DD>)
echo "Run dir: $RUN"
```

`$RUN` is the ephemeral working directory for all subsequent subcommands. Everything in it is
gitignored. Record the run dir in chat — every subsequent step references it.

---

## Step 2 — claim extraction (temp 0, strict JSON)

Extract every falsifiable claim from the source. Use `temp 0`. Produce `$RUN/claims.json`:

```json
[
  {
    "claim": "<the source's assertion, verbatim or tightly paraphrased>",
    "falsifiability": "falsifiable" | "not_falsifiable",
    "claim_date": "<YYYY-MM-DD or null>",
    "suggested_pin": "<optional hint for the auditor, or null>"
  }
]
```

**In chat (not the file),** list every assertion judged `not_falsifiable` and why — rhetoric,
value judgment, or unfalsifiable? This triage is visible to the operator before the pipeline
proceeds.

Then validate and assign content-addressed ids:

```sh
falsify validate --run-dir "$RUN"
```

This prints `<id>  <claim>` per line. **Record the id for every claim** — you will need them
when writing `audits.json` and `verdicts.json`. Never emit ids yourself; they are assigned by
Rust from the normalizer.

---

## Step 3 — local canon pass (subagent, per claim/author)

Spawn **`subagents/local-auditor.md`** — one invocation per claim × author pair (or
batched per the subagent's instructions). **falsify does NOT search — the subagent discovers
with its own tools.** It:

1. Discovers relevant passages itself — `grep -rin` over the author's book corpus, found
   recursively under the corpus root (`$PLAINBRAIN_DATA`, default `~/data`) at any
   `*/<key>/books/txt/` (so corpora may nest under domain folders) — with many phrasings + a
   `wiki-query` mechanism pass, then OPENS the files and forms verbatim pins by **copying** exact
   text from the source, never recalled text.
2. Surfaces contradiction pairs: two pins from the same author that assert incompatible
   positions.
3. Proposes `UNADDRESSED` **only when BOTH** (a) its own lexical sweep is empty across every
   term AND (b) a mechanism-level `wiki-query` pass finds no oblique engagement. It supplies only
   `{author, terms_searched, mechanism_checked}` — falsify computes and validates the rest in Step 6.
4. Writes `$RUN/audits.json` — array of `Audit` objects (fields: `claim_id`, `author`,
   `map_fragments`, `contradictions`, `silence`).

The auditor subagent is isolated — wiki-drilling noise never reaches the main thread.

---

## Step 4 — adversarial pass

Spawn **`subagents/adversarial.md`** with the compact bundle:
`claims.json` + `audits.json` + the per-claim proposed labels.

The adversarial subagent audits:
- **Pin-vs-label fit:** does each verbatim pin actually support its proposed label, or is it
  cherry-picked / out of context?
- **Fairness:** is each side represented proportionally, or does framing bias the synthesis?
- **Understated agreement:** where do the sources agree more than the labels imply?
- **Missed self-contradiction:** are there contradiction pairs the local-auditor subagent
  missed?

It returns a structured critique. Fold the findings into the verdict step.

---

## Step 5 — verdict (temp 0, fixed rubric)

Write `$RUN/verdicts.json` using the adversarial critique plus the audits. One `Verdict`
object per claim:

```json
{
  "claim_id": "<id from Step 2>",
  "label": "match" | "diverge" | "refuted" | "nei" | "not_falsifiable",
  "confidence": "high" | "medium" | "low",
  "load_bearing_pin": { <Pin object or null> },
  "temporal_flag": "<string if evidence postdates the claim, else null>",
  "votes": [],
  "rationale": "<why this label; reference the adversarial critique where relevant>"
}
```

**Rubric rules:**
- `not_falsifiable` claims are routed out of the rubric — they get no `match`/`refuted` call.
- `nei` is first-class. Never force `match` or `refuted` when the evidence is insufficient.
- Set `temporal_flag` when evidence postdates the claim date — flag only, never suppress.
- `votes` is empty in v1 (schema-forward for v2 multi-vote).

---

## Step 6 — verify-evidence (validate silence + freeze the inputs)

```sh
falsify verify-evidence --run-dir "$RUN"
```

The ABSENCE half of the gate, plus input-pinning. For every silence flag it re-greps the terms
across the scope it enumerates and **fails (exit 1) if any occurrence is found** — a refuted
silence claim must be fixed (drop the flag, add the missed pin), never forced. A silence flag's
`scope` is either `author_books` (the author's full `books/txt`) or `wiki` (the compiled canon —
`concepts/`/`entities/`/hubs, with `sources/` + `comparisons/` excluded). Either way falsify owns the
file set AND audits only the **canon view** — it strips its own `<!-- falsify:* -->` blocks before
grepping, so a term that appears only inside a prior verdict falsify wrote never refutes (or, in a
pin, never self-validates). On success it writes the validated scope back into the silence flags and
**freezes every load-bearing file** (pin sources + silence scopes, including verdict pins) into the
manifest, hashing the canon view. Run it *after* verdicts so the frozen slice is complete. Discovery
is the subagent's; verification is falsify's.

## Step 7 — verify-pins (presence gate; must pass before persist)

```sh
falsify verify-pins --run-dir "$RUN"
```

Exit 1 means a pin quote does not exist verbatim (case-sensitive) in its `source_path`. Fix the
offending pin in `audits.json` or `verdicts.json` — go back to the source file and copy the exact
text. **Never paraphrase to pass the gate.** Re-run until exit 0. (Presence is case-sensitive here;
absence in Step 6 is case-insensitive — two questions, two correct policies.)

---

## Step 8 — persist (propose-diff, operator approval)

```sh
falsify persist --run-dir "$RUN" --page "<canon-page>" --topic "<Topic Label>"
```

`--page` is the canon page that hosts the synthesis — absolute, `~/`, or **wiki-relative**
(e.g. `concepts/<topic>.md`). It must resolve **inside the wiki root** — a `..` component or an
absolute path outside the wiki is refused (v1 writes only inside `~/wiki`). Pick the claim's
canonical home; it is auto-created (minimal `type: comparison` frontmatter) if it does not exist. persist writes the synthesis **inline** into
that page as a topic-keyed `<!-- falsify:begin topic=<slug> … -->` block, and drops a one-line
backlink **mark** (`<!-- falsify:begin mark=<slug> … -->`) on every OTHER audited wiki page that
contributed a pin. **Everything outside falsify's fences is preserved** — host frontmatter, canon
prose, other topics' blocks, and the operator-only `### My read`.

Before rendering, persist re-checks the frozen input slice (Step 6 must have run) and **aborts if
any load-bearing file drifted** since the audit — a verdict may only be written against the exact
canon bytes it was judged on. It compares the **canon view** (falsify's own fenced blocks stripped),
so writing a block into a frozen canon page is *not* drift, but an operator edit to real canon *is*.
It then writes `<page>.proposed` (one per edited file) — no overwrite of `~/wiki`. Show the operator
each proposed diff. On approval:

```sh
falsify persist --run-dir "$RUN" --page "<canon-page>" --topic "<Topic Label>" --apply
```

`--apply` installs **only** a reviewed proposal: it regenerates the content and refuses unless a
`<page>.proposed` exists and is byte-identical (so a one-shot `--apply` that skipped the propose
step, or a proposal gone stale because the run artifacts or the host page changed since, aborts —
re-run without `--apply`, review, then `--apply`).

Re-running a topic UPDATES its block in place (idempotent splice) — it never forks a new file. The
block follows the wiki's dispute schema as `###` subsections: `### Positions`, `### Nature of the
disagreement`, `### Status` table, `### UNADDRESSED`, `### My read`. `### My read` is operator-only
and marker-protected — `persist` never writes into it. A malformed (unbalanced) falsify fence on any
target aborts the write rather than risk clobbering canon.

---

## Step 9 — coverage report

```sh
falsify coverage --run-dir "$RUN"
```

Prints the gap report: authors audited vs **discoverable** book corpora (with the gap named),
load-bearing files frozen, which claims are UNADDRESSED, which are NEI, which have no audit.
**Surface this to the operator — never hide coverage gaps.** There is no completeness fraction:
the audited scope is LLM-selected, so it is reported as an honest count, not a percentage.
Coverage is computed from the run artifacts; it is never stored.

---

## Quality levers (optional — raise rigor on high-stakes runs)

These trade tokens for confidence. Use on contested or high-stakes claims; skip on quick passes.

- **Per-claim multi-vote verdict.** Instead of one verdict, spawn N independent judges per claim
  (e.g. the main model + `grok-ro`, or different lenses); record each in the `votes` array
  (`{"voter": "...", "label": "..."}`). Call the label only on a majority; otherwise `nei`.
  Agreement level → confidence. Turns LLM variance into a measured number, not a coin flip.
- **Ensemble claim extraction.** Run extraction K times (or with K segmentations); union the
  claims; `falsify validate` assigns ids and `persist`'s near-dup detector flags reworded
  duplicates for merge. Note which claims appeared in all K passes vs only some — extraction
  stability, made visible.
- **Mechanical contradiction pre-filter.** `falsify suggest-contradictions --run-dir "$RUN"`
  flags pin pairs that assert disjoint NUMBERS about a shared subject — high-precision and cheap.
  Feed the suggestions to the local-auditor / adversarial pass to confirm. Semantic / polarity
  contradictions stay the LLM's job (a mechanical polarity detector is too noisy to trust).

## Hard rules

- **The LLM proposes; Rust validates and pins — it never discovers.** Discovery is the
  orchestrator's own tools (grep, `wiki-query`, `/recon`, reads); every quote is copied from the
  source file the subagent read. The verbatim-pin gate guards fabrication (presence) and
  `verify-evidence` guards false silence (absence) — neither makes labels right. Label-correctness
  is the adversarial pass's job.
- **v1 is wiki-only, books-only.** No external dataset (`--with-dataset` is v2). Transcripts
  are a fast-follow; book-grade pins only in v1.
- **UNADDRESSED is a flag, never a verdict, never lowers a tier.** It fires only on dual-gate
  silence (lexical AND mechanism-level non-engagement). The lexical half is *verified* by
  `verify-evidence` — it tries to falsify the absence across the author's full corpus; the LLM's
  word alone never stands. Absence is corroborated, never machine-certified (it can only ever
  exhibit a counterexample). The validated envelope (terms, scope, replay hash) is persisted so the
  flag is re-runnable.
- **The agent never writes `### My read`.** That section is operator-only and marker-protected
  by `persist` (preserved across runs, inside the falsify block).
- **falsify writes only inside its own fences.** `persist` compounds the synthesis INTO a canon page
  as a `<!-- falsify:begin topic=… -->` block + backlink marks on other audited pages; it never
  touches canon prose, host frontmatter, other topics' blocks, or `### My read`. It never audits its
  own writing — `canon_bytes` strips every falsify block before any grep/hash/pin, so a verdict can
  neither refute nor self-validate. `comparisons/` is retired.
- **No blind overwrite.** `persist` always proposes a diff (`<page>.proposed`, one per edited file).
  The operator approves before `--apply`. A malformed falsify fence aborts the write.
- **NEI is always available.** Never force a call when evidence is insufficient.
