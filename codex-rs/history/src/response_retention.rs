//! Bounded admission for Responses-compatible history items.
//!
//! This is a logical per-item charge, not an allocator or process-wide memory quota. The check
//! avoids cloning oversized payloads inside this adapter; callers may already have allocated or
//! cloned the input before calling it. When media is omitted, callers must retain the returned
//! omission flag alongside the payload and must not present that payload as complete evidence.
//! This adapter does not choose a retention class or imply trust, approval, or authority.

use std::borrow::Cow;
use std::io;
use std::io::Write;
use std::mem::size_of;

use codex_protocol::ResponseItemId;
use codex_protocol::models::ContentItem;
use codex_protocol::models::InternalChatMessageMetadataPassthrough;
use codex_protocol::models::MessagePhase;
use codex_protocol::models::ResponseItem;
use serde::Serialize;
use serde::Serializer;
use serde::ser::SerializeMap;
use serde::ser::SerializeSeq;

use crate::CodexHarnessMetadata;
use crate::ResponseItemEnvelope;

/// Why an item could not be admitted by the compatibility retention adapter.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ResponseItemRetentionRejection {
    /// The item is a compaction control/summary rather than ordinary transcript evidence.
    CompactionControl,
    /// The item is an unknown wire variant whose original payload has already been lost.
    UnsupportedItem,
    /// The item or its supported text-only fallback exceeds the caller's logical byte budget.
    OverBudget,
    /// A known item failed serialization for a reason other than the bounded writer's limit.
    Serialization,
}

/// A prepared item and the information its caller must preserve when storing it.
#[derive(Debug)]
pub struct PreparedResponseItemRetention<'a> {
    envelope: Cow<'a, ResponseItemEnvelope>,
    charged_bytes: usize,
    omitted_media: bool,
}

impl<'a> PreparedResponseItemRetention<'a> {
    /// The admitted envelope; a full-media item borrows the caller's original envelope.
    pub fn envelope(&self) -> &ResponseItemEnvelope {
        self.envelope.as_ref()
    }

    /// The caller-supplied accounting charge for this prepared item.
    pub fn charged_bytes(&self) -> usize {
        self.charged_bytes
    }

    /// Whether the user-message fallback omitted image or audio content.
    pub fn omitted_media(&self) -> bool {
        self.omitted_media
    }

    /// Consumes the result without dropping the omission status needed by downstream consumers.
    pub fn into_parts(self) -> (Cow<'a, ResponseItemEnvelope>, usize, bool) {
        (self.envelope, self.charged_bytes, self.omitted_media)
    }
}

/// Measures an item without allocating a JSON buffer and prepares the only supported media
/// fallback: user-message text after an oversized image-bearing message fails the full charge.
/// At least one text entry must be nonempty; once eligible, all text entries and their order are
/// preserved, including empty strings. Whitespace counts as nonempty.
pub fn prepare_response_item_retention(
    envelope: &ResponseItemEnvelope,
    max_bytes: usize,
) -> Result<PreparedResponseItemRetention<'_>, ResponseItemRetentionRejection> {
    if matches!(&envelope.item, ResponseItem::Other) {
        return Err(ResponseItemRetentionRejection::UnsupportedItem);
    }
    if is_compaction_control(&envelope.item) || has_compaction_summary_annotation(&envelope.item) {
        return Err(ResponseItemRetentionRejection::CompactionControl);
    }

    match measure_envelope(envelope, max_bytes) {
        Ok(charged_bytes) => Ok(PreparedResponseItemRetention {
            envelope: Cow::Borrowed(envelope),
            charged_bytes,
            omitted_media: false,
        }),
        Err(ResponseItemRetentionRejection::OverBudget) => {
            prepare_text_fallback(envelope, max_bytes)
        }
        Err(rejection) => Err(rejection),
    }
}

fn is_compaction_control(item: &ResponseItem) -> bool {
    matches!(
        item,
        ResponseItem::Compaction { .. }
            | ResponseItem::CompactionTrigger { .. }
            | ResponseItem::ContextCompaction { .. }
    )
}

