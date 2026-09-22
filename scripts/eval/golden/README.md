# Decision wire-format golden

`decision-wire-format.golden` is a **generated test fixture**, not a captured
log. Nothing in it came from a real agent run, and no runtime activity is
recorded here.

**The bytes are the assertion.** Each line is real output from the production
`tracing` layer, and the test's whole question is whether the emitter still
produces exactly this text and whether the scrape still recovers the right
record from it.

| | |
|---|---|
| **Written by** | `packages/agent/tests/decision_marker_golden.rs` |
| **Read by** | `scripts/eval/decision-roundtrip.test.ts` |
| **Regenerate** | `UPDATE_GOLDEN=1 cargo test -p nodespace-agent --test decision_marker_golden` |

After regenerating, review the `git diff` **and** run the TypeScript side
(`bun run test:scripts`) — it asserts what these lines parse back to, and a
deliberate emission change means updating its expectations too. A bare
`cargo test` never writes this file; the env var is read at the write site.

## Why it lives here

The Rust test writes it, but the TypeScript test is what it exists for. A
generated file inside a Rust crate's tree reads as Rust build output, so it
sits next to its consumer instead.

## What it guards

The `[decision ...]` marker format is a contract spanning three files in two
languages: `local_agent::decisions` and `agent_loop.rs` emit it, `scripts/aichat.ts`
scrapes the daemon log into a marker, and `scripts/eval/runner.ts` parses that
marker into a record the eval scores.

Before this fixture, each site tested itself against its own hand-written
string, so all three could drift apart while every test stayed green. That is
not hypothetical — it happened twice:

- a scrape looked for a `scoped tool list` line carrying `selected_tools=` that
  had never existed in the daemon, so every scored turn recorded an empty tool
  list;
- `decision_selected` was matched as a non-whitespace run, silently truncating
  `"Schema Creation"` to `"Schema"` — invisible for tool names and type ids,
  wrong for every skill, and caught by eye rather than by a test.

Change the Rust emission now and the TypeScript test fails, because its input
is the Rust output.

## The nine lines

| # | Pins |
|---|---|
| 1–2 | Ordinary skill and operation decisions |
| 3 | Off-menu — the model named `album`, which retrieval never offered |
| **4** | A candidate named `Company, Sold To` — **the comma that used to split one candidate into two** |
| **5** | A candidate containing `"` |
| **6** | A candidate containing a newline, encoded so the record stays on one line |
| 7 | `selected: null` — tools were offered and none was called |
| 8 | `selected: ""` — a blank name, which must not collapse into `null` |
| 9 | Nothing on offer at all |

Lines 4–6 are the shapes the previous delimited format could not represent.
Line 4 used to parse as three candidates rather than two, which also made the
selection read as off-menu; line 6 would have broken the record in half at the
newline.

Line 8 also carries `off_menu: true`, which is correct rather than incidental:
`""` is a selection, and it is not among the candidates. The eval both fails a
scenario on that flag and counts it into a reported figure, so it is pinned
deliberately.

Every line carries the same fixed timestamp (`2026-09-22T10:00:00.000000Z`) and
the `nodespace_agent` target. Both are normalised by the writer so the file is
byte-stable across runs — a real log would show nine different timestamps.
Neither is parsed by the scrape, which keys on the `"Agent decision:"`
substring and the `decision=` / `decision_payload=` fields. Everything the
contract depends on — field names, field order, quoting, escaping — is genuine
subscriber output.

## Why raw bytes, and not YAML or JSON

This file is expected **output**, not input. The inputs are `DecisionRecord`s
defined in Rust, in `decision_marker_golden.rs`.

Re-encoding the emitted line into a structured format would mean the test
verifies a faithful transcription rather than what the daemon actually writes —
which is the hand-written-fixture problem this fixture was introduced to
remove. A quoting or escaping change would pass a YAML round trip and still
break the real scrape.

The same reasoning applies to the goldens under `packages/agent/goldens/` and
`packages/agent/tests/golden/prompt_assembly/`: snapshot the artifact in its
own form, or you are snapshotting your transcription of it.
