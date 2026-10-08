use std::num::NonZeroU64;

use anyhow::Result;
use codex_protocol::ThreadId;
use pretty_assertions::assert_eq;
use serde_json::Value;
use serde_json::json;

use super::*;

fn identity() -> InputIdentity {
    InputIdentity {
        thread_id: ThreadId::from_u128(0x0123456789abcdef0123456789abcdef),
        incarnation: serde_json::from_value(json!("fedcba98-7654-3210-fedc-ba9876543210"))
            .expect("fixed incarnation is valid"),
        sequence: NonZeroU64::new(u64::MAX).expect("maximum sequence is nonzero"),
    }
}

fn association(source: InputSource) -> InputAssociation {
    InputAssociation {
        identity: identity(),
        source,
    }
}

fn response_message(role: &str, text: &str) -> ResponseItem {
    ResponseItem::Message {
        id: None,
        role: role.to_string(),
        content: vec![ContentItem::InputText {
            text: text.to_string(),
        }],
        phase: None,
        internal_chat_message_metadata_passthrough: None,
    }
}

#[test]
fn input_association_has_fixed_json_shape_for_each_source_and_max_sequence() -> Result<()> {
    for (source, source_value) in [
        (InputSource::Unknown, "unknown"),
        (InputSource::Synthetic, "synthetic"),
    ] {
        let expected = json!({
            "identity": {
                "thread_id": "01234567-89ab-cdef-0123-456789abcdef",
                "incarnation": "fedcba98-7654-3210-fedc-ba9876543210",
                "sequence": u64::MAX,
            },
            "source": source_value,
        });
        let serialized = serde_json::to_value(association(source))?;

        assert_eq!(serialized, expected);
        assert_eq!(
            serde_json::from_value::<InputAssociation>(serialized)?,
            association(source)
        );
    }
    Ok(())
}

#[test]
fn legacy_absent_association_keeps_response_and_metadata_shape_unchanged() -> Result<()> {
    let legacy_without_metadata = r#"{"type":"response_item","payload":{"type":"message","role":"user","content":[{"type":"input_text","text":"hello"}]}}"#;
    let restored: RolloutItem = serde_json::from_str(legacy_without_metadata)?;
    let RolloutItem::ResponseItem(envelope) = &restored else {
        panic!("expected response item");
    };
    assert_eq!(envelope.metadata, None);
    assert_eq!(serde_json::to_string(&restored)?, legacy_without_metadata);

    let legacy_with_metadata = r#"{"type":"response_item","payload":{"type":"message","role":"user","content":[{"type":"input_text","text":"hello"}]},"metadata":{"client_authored":true,"fallback_token_limit_override":4096}}"#;
    let item = RolloutItem::ResponseItem(ResponseItemEnvelope {
        item: response_message("user", "hello"),
        metadata: Some(CodexHarnessMetadata {
            client_authored: true,
            fallback_token_limit_override: Some(4096),
            input_association: None,
        }),
    });
    assert_eq!(serde_json::to_string(&item)?, legacy_with_metadata);
    let restored: RolloutItem = serde_json::from_str(legacy_with_metadata)?;
    let RolloutItem::ResponseItem(envelope) = &restored else {
        panic!("expected response item");
    };
    assert_eq!(
        envelope.metadata,
        Some(CodexHarnessMetadata {
            client_authored: true,
            fallback_token_limit_override: Some(4096),
            input_association: None,
        })
    );
    assert_eq!(serde_json::to_string(&restored)?, legacy_with_metadata);
    assert_eq!(
        serde_json::to_value(CodexHarnessMetadata::default())?,
        json!({ "client_authored": false }),
    );
    Ok(())
}

#[test]
fn response_item_round_trip_keeps_association_beside_unchanged_payload() -> Result<()> {
    let response_item = response_message("user", "associated input");
    let expected_envelope = ResponseItemEnvelope {
        item: response_item.clone(),
        metadata: Some(CodexHarnessMetadata {
            input_association: Some(association(InputSource::Unknown)),
            ..Default::default()
        }),
    };
    let item = RolloutItem::ResponseItem(expected_envelope.clone());
    let serialized = serde_json::to_value(&item)?;

    assert_eq!(serialized["payload"], serde_json::to_value(&response_item)?);
    assert_eq!(
        serialized["metadata"]["input_association"],
        serde_json::to_value(association(InputSource::Unknown))?,
    );

    let restored: RolloutItem = serde_json::from_value(serialized.clone())?;
    let RolloutItem::ResponseItem(restored_envelope) = restored else {
        panic!("expected response item");
    };
    assert_eq!(restored_envelope, expected_envelope);
    assert_eq!(
        serde_json::to_value(restored_envelope.item)?,
        serialized["payload"]
    );
    Ok(())
}

