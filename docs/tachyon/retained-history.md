# Bounded retained transcript history

Status: storage seam and compatibility admission implemented and package-tested; production lifecycle integration is not implemented.

## Ownership

Durable harness history belongs above model execution. A bounded store for original transcript
evidence is a reusable history capability, not a provider runtime or an approval decision.

The storage seam belongs in `codex-history` alongside the generic `HistoryEnvelope<T, M>`.
Its payload is generic: the existing `ResponseItemEnvelope` can be an explicit compatibility
consumer without becoming the kernel's universal item format.

Keep three responsibilities distinct:

- The model's prompt window can be rewritten by compaction.
- Retained original transcript evidence can outlive that window, within its own budget.
- Guardian's runtime trusted answers and completed review scores have separate eligibility and
  lifetime rules. This change does not persist those scores or grant approval authority.

## Storage contract

The caller supplies each item's retention class and charged size. Protected and ordinary classes
have independent count and charged-byte limits. Tool traffic must not evict protected input.
Protected means eviction priority, not trusted content or authorization.

The store preserves chronological order. Protected overflow removes only enough oldest protected
items to fit. Ordinary overflow removes at least the oldest half of existing ordinary items, and
then any further oldest ordinary items required by either limit. Oversized or disabled-class
items are rejected before cloning, without changing existing items or generation.

Snapshots share payloads and preserve captured membership, order and accounting after mutation.
Arbitrary payload interior mutability remains the caller's responsibility. Budgets apply to the
live store's charged contents; outstanding snapshots can retain evicted payloads. These budgets
are neither an allocator quota nor a process-wide memory or disk limit.

Eviction, reset and rollback invalidate retained offsets through a generation change. Appending
without eviction does not. A generation is container-local, not a cross-session identity or a
completeness guarantee. Outcomes must make rejection and eviction observable. Class-specific
eviction can create gaps inside the transcript, not merely remove one contiguous prefix.

Rollback removes a caller-selected boundary and everything after it. If the boundary no longer
exists, it clears all retained evidence. The store cannot determine accepted-input order or
whether an item represents a grant; that requires host-owned provenance.

## Downstream integration gates

The storage seam alone must not be advertised as the upstream Guardian feature. Before wiring it
into production, separately implement and validate:

1. A compatibility adapter that measures retained payloads before cloning, accounts for content
   omitted by wire serialization, and applies explicit oversized-media policy.
2. Original-item provenance, accepted-input identity/order and compaction-output exclusion.
   Record order alone must not substitute for actual accepted-input order.
3. Independent immutable review snapshots, with omission indicators and missing tool-call/result
   handling. A missing piece must not be presented as complete evidence.
4. Review cursor/cache invalidation separate from user authorization. Confirmed host delivery is
   distinct from speculative assistant output or a failed external send.
5. Consistent bounded checkpoint capture and replay across local and remote compaction. Existing
   resume reconstruction starts at replacement history, so an extra live vector is insufficient.
6. Legacy missing-metadata behavior, rollback of queued/local sources and explicit fork-source
   handling. Inherited transcript is not an inherited runtime approval/cache.

Provider wire schemas, Code Mode aliases, Guardian mode selection and transcript rendering stay
in their compatibility or product layers. No new provider-specific field belongs in canonical
`ModelRequest` or `ModelEvent` for this retention work.

## Validation boundary

Stage A tests cover independent class limits, order, half-eviction, pre-clone rejection,
accounting overflow, immutable snapshots, bounded reset reporting and missing-boundary clearing.
The whole history crate must still pass its existing projection and rollout serialization tests.
Those results do not establish compaction, resume, fork, confirmed-delivery or approval behavior;
the corresponding production integration regressions remain required.

The focused storage tests passed 12/12, and the complete `codex-history --lib` suite passed 40/40
with Rust 1.95.0 and no retries or selected skips. Independent source review found no Stage A
correctness blocker. This is a local, unpublished change, not validation of downstream callers.

For publication, split the current foundation into append/snapshot/budgets/eviction and a dependent
reset/truncate/reporting slice. The combined implementation, tests and design note exceed the
repository's preferred review size; preserve the lifecycle tests rather than removing coverage
to meet that guideline.

## Compatibility admission slice

The next slice prepares existing `ResponseItemEnvelope` payloads for the generic store without
teaching `RetainedHistory<T>` about Responses. Its counting writer bounds serialized item,
sidecar metadata and separately serialized reasoning content before any deep payload clone.
Reasoning is conservatively charged even when wire serialization already includes it. All
metadata is charged, including analytics: this local storage budget does not exempt potentially
large metadata from admission checks.

Compaction controls and messages annotated as `compaction.summary` are excluded. A fitting
original envelope stays borrowed. Only an oversized user message containing images is eligible
for a text-only fallback; empty text, oversized text/metadata and other oversized item types
remain rejected. The fallback must expose media omission, preserve message identity/turn/time,
phase and sidecar metadata, and filter aligned content classifications with the retained text.
Malformed classification alignment becomes unknown rather than fabricating a new provenance.

