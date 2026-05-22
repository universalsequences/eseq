# Agentic Bubbles in the Patcher

## Concept

Spatial, asynchronous prompt nodes ("bubbles") that live in the patch canvas. Each bubble is a request for a small reusable `defmacro`. Multiple can be in flight at once, resolving independently. The user patches around and through them, builds up variants, and the macros they actually wire in graduate into a personal library.

The aim is **co-discovery, not delegation** — fast enough that the user stays in flow, dumb enough about local context that results stay portable and surprising.

## Lifecycle

1. **Spawn** — `cmd+k` at cursor. A bubble appears at the cursor position with prompt field focused.
2. **Wire (optional, pre-submit)** — User may drag wires into the bubble's input ports and out of its output port before submitting. While unsubmitted, the bubble exposes generic ports that accept anything.
3. **Submit** — Enter sends the request. Bubble enters `pending` state.
4. **Pending** — Bubble becomes a passthrough macro: `(defmacro agentic-wip (x) x)`. Signal flows through any wired path, so the surrounding patch stays audible. Soft pulsing outline indicates work in progress; tiny token counter visible.
5. **Resolve** — Model returns lisp. Parser materializes the macro in place, preserving wires. Outline shifts to "resolved" color (soft glow, no audio cue, no focus steal).
6. **Use** — When the resolved bubble's output wire is connected to anything downstream, the macro is logged to the library with its prompt, signature, and timestamp.
7. **Variant** — A small `another` button on the resolved bubble spawns a sibling next to it with the same prompt + a "different approach" nudge. Originals remain.

## Signature (Hybrid)

At submit time, the bubble's signature is derived as follows:

- **Inputs**: any wires already connected to input ports are fixed inputs to the macro, named by the upstream signal where possible (`f1`, `amp_env`, etc.) or by ordinal (`in1`, `in2`).
- **Output**: assumed singular unless prompt explicitly says otherwise (e.g. "stereo", "L and R").
- **Additional params**: model is free to add macro parameters (`freq`, `decay`, `tone`) which surface as additional ports/knobs on the resolved bubble.

The model is told: *"Produce a `defmacro` named `<slug-of-prompt>` taking these fixed inputs [...] plus any additional control parameters you choose. Output a single signal. Keep it under ~20 lines. Use only stdlib primitives."*

## Context Sent to Model

**Minimal and portable**: prompt + signature + (small, fixed) primer of available dgenlisp primitives. **No patch context.** This is deliberate — macros must work in any patch, not just this one.

## Latency Budget

**Under 2 seconds wall time.** Implies:

- Haiku 4.5, low max_tokens (~400)
- Aggressive system prompt that pre-loads primitives and output format
- Prompt caching on the primer
- Single auto-retry on parse failure (one extra round-trip budgeted into the 2s target; if both attempts return garbage, surface error state)

If the user wants a more elaborate generation later, that's a future "think harder" mode — out of scope for v1.

## Failure Handling

- **Parse fail** or **signature mismatch**: silently retry once, feeding the error back to the model. If still bad, bubble enters `error` state with raw output visible and a "retry" / "edit prompt" affordance.
- **Timeout** (>5s): treat as failure. Same path.
- Errors never auto-resolve into a placeholder that could pass bad signal — the passthrough only persists while the request is genuinely in flight.

## Persistence

- Prompts persist with the patch file. Unresolved bubbles re-open as "pending but not running" sticky notes; user can hit re-run.
- In-flight requests are dropped on close. No background daemon, no reconciliation problems.
- Resolved bubbles persist as their materialized macro form (they're just lisp at that point).

## Library (Save-on-Use)

When a resolved bubble's output wire is connected downstream, log to library:

- The macro source
- The original prompt
- The signature
- Timestamp + originating patch path
- A short hash so duplicates can be de-duped

Library UI is out of scope for this spec — but the data shape above should be enough to support whatever browsing/search UI lands later.

## Visual States

| State | Appearance |
|---|---|
| Empty (just spawned) | Bubble outline, prompt field focused |
| Pending | Pulsing outline, token counter, passthrough internally |
| Resolved | Soft glow on resolve (briefly), then normal node-with-macro appearance |
| Error | Red-tinted outline, retry/edit affordance |
| Persisted-pending (post-reopen) | Greyed outline, "re-run" affordance |

## Open Questions / Deliberately Deferred

- **Library UI**: browsing, tagging, search. Spec only defines the *write path*.
- **"Think harder" tier**: Sonnet/Opus path for complex asks.
- **Cross-bubble awareness**: bubbles currently don't know about each other. Could later let a bubble reference a sibling ("like that one but with feedback").
- **Cancellation**: can the user kill an in-flight request? Probably yes via right-click → discard, but interaction details TBD.
- **Concurrency cap**: should we limit how many bubbles can be in flight at once? Probably soft cap (~8) just to keep API costs sane.
