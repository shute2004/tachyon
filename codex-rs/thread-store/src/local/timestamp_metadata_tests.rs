use std::path::PathBuf;
use std::sync::Arc;

use crate::ResumeThreadParams;
use crate::ThreadMetadataPatch;
use crate::ThreadPersistenceMetadata;
use crate::ThreadStore;
use crate::UpdateThreadMetadataParams;
use crate::local::LocalThreadStore;
use crate::local::test_support::test_config;
use crate::local::test_support::write_session_file;
use crate::local::test_support::write_session_file_with_history_mode;
use chrono::DateTime;
use chrono::Utc;
use codex_protocol::ThreadId;
use codex_protocol::protocol::ThreadHistoryMode;
use codex_protocol::protocol::ThreadMemoryMode;
use codex_rollout::state_db::reconcile_rollout;
use codex_state::StateRuntime;
use codex_state::ThreadMetadata;
use pretty_assertions::assert_eq;
use tempfile::TempDir;
use uuid::Uuid;

fn fixed_timestamp(value: &str) -> DateTime<Utc> {
    DateTime::parse_from_rfc3339(value)
        .expect("valid fixed timestamp")
        .with_timezone(&Utc)
}

fn thread_id(uuid: Uuid) -> ThreadId {
    ThreadId::from_string(&uuid.to_string()).expect("valid thread id")
}

fn persistence_metadata(cwd: PathBuf) -> ThreadPersistenceMetadata {
    ThreadPersistenceMetadata {
        cwd: Some(cwd),
        model_provider: "test-provider".to_string(),
        memory_mode: ThreadMemoryMode::Enabled,
    }
}

async fn store_with_existing_thread(
    home: &TempDir,
    uuid: Uuid,
    history_mode: ThreadHistoryMode,
) -> (LocalThreadStore, Arc<StateRuntime>, ThreadId, PathBuf) {
    let config = test_config(home.path());
    let path = write_session_file_with_history_mode(
        home.path(),
        "2025-01-03T14-00-00",
        uuid,
        history_mode,
    )
    .expect("session file");
    let runtime = StateRuntime::init(
        config.sqlite.clone(),
        config.default_model_provider_id.clone(),
    )
    .await
    .expect("state db should initialize");
    reconcile_rollout(
        Some(runtime.as_ref()),
        path.as_path(),
        "test-provider",
        /*builder*/ None,
        &[],
        /*archived_only*/ None,
        /*new_thread_memory_mode*/ None,
    )
    .await;
    let store = LocalThreadStore::new(config, Some(runtime.clone()));
    (store, runtime, thread_id(uuid), path)
}

async fn install_write_counters(store: &LocalThreadStore) -> sqlx::SqlitePool {
    let sqlite = &store.config.sqlite;
    let pool = sqlite
        .open_read_write_pool(&sqlite.state_db_path())
        .await
        .expect("open state database observer");
    for statement in [
        "CREATE TABLE test_metadata_write_counts (kind TEXT PRIMARY KEY, count INTEGER NOT NULL)",
        "INSERT INTO test_metadata_write_counts(kind, count) VALUES ('timestamp', 0), ('full', 0)",
        "CREATE TRIGGER test_count_timestamp_touch AFTER UPDATE OF updated_at_ms ON threads BEGIN UPDATE test_metadata_write_counts SET count = count + 1 WHERE kind = 'timestamp'; END",
        "CREATE TRIGGER test_count_full_update AFTER UPDATE OF title ON threads BEGIN UPDATE test_metadata_write_counts SET count = count + 1 WHERE kind = 'full'; END",
        "CREATE TRIGGER test_count_full_insert AFTER INSERT ON threads BEGIN UPDATE test_metadata_write_counts SET count = count + 1 WHERE kind = 'full'; END",
    ] {
        sqlx::query(statement)
            .execute(&pool)
            .await
            .expect("install SQL write counter");
    }
    pool
}

async fn write_count(pool: &sqlx::SqlitePool, kind: &str) -> i64 {
    sqlx::query_scalar("SELECT count FROM test_metadata_write_counts WHERE kind = ?")
        .bind(kind)
        .fetch_one(pool)
        .await
        .expect("read SQL write counter")
}