This preparation does not select an authorization class, rewrite the caller's history or grant
trust. Production original-item provenance, accepted-input ordering, checkpoint/replay, reviewer
omission handling and session ownership are still separate dependent integration gates.

Before publication, split this adapter into bounded full-item admission and a dependent lossy
media-fallback slice. The first slice includes the counting writer, sidecar/reasoning charge,
exclusions, borrowed full-fit path and their tests; over-budget items conservatively reject.
The second adds the borrowed fallback serializer, text/annotation filtering, omission propagation
and fallback tests. The combined source/tests exceed the 800-line review guideline, even though
production logic is below the 500-line complex-logic ceiling. Keep the regression oracles intact.

The frozen compatibility adapter passed 11/11 focused tests, then the complete history library
passed 51/51 with Rust 1.95.0, two build jobs and retries disabled. Independent source review
found no blocker in this scope; publication remains conditional on the split above. The focused
run's 40 filter exclusions were subsequently exercised by the full package run. All six tested
source/document hashes remained unchanged through validation. Shared target size was
4,586,864 KiB before and 4,587,416 KiB after; free space stayed above 30 GiB. These local tests do
not establish production compaction, restart, fork or review-authority behavior.

## Accepted-input provenance prerequisite

Current Core distinguishes routing admission (`Started`/`Steered`) from prompt-hook success.
The serialized `session/handlers.rs` submission loop is the available admission-order owner.
For steering, `turn_input.rs` commits input to the pending queue under the active-turn lock;
for a start, it hands input to the newly started task. A hook may subsequently block that input
before `record_user_prompt_and_emit_turn_item` records any original transcript evidence.

A future host-owned accepted-input envelope must carry identity/order across that gap. Allocate
it at successful admission/queue commit, keep it through started/steered input and hook handling,
and attach persisted source metadata only to recorded, hook-allowed evidence. Caller client IDs,
UUID creation timestamps, response-item IDs assigned later and rollout-write ordinals are not
acceptance-order substitutes. Caller disconnection does not retract a submitted request;
suspension may discard queued input, which must not become durable evidence merely by admission.

Before implementing the cross-restart contract, choose a durable high-water/epoch owner that
cannot reuse an old sequence after rollback or resume, and an explicit inherited-source policy
for forks. A local counter alone cannot prove those semantics. Regressions must cross start,
steer, rejected admission, blocked hooks, disconnection, rollback, resume and fork, alongside the
compaction/checkpoint tests above. Runtime approvals/caches remain separate from transcript data.

### Durable owner direction (not implemented)

Use a storage-neutral optional ThreadStore reservation contract, implemented locally by a
per-thread journal outside canonical rollouts, optional SQLite projections and disposable
writer-lock files. A stable journal incarnation plus a monotonically reserved sequence supplies
identity/order; a reservation may be unused, so the sequence is deliberately not gapless.
Rollback and immutable-rollout revert must not rewind it. A missing/corrupt journal must not be
reconstructed from timestamps or rollout ordinals: a new incarnation is separate, and review
evidence spanning the discontinuity is incomplete. Persistence/atomic replacement, corruption,
concurrent ownership and counter overflow need explicit tests before this becomes a contract.

Reserve at the serialized successful admission/queue commit, before hooks can finish out of
order. A reservation immediately before a subsequently failed commit may leave a gap. Carry it
through start/steer queues and hooks, but record evidence only for allowed original human input.
Blocked, discarded or synthetic inputs must never become a user grant merely because a host
reserved an identity. The journal itself is not evidence. Started/Steered remains a routing
acknowledgment, not a durability acknowledgment.

The parent inspected two additional production gates: realtime text directly calls turn_input
handling outside the normal submission loop, and generic start paths also serve synthetic
prompts. All actual human input paths need one admission-order owner and explicit origin
classification; model-provided role/content and a caller client ID cannot supply that origin.
The current persist_rollout_items helper logs and swallows append failures. Enhanced evidence
can be marked durable only after a checked canonical append/flush, without silently changing
ordinary input availability. Unsupported, volatile, ephemeral or failed reservation persistence
must have an observable unavailable/incomplete provenance state. Legacy non-SQLite input still
works; optional review enhancement must not silently turn SQLite into an availability dependency.

Forks receive a child journal incarnation. Inherited sources keep their parent identity rather
than being re-stamped as new human input; source-relative order does not invent a total order
across independent streams. Runtime review/approval caches start empty. Resume, rollback and
compaction must carry completeness and source identity independently of the mutable prompt.

The read-only recommendation spans too many responsibilities for one worker assignment. Before
implementation, split into a reviewed storage contract/Local journal with crash/reopen tests;
an independently reviewed explicit-origin and shared admission/queue integration; then checked
original-evidence persistence/replay and lifecycle integration. Exact rollout representation and
outage reporting are unresolved design gates, not license to add provider fields to canonical
ModelRequest/ModelEvent. No current production call site uses a reservation journal.
