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

## Next slices

The next implementation units should:

1. introduce the smallest kernel-owned history item vocabulary justified by current `ContextManager` behavior;
2. project representable message, reasoning, tool-call, and tool-result history into that vocabulary while retaining exact provider compatibility data separately;
3. split reusable history metadata from host/provider compatibility metadata without changing persisted bytes;
4. migrate model-visible `ContextManager` operations to the kernel-owned item semantics;
5. migrate persisted replacement history and resume/fork/rollback paths;
6. remove Responses-shaped ownership from kernel-facing history contracts only after lossless fallback exists.

Do not use `ModelOutputItem` as a drop-in replacement for durable history. Persisted harness history includes request-side messages, tool results, compaction state, and other lifecycle semantics that are broader than model output events alone.