async fn update_timestamp(
    store: &LocalThreadStore,
    thread_id: ThreadId,
    updated_at: DateTime<Utc>,
) -> crate::StoredThread {
    store
        .update_thread_metadata(UpdateThreadMetadataParams {
            thread_id,
            patch: ThreadMetadataPatch {
                updated_at: Some(updated_at),
                ..Default::default()
            },
            include_archived: false,
        })
        .await
        .expect("update thread metadata")
        .expect("local store returns updated thread")
}

#[tokio::test]
async fn timestamp_only_update_touches_existing_metadata_for_both_history_modes() {
    for (index, history_mode) in [ThreadHistoryMode::Legacy, ThreadHistoryMode::Paginated]
        .into_iter()
        .enumerate()
    {
        let home = TempDir::new().expect("temp dir");
        let uuid = Uuid::from_u128(510 + index as u128);
        let (store, runtime, thread_id, _) =
            store_with_existing_thread(&home, uuid, history_mode).await;
        let before = runtime
            .get_thread(thread_id)
            .await
            .expect("read metadata before update")
            .expect("thread metadata");
        let pool = install_write_counters(&store).await;
        let updated_at = fixed_timestamp("2030-01-03T14:01:00Z");

        update_timestamp(&store, thread_id, updated_at).await;

        let actual = runtime
            .get_thread(thread_id)
            .await
            .expect("read metadata after update")
            .expect("thread metadata");
        let mut expected = before;
        expected.updated_at = updated_at;
        assert_eq!(actual, expected);
        assert_eq!(write_count(&pool, "timestamp").await, 1);
        assert_eq!(write_count(&pool, "full").await, 0);
        pool.close().await;
    }
}

#[tokio::test]
async fn timestamp_update_repairs_missing_sqlite_metadata_with_an_upsert() {
    for (index, history_mode) in [ThreadHistoryMode::Legacy, ThreadHistoryMode::Paginated]
        .into_iter()
        .enumerate()
    {
        let home = TempDir::new().expect("temp dir");
        let uuid = Uuid::from_u128(520 + index as u128);
        let thread_id = thread_id(uuid);
        let config = test_config(home.path());
        let path = write_session_file_with_history_mode(
            home.path(),
            "2025-01-03T14-00-00",
            uuid,
            history_mode,
        )
        .expect("session file");
        let runtime = StateRuntime::init(
            config.sqlite.clone(),
            config.default_model_provider_id.clone(),
        )
        .await
        .expect("state db should initialize");
        let store = LocalThreadStore::new(config, Some(runtime.clone()));
        assert!(
            runtime
                .get_thread(thread_id)
                .await
                .expect("read missing metadata row")
                .is_none()
        );
        let pool = install_write_counters(&store).await;
        let updated_at = fixed_timestamp("2030-01-03T14:02:00Z");

        update_timestamp(&store, thread_id, updated_at).await;

        let metadata = runtime
            .get_thread(thread_id)
            .await
            .expect("read repaired metadata")
            .expect("missing row should be repaired");
        assert_eq!(metadata.rollout_path, path);
        assert_eq!(metadata.history_mode, history_mode);
        assert_eq!(metadata.updated_at, updated_at);
        // Filesystem lookup read-repairs a missing row with an INSERT, then metadata update
        // performs its normal UPSERT. The timestamp observer also fires on that full UPDATE.
        assert_eq!(write_count(&pool, "timestamp").await, 1);
        assert_eq!(write_count(&pool, "full").await, 2);
        pool.close().await;
    }
}

#[tokio::test]
async fn explicit_rollout_path_and_timestamp_use_the_full_upsert_route() {
    let home = TempDir::new().expect("temp dir");
    let external_home = TempDir::new().expect("external temp dir");
    let uuid = Uuid::from_u128(530);
    let (store, runtime, thread_id, _) =
        store_with_existing_thread(&home, uuid, ThreadHistoryMode::Legacy).await;
    let new_path = write_session_file(external_home.path(), "2025-01-04T14-00-00", uuid)
        .expect("external session file");
    let pool = install_write_counters(&store).await;
    let updated_at = fixed_timestamp("2030-01-03T14:03:00Z");

    store
        .update_thread_metadata(UpdateThreadMetadataParams {
            thread_id,
            patch: ThreadMetadataPatch {
                rollout_path: Some(new_path.clone()),
                updated_at: Some(updated_at),
                ..Default::default()
            },
            include_archived: false,
        })
        .await
        .expect("update metadata")
        .expect("local store returns updated thread");

    let metadata = runtime
        .get_thread(thread_id)
        .await
        .expect("read metadata after update")
        .expect("thread metadata");
    assert_eq!(metadata.rollout_path, new_path);
    assert_eq!(metadata.updated_at, updated_at);
    assert_eq!(write_count(&pool, "timestamp").await, 1);
    assert_eq!(write_count(&pool, "full").await, 1);
    pool.close().await;
}

