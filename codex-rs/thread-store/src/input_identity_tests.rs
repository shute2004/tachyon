use std::num::NonZeroU64;

use codex_protocol::ThreadId;
use pretty_assertions::assert_eq;

use crate::InMemoryThreadStore;
use crate::InputIdentityContinuity;
use crate::InputIdentityReservation;
use crate::InputStreamIncarnation;
use crate::ReservedInputIdentity;
use crate::ThreadStore;
use crate::ThreadStoreError;

#[test]
fn input_identity_reservation_round_trips_and_rejects_zero_sequence() {
    let incarnation: InputStreamIncarnation =
        serde_json::from_value(serde_json::json!("fedcba98-7654-3210-fedc-ba9876543210"))
            .expect("deserialize fixed incarnation");
    let reservation = InputIdentityReservation {
        identity: ReservedInputIdentity {
            thread_id: ThreadId::from_u128(0x0123456789abcdef0123456789abcdef),
            incarnation,
            sequence: NonZeroU64::new(7).expect("positive sequence"),
        },
        continuity: InputIdentityContinuity::Continuing,
    };
    let expected = serde_json::json!({
        "identity": {
            "thread_id": "01234567-89ab-cdef-0123-456789abcdef",
            "incarnation": "fedcba98-7654-3210-fedc-ba9876543210",
            "sequence": 7,
        },
        "continuity": "Continuing",
    });
    let value = serde_json::to_value(reservation).expect("serialize reservation");
    assert_eq!(value, expected);
    let decoded: InputIdentityReservation =
        serde_json::from_value(value.clone()).expect("deserialize reservation");
    assert_eq!(decoded, reservation);

    let history_identity: codex_rollout::InputIdentity = reservation.identity;
    assert_eq!(history_identity, reservation.identity);
    let history_incarnation: codex_rollout::InputStreamIncarnation =
        reservation.identity.incarnation;
    assert_eq!(history_incarnation, incarnation);

    let mut zero_sequence = value;
    zero_sequence["identity"]["sequence"] = serde_json::Value::from(0_u64);
    assert!(serde_json::from_value::<InputIdentityReservation>(zero_sequence).is_err());
}

#[tokio::test]
async fn in_memory_store_reports_input_identity_reservation_unsupported() {
    let concrete_store = InMemoryThreadStore::default();
    let store: &dyn ThreadStore = &concrete_store;

    let result = store.reserve_input_identity(ThreadId::from_u128(1)).await;

    assert!(matches!(
        result,
        Err(ThreadStoreError::Unsupported {
            operation: "reserve_input_identity"
        })
    ));
}
