use std::num::NonZeroU64;

use codex_protocol::ThreadId;
use pretty_assertions::assert_eq;

use crate::InputIdentity;
use crate::InputStreamIncarnation;

#[test]
fn input_identity_has_fixed_json_shape_round_trips_and_displays_incarnation() {
    let incarnation: InputStreamIncarnation =
        serde_json::from_value(serde_json::json!("fedcba98-7654-3210-fedc-ba9876543210"))
            .expect("deserialize fixed incarnation");
    let identity = InputIdentity {
        thread_id: ThreadId::from_u128(0x0123456789abcdef0123456789abcdef),
        incarnation,
        sequence: NonZeroU64::new(u64::MAX).expect("maximum sequence is nonzero"),
    };
    let expected = serde_json::json!({
        "thread_id": "01234567-89ab-cdef-0123-456789abcdef",
        "incarnation": "fedcba98-7654-3210-fedc-ba9876543210",
        "sequence": u64::MAX,
    });

    let value = serde_json::to_value(identity).expect("serialize identity");
    assert_eq!(value, expected);
    let decoded: InputIdentity = serde_json::from_value(value).expect("deserialize identity");
    assert_eq!(decoded, identity);
    assert_eq!(
        identity.incarnation.to_string(),
        "fedcba98-7654-3210-fedc-ba9876543210"
    );
}

#[test]
fn input_identity_rejects_zero_sequence_and_malformed_incarnation() {
    let valid = serde_json::json!({
        "thread_id": "01234567-89ab-cdef-0123-456789abcdef",
        "incarnation": "fedcba98-7654-3210-fedc-ba9876543210",
        "sequence": u64::MAX,
    });

    let mut zero_sequence = valid.clone();
    zero_sequence["sequence"] = serde_json::json!(0);
    assert!(serde_json::from_value::<InputIdentity>(zero_sequence).is_err());

    let mut malformed_incarnation = valid;
    malformed_incarnation["incarnation"] = serde_json::json!("not-a-uuid");
    assert!(serde_json::from_value::<InputIdentity>(malformed_incarnation).is_err());
}