#[test]
fn compacted_associations_remain_aligned_and_do_not_fill_neighbor_defaults() -> Result<()> {
    let associated_item = response_message("user", "associated");
    let unassociated_item = response_message("assistant", "unassociated");
    let replacement_history = vec![
        ResponseItemEnvelope {
            item: associated_item.clone(),
            metadata: Some(CodexHarnessMetadata {
                input_association: Some(association(InputSource::Synthetic)),
                ..Default::default()
            }),
        },
        ResponseItemEnvelope::new(unassociated_item.clone()),
    ];
    let compacted = CompactedItem {
        message: "summary".to_string(),
        replacement_history: Some(replacement_history.clone()),
        mcp_resource_origins: None,
        window_number: None,
        first_window_id: None,
        previous_window_id: None,
        window_id: None,
    };
    let serialized = serde_json::to_value(compacted)?;

    assert_eq!(
        serialized["replacement_history"],
        json!([associated_item, unassociated_item]),
    );
    assert_eq!(
        serialized["replacement_history_metadata"],
        json!([
            {
                "client_authored": false,
                "input_association": serde_json::to_value(association(InputSource::Synthetic))?,
            },
            { "client_authored": false },
        ]),
    );

    let restored: CompactedItem = serde_json::from_value(serialized)?;
    assert_eq!(
        restored.replacement_history,
        Some(vec![
            replacement_history[0].clone(),
            ResponseItemEnvelope {
                item: unassociated_item,
                metadata: Some(CodexHarnessMetadata::default()),
            },
        ]),
    );
    assert_eq!(
        restored.replacement_history.as_ref().expect("history")[1]
            .metadata
            .as_ref()
            .expect("aligned metadata")
            .input_association,
        None,
    );
    Ok(())
}

#[test]
fn compacted_association_sidecars_still_reject_misalignment_and_missing_history() {
    let association_value = serde_json::to_value(association(InputSource::Unknown))
        .expect("association should serialize");
    let malformed = [
        json!({
            "message": "summary",
            "replacement_history": [response_message("user", "hello")],
            "replacement_history_metadata": [],
        }),
        json!({
            "message": "summary",
            "replacement_history_metadata": [{ "input_association": association_value }],
        }),
    ];

    for value in malformed {
        assert!(serde_json::from_value::<CompactedItem>(value).is_err());
    }
}

#[test]
fn projections_preserve_association_for_canonical_and_fallback_items() {
    let canonical = ResponseItemEnvelope {
        item: response_message("user", "canonical input"),
        metadata: Some(CodexHarnessMetadata {
            input_association: Some(association(InputSource::Unknown)),
            ..Default::default()
        }),
    };
    let fallback = ResponseItemEnvelope {
        item: response_message("future_role", "fallback input"),
        metadata: Some(CodexHarnessMetadata {
            input_association: Some(association(InputSource::Synthetic)),
            ..Default::default()
        }),
    };

    match project_response_item(canonical.clone()) {
        HistoryItemProjection::Canonical { compatibility, .. } => {
            assert_eq!(compatibility, canonical);
        }
        HistoryItemProjection::Fallback { .. } => panic!("expected canonical projection"),
    }
    match project_response_item(fallback.clone()) {
        HistoryItemProjection::Fallback { compatibility, .. } => {
            assert_eq!(compatibility, fallback);
        }
        HistoryItemProjection::Canonical { .. } => panic!("expected fallback projection"),
    }
}

#[test]
fn association_deserialization_rejects_missing_source_zero_sequence_and_bad_uuid() -> Result<()> {
    let valid = serde_json::to_value(association(InputSource::Unknown))?;

    let mut missing_source = valid.clone();
    missing_source
        .as_object_mut()
        .expect("association is an object")
        .remove("source");
    assert!(serde_json::from_value::<InputAssociation>(missing_source).is_err());

    let mut zero_sequence = valid.clone();
    zero_sequence["identity"]["sequence"] = json!(0);
    assert!(serde_json::from_value::<InputAssociation>(zero_sequence).is_err());

    let mut malformed_uuid = valid;
    malformed_uuid["identity"]["incarnation"] = json!("not-a-uuid");
    assert!(serde_json::from_value::<InputAssociation>(malformed_uuid).is_err());
    Ok(())
}

fn schema_has_required(schema: &Value, field: &str) -> bool {
    schema["required"]
        .as_array()
        .is_some_and(|required| required.contains(&json!(field)))
}

#[test]
fn association_and_metadata_schemas_describe_required_values_and_optional_sidecar() -> Result<()> {
    let association_schema = serde_json::to_value(schemars::schema_for!(InputAssociation))?;
    assert!(schema_has_required(&association_schema, "identity"));
    assert!(schema_has_required(&association_schema, "source"));
    assert_eq!(
        association_schema["definitions"]["InputSource"]["enum"],
        json!(["unknown", "synthetic"]),
    );
    let identity_schema = &association_schema["definitions"]["InputIdentity"];
    assert!(schema_has_required(identity_schema, "thread_id"));
    assert!(schema_has_required(identity_schema, "incarnation"));
    assert!(schema_has_required(identity_schema, "sequence"));
    assert_eq!(
        identity_schema["properties"]["sequence"]["minimum"].as_f64(),
        Some(1.0),
    );
    assert_eq!(
        identity_schema["properties"]["incarnation"]["allOf"],
        json!([{ "$ref": "#/definitions/ThreadId" }]),
        "incarnation schema: {}",
        identity_schema["properties"]["incarnation"],
    );
    assert_eq!(
        association_schema["definitions"]["ThreadId"]["type"],
        json!("string"),
    );
    let incarnation_schema = serde_json::to_value(schemars::schema_for!(InputStreamIncarnation))?;
    assert_eq!(incarnation_schema["type"], json!("string"));

    let metadata_schema = serde_json::to_value(schemars::schema_for!(CodexHarnessMetadata))?;
    assert!(
        metadata_schema["properties"]
            .get("input_association")
            .is_some()
    );
    assert!(!schema_has_required(&metadata_schema, "input_association"));
    Ok(())
}
