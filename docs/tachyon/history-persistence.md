# History and persistence boundary

Tachyon preserves mature conversation history, compaction, resume, fork, and rollback behavior while removing provider-specific ownership from the kernel incrementally.

## Target ownership

The long-term history boundary has two distinct responsibilities:

1. kernel-owned history semantics used by context management, compaction, persistence, resume, fork, and rollback;
2. provider compatibility data needed to reproduce a provider's exact request or continuation behavior.

Provider compatibility data must not become the semantic definition of kernel history merely because the current Codex implementation stores OpenAI Responses items.

## Migration rule

Behavior preservation comes before representation replacement.

The migration therefore proceeds in small stages:

```text
Responses-shaped persisted history
        |
        v
generic history envelope + unchanged compatibility payload/metadata
        |
        v
kernel-owned history item/metadata semantics + explicit provider compatibility data
        |
        v
ContextManager and persistence consume the kernel-owned representation
        |
        v
provider adapters reconstruct provider request history where needed
```

A history item must not be forced into the canonical representation when doing so would lose provider-private data or mature harness behavior. Such cases remain on an explicit compatibility path until their generic semantics are identified.

## First slice

The first slice introduces `HistoryEnvelope<T, M>`, which is neutral with respect to both the stored item and its sidecar metadata.

Existing Codex/Responses history remains represented as:

```text
HistoryEnvelope<ResponseItem, CodexHarnessMetadata>
```

This is deliberate. `ResponseItem` is still provider/protocol-shaped, and `CodexHarnessMetadata::client_authored` is still a host-specific migration field. Neither is renamed into a generic contract merely to make the code look neutral.

The serialized rollout shape is intentionally unchanged in this slice. Existing rollouts must continue to deserialize and reserialize without changing their `response_item` payload or metadata layout.

This slice therefore neutralizes only the envelope ownership. It does **not** claim that the history item or all metadata semantics have been neutralized.

## Original input association

`InputIdentity` identifies a thread-local input stream by thread, incarnation, and nonzero
sequence. `InputAssociation` attaches that identity and a descriptive source to the original
input record, not to every message generated while processing it. Legacy records need not
have an association; no identity or origin is inferred from their text or client IDs.

Core carries the source in a process-local `SessionSubmission` envelope. Generic submissions
and realtime text remain `Unknown`. The one-shot review and Guardian review producers
explicitly classify their constructed prompt inputs as `Synthetic`; forwarding preserves
that classification. A constructed review prompt may contain user-provided text, so this
classification is not proof that its content is non-human, trusted, or authorized.

For accepted nonempty user-input admissions, Core reserves an identity through the thread
store before merging additional context and queuing the input. Rejected, empty, automatic,
and recovery admissions do not reserve an identity on this path. Reservation is best-effort:
store errors or a wrong-thread result leave the input unassociated rather than reject the
work. Reservation, admission, append, materialization, and flush are not one atomic
transaction, and unused sequence gaps are permitted.

The association is optional canonical history sidecar metadata. It is not part of the
protocol submission/request shape, serialized pending-input queue, provider request payload,
or UI turn item. Additional context retains its own unassociated records. Deserializing an
association establishes neither its source nor successful admission, persistence, or
transcript completeness.

## Next slices

The next implementation units should:

1. introduce the smallest kernel-owned history item vocabulary justified by current `ContextManager` behavior;
2. project representable message, reasoning, tool-call, and tool-result history into that vocabulary while retaining exact provider compatibility data separately;
3. split reusable history metadata from host/provider compatibility metadata without changing persisted bytes;
4. migrate model-visible `ContextManager` operations to the kernel-owned item semantics;
5. migrate persisted replacement history and resume/fork/rollback paths;
6. remove Responses-shaped ownership from kernel-facing history contracts only after lossless fallback exists.

Do not use `ModelOutputItem` as a drop-in replacement for durable history. Persisted harness history includes request-side messages, tool results, compaction state, and other lifecycle semantics that are broader than model output events alone.