fn has_compaction_summary_annotation(item: &ResponseItem) -> bool {
    match item {
        ResponseItem::Message {
            internal_chat_message_metadata_passthrough: Some(metadata),
            ..
        } => metadata
            .content_item_kinds
            .as_ref()
            .is_some_and(|kinds| kinds.iter().any(|kind| kind.0 == "compaction.summary")),
        _ => false,
    }
}

fn measure_envelope(
    envelope: &ResponseItemEnvelope,
    max_bytes: usize,
) -> Result<usize, ResponseItemRetentionRejection> {
    let mut writer = CountingWriter::new(max_bytes)?;
    write_serialized(&mut writer, &envelope.item)?;
    if let Some(metadata) = envelope.metadata.as_ref() {
        write_serialized(&mut writer, metadata)?;
    }
    if let ResponseItem::Reasoning {
        content: Some(content),
        ..
    } = &envelope.item
    {
        // The main ResponseItem serializer intentionally skips some reasoning content. Charge it
        // separately in every case so skipped Text content does not become an uncharged payload.
        write_serialized(&mut writer, content)?;
    }
    Ok(writer.charged_bytes)
}

fn prepare_text_fallback(
    envelope: &ResponseItemEnvelope,
    max_bytes: usize,
) -> Result<PreparedResponseItemRetention<'_>, ResponseItemRetentionRejection> {
    let ResponseItem::Message {
        id,
        role,
        content,
        phase,
        internal_chat_message_metadata_passthrough,
    } = &envelope.item
    else {
        return Err(ResponseItemRetentionRejection::OverBudget);
    };

    let has_image = content
        .iter()
        .any(|item| matches!(item, ContentItem::InputImage { .. }));
    let has_nonempty_text = content.iter().any(|item| match item {
        ContentItem::InputText { text } | ContentItem::OutputText { text } => !text.is_empty(),
        ContentItem::InputImage { .. } | ContentItem::InputAudio { .. } => false,
    });
    let text_count = text_item_count(content);
    if role != "user" || !has_image || !has_nonempty_text {
        return Err(ResponseItemRetentionRejection::OverBudget);
    }

    let charged_bytes = measure_text_fallback(
        id,
        role,
        content,
        phase,
        internal_chat_message_metadata_passthrough,
        envelope.metadata.as_ref(),
        max_bytes,
    )?;

    // The bounded borrowed representation has fit. Only now clone retained text and metadata;
    // image/audio payloads are never cloned by this fallback.
    let retained_content = retain_text_content(content, text_count);
    let retained_metadata =
        retain_text_metadata(internal_chat_message_metadata_passthrough, content);
    let item = ResponseItem::Message {
        id: id.clone(),
        role: role.clone(),
        content: retained_content,
        phase: phase.clone(),
        internal_chat_message_metadata_passthrough: retained_metadata,
    };
    let prepared = ResponseItemEnvelope {
        item,
        metadata: envelope.metadata.clone(),
    };

    Ok(PreparedResponseItemRetention {
        envelope: Cow::Owned(prepared),
        charged_bytes,
        omitted_media: true,
    })
}

fn text_item_count(content: &[ContentItem]) -> usize {
    content
        .iter()
        .filter(|item| {
            matches!(
                item,
                ContentItem::InputText { .. } | ContentItem::OutputText { .. }
            )
        })
        .count()
}

fn retain_text_content(content: &[ContentItem], text_count: usize) -> Vec<ContentItem> {
    let mut retained = Vec::with_capacity(text_count);
    for item in content {
        match item {
            ContentItem::InputText { text } => {
                retained.push(ContentItem::InputText { text: text.clone() });
            }
            ContentItem::OutputText { text } => {
                retained.push(ContentItem::OutputText { text: text.clone() });
            }
            ContentItem::InputImage { .. } | ContentItem::InputAudio { .. } => {}
        }
    }
    retained
}

fn retain_text_metadata(
    metadata: &Option<InternalChatMessageMetadataPassthrough>,
    original_content: &[ContentItem],
) -> Option<InternalChatMessageMetadataPassthrough> {
    let mut retained = metadata.clone();
    if let Some(retained) = retained.as_mut() {
        if let Some(kinds) = retained.content_item_kinds.take() {
            retained.content_item_kinds = if kinds.len() == original_content.len() {
                Some(
                    kinds
                        .into_iter()
                        .zip(original_content)
                        .filter_map(|(kind, item)| {
                            matches!(
                                item,
                                ContentItem::InputText { .. } | ContentItem::OutputText { .. }
                            )
                            .then_some(kind)
                        })
                        .collect(),
                )
            } else {
                None
            };
        }
    }
    retained
}