#[tokio::test]
async fn timestamp_update_with_staged_model_patch_uses_upsert_and_consumes_registry() {
    let home = TempDir::new().expect("temp dir");
    let uuid = Uuid::from_u128(540);
    let (store, runtime, thread_id, _) =
        store_with_existing_thread(&home, uuid, ThreadHistoryMode::Legacy).await;
    let pool = install_write_counters(&store).await;
    store
        .stage_pending_thread_metadata(
            thread_id,
            ThreadMetadataPatch {
                model_provider: Some("staged-provider".to_string()),
                ..Default::default()
            },
        )
        .await
        .expect("stage model metadata");
    let updated_at = fixed_timestamp("2030-01-03T14:04:00Z");

    update_timestamp(&store, thread_id, updated_at).await;

    let metadata = runtime
        .get_thread(thread_id)
        .await
        .expect("read metadata after update")
        .expect("thread metadata");
    assert_eq!(metadata.model_provider, "staged-provider");
    assert_eq!(metadata.updated_at, updated_at);
    assert_eq!(write_count(&pool, "full").await, 1);
    store
        .stage_pending_thread_metadata(
            thread_id,
            ThreadMetadataPatch {
                model_provider: Some("cleanup".to_string()),
                ..Default::default()
            },
        )
        .await
        .expect("pending metadata registry entry was consumed");
    store
        .remove_pending_thread_metadata(thread_id)
        .await
        .expect("remove cleanup metadata");
    pool.close().await;
}

#[tokio::test]
async fn timestamp_only_update_repairs_a_live_resume_path_mismatch() {
    let home = TempDir::new().expect("temp dir");
    let active_home = TempDir::new().expect("active rollout dir");
    let uuid = Uuid::from_u128(550);
    let (store, runtime, thread_id, stale_path) =
        store_with_existing_thread(&home, uuid, ThreadHistoryMode::Legacy).await;
    let active_path = write_session_file(active_home.path(), "2025-01-05T14-00-00", uuid)
        .expect("active session file");
    store
        .resume_thread(ResumeThreadParams {
            thread_id,
            rollout_path: Some(active_path.clone()),
            history: None,
            include_archived: false,
            metadata: persistence_metadata(home.path().to_path_buf()),
        })
        .await
        .expect("resume live thread with state database");
    let pool = install_write_counters(&store).await;
    sqlx::query("UPDATE threads SET rollout_path = ? WHERE id = ?")
        .bind(stale_path.display().to_string())
        .bind(thread_id.to_string())
        .execute(&pool)
        .await
        .expect("seed stale metadata path after live resume");
    let before = runtime
        .get_thread(thread_id)
        .await
        .expect("read stale metadata")
        .expect("thread metadata");
    let updated_at = fixed_timestamp("2030-01-03T14:05:00Z");

    update_timestamp(&store, thread_id, updated_at).await;

    let actual = runtime
        .get_thread(thread_id)
        .await
        .expect("read repaired metadata")
        .expect("thread metadata");
    let mut expected = before;
    expected.rollout_path = active_path;
    expected.updated_at = updated_at;
    assert_eq!(actual, expected);
    assert_eq!(write_count(&pool, "timestamp").await, 1);
    assert_eq!(write_count(&pool, "full").await, 1);
    store
        .shutdown_thread(thread_id)
        .await
        .expect("shutdown resumed live writer");
    pool.close().await;
}