fn measure_text_fallback(
    id: &Option<ResponseItemId>,
    role: &str,
    content: &[ContentItem],
    phase: &Option<MessagePhase>,
    internal_metadata: &Option<InternalChatMessageMetadataPassthrough>,
    sidecar_metadata: Option<&CodexHarnessMetadata>,
    max_bytes: usize,
) -> Result<usize, ResponseItemRetentionRejection> {
    let mut writer = CountingWriter::new(max_bytes)?;
    let message = TextOnlyMessage {
        id,
        role,
        content,
        phase,
        internal_metadata,
    };
    write_serialized(&mut writer, &message)?;
    if let Some(metadata) = sidecar_metadata {
        write_serialized(&mut writer, metadata)?;
    }
    Ok(writer.charged_bytes)
}

struct TextOnlyMessage<'a> {
    id: &'a Option<ResponseItemId>,
    role: &'a str,
    content: &'a [ContentItem],
    phase: &'a Option<MessagePhase>,
    internal_metadata: &'a Option<InternalChatMessageMetadataPassthrough>,
}

impl Serialize for TextOnlyMessage<'_> {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        let fields = 3
            + usize::from(self.id.is_some() as u8)
            + usize::from(self.phase.is_some() as u8)
            + usize::from(self.internal_metadata.is_some() as u8);
        let mut map = serializer.serialize_map(Some(fields))?;
        map.serialize_entry("type", "message")?;
        if let Some(id) = self.id.as_ref() {
            map.serialize_entry("id", id)?;
        }
        map.serialize_entry("role", self.role)?;
        map.serialize_entry("content", &TextOnlyContent(self.content))?;
        if let Some(phase) = self.phase.as_ref() {
            map.serialize_entry("phase", phase)?;
        }
        if let Some(metadata) = self.internal_metadata.as_ref() {
            map.serialize_entry("internal_chat_message_metadata_passthrough", metadata)?;
        }
        map.end()
    }
}

struct TextOnlyContent<'a>(&'a [ContentItem]);

impl Serialize for TextOnlyContent<'_> {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        let mut sequence = serializer.serialize_seq(Some(text_item_count(self.0)))?;
        for item in self.0 {
            if matches!(
                item,
                ContentItem::InputText { .. } | ContentItem::OutputText { .. }
            ) {
                sequence.serialize_element(item)?;
            }
        }
        sequence.end()
    }
}

struct CountingWriter {
    max_bytes: usize,
    charged_bytes: usize,
    exceeded: bool,
}

impl CountingWriter {
    fn new(max_bytes: usize) -> Result<Self, ResponseItemRetentionRejection> {
        let charged_bytes = size_of::<ResponseItemEnvelope>();
        if charged_bytes > max_bytes {
            return Err(ResponseItemRetentionRejection::OverBudget);
        }
        Ok(Self {
            max_bytes,
            charged_bytes,
            exceeded: false,
        })
    }
}

impl Write for CountingWriter {
    fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
        let Some(remaining) = self.max_bytes.checked_sub(self.charged_bytes) else {
            self.exceeded = true;
            return Err(io::Error::other("retention limit exceeded"));
        };
        if bytes.len() > remaining {
            self.exceeded = true;
            return Err(io::Error::other("retention limit exceeded"));
        }
        let Some(charged_bytes) = self.charged_bytes.checked_add(bytes.len()) else {
            self.exceeded = true;
            return Err(io::Error::other("retention limit exceeded"));
        };
        self.charged_bytes = charged_bytes;
        Ok(bytes.len())
    }

    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

fn write_serialized<T: Serialize>(
    writer: &mut CountingWriter,
    value: &T,
) -> Result<(), ResponseItemRetentionRejection> {
    match serde_json::to_writer(&mut *writer, value) {
        Ok(()) => Ok(()),
        Err(_) if writer.exceeded => Err(ResponseItemRetentionRejection::OverBudget),
        Err(_) => Err(ResponseItemRetentionRejection::Serialization),
    }
}