#[tokio::test]
async fn touch_that_affects_no_row_falls_back_to_metadata_upsert() {
    let home = TempDir::new().expect("temp dir");
    let uuid = Uuid::from_u128(560);
    let (store, runtime, thread_id, _) =
        store_with_existing_thread(&home, uuid, ThreadHistoryMode::Legacy).await;
    let before = runtime
        .get_thread(thread_id)
        .await
        .expect("read metadata before update")
        .expect("thread metadata");
    let pool = install_write_counters(&store).await;
    sqlx::query(
        "CREATE TRIGGER test_delete_before_timestamp_touch BEFORE UPDATE OF updated_at_ms ON threads BEGIN DELETE FROM threads WHERE id = OLD.id; SELECT RAISE(IGNORE); END",
    )
    .execute(&pool)
    .await
    .expect("install no-row touch trigger");
    let updated_at = fixed_timestamp("2030-01-03T14:06:00Z");

    update_timestamp(&store, thread_id, updated_at).await;

    let actual = runtime
        .get_thread(thread_id)
        .await
        .expect("read repaired metadata")
        .expect("fallback upsert should restore row");
    let mut expected: ThreadMetadata = before;
    // The failed touch consumes the requested updated_at millisecond; the fallback upsert
    // advances updated_at and recency_at once in their respective hot buckets.
    expected.updated_at = updated_at + chrono::Duration::milliseconds(1);
    expected.recency_at += chrono::Duration::milliseconds(1);
    assert_eq!(actual, expected);
    assert_eq!(write_count(&pool, "timestamp").await, 0);
    assert_eq!(write_count(&pool, "full").await, 1);
    pool.close().await;
}

#[tokio::test]
async fn timestamp_touch_errors_follow_required_and_best_effort_policies() {
    for (index, history_mode) in [ThreadHistoryMode::Legacy, ThreadHistoryMode::Paginated]
        .into_iter()
        .enumerate()
    {
        for require_sqlite_write in [false, true] {
            let home = TempDir::new().expect("temp dir");
            let required_offset = if require_sqlite_write { 1 } else { 0 };
            let uuid = Uuid::from_u128(570 + index as u128 * 2 + required_offset);
            let (store, runtime, thread_id, _) =
                store_with_existing_thread(&home, uuid, history_mode).await;
            let before = runtime
                .get_thread(thread_id)
                .await
                .expect("read metadata before update")
                .expect("thread metadata");
            let pool = install_write_counters(&store).await;
            sqlx::query(
                "CREATE TRIGGER test_fail_timestamp_touch BEFORE UPDATE OF updated_at_ms ON threads BEGIN SELECT RAISE(ABORT, 'synthetic timestamp touch failure'); END",
            )
            .execute(&pool)
            .await
            .expect("install timestamp touch failure trigger");
            let updated_at = fixed_timestamp("2030-01-03T14:09:00Z");

            if require_sqlite_write {
                store
                    .stage_pending_thread_metadata(
                        thread_id,
                        ThreadMetadataPatch {
                            updated_at: Some(updated_at),
                            ..Default::default()
                        },
                    )
                    .await
                    .expect("stage timestamp-only patch");
                let error = store
                    .update_thread_metadata(UpdateThreadMetadataParams {
                        thread_id,
                        patch: ThreadMetadataPatch::default(),
                        include_archived: false,
                    })
                    .await
                    .expect_err("staged timestamp write failures are required");
                assert!(matches!(error, crate::ThreadStoreError::Internal { .. }));
                store
                    .stage_pending_thread_metadata(
                        thread_id,
                        ThreadMetadataPatch {
                            model_provider: Some("still-staged".to_string()),
                            ..Default::default()
                        },
                    )
                    .await
                    .expect_err("failed required update must retain pending metadata");
                store
                    .remove_pending_thread_metadata(thread_id)
                    .await
                    .expect("remove retained pending metadata");
            } else {
                update_timestamp(&store, thread_id, updated_at).await;
            }

            let after = runtime
                .get_thread(thread_id)
                .await
                .expect("read metadata after timestamp touch failure")
                .expect("thread metadata remains readable");
            assert_eq!(after, before);
            assert_eq!(write_count(&pool, "timestamp").await, 0);
            assert_eq!(write_count(&pool, "full").await, 0);
            pool.close().await;
        }
    }
}

#[test]
fn timestamp_predicate_excludes_clearable_and_recency_fields() {
    let empty = ThreadMetadataPatch::default();
    assert!(empty.is_empty());
    assert!(empty.is_empty_except_updated_at());

    let updated_only = ThreadMetadataPatch {
        updated_at: Some(fixed_timestamp("2030-01-03T14:07:00Z")),
        ..Default::default()
    };
    assert!(!updated_only.is_empty());
    assert!(updated_only.is_empty_except_updated_at());

    let clear_name = ThreadMetadataPatch {
        name: Some(None),
        ..Default::default()
    };
    assert!(!clear_name.is_empty_except_updated_at());

    let advance_recency = ThreadMetadataPatch {
        advance_recency_at: Some(fixed_timestamp("2030-01-03T14:08:00Z")),
        ..Default::default()
    };
    assert!(!advance_recency.is_empty_except_updated_at());
}
