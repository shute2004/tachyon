#[cfg(unix)]
mod unix_tests {
    use std::fs;
    use std::num::NonZeroU64;
    use std::os::unix::fs::PermissionsExt;
    use std::os::unix::fs::symlink;
    use std::path::Path;
    use std::path::PathBuf;
    use std::sync::mpsc;
    use std::time::Duration;

    use codex_protocol::ThreadId;
    use codex_protocol::models::BaseInstructions;
    use codex_protocol::protocol::SessionSource;
    use codex_protocol::protocol::ThreadHistoryMode;
    use codex_protocol::protocol::ThreadMemoryMode;
    use pretty_assertions::assert_eq;
    use tempfile::TempDir;

    use super::super::LocalThreadStore;
    use super::super::test_support::test_config;
    use crate::CreateThreadParams;
    use crate::InputIdentityContinuity;
    use crate::InputIdentityReservation;
    use crate::PersistContext;
    use crate::ResumeThreadParams;
    use crate::ThreadPersistenceMetadata;
    use crate::ThreadStore;
    use crate::ThreadStoreError;

    const JOURNAL_DIRECTORY: &str = "thread-input-identities";

    #[tokio::test]
    async fn reservation_requires_an_existing_live_writer_without_creating_journal() {
        let home = TempDir::new().expect("temp dir");
        let (store, thread_id, _) = create_live_store(home.path()).await;
        let unrelated_thread_id = ThreadId::new();

        let err = store
            .reserve_input_identity(unrelated_thread_id)
            .await
            .expect_err("missing live writer must not reserve an identity");
        assert!(
            matches!(err, ThreadStoreError::ThreadNotFound { thread_id: missing } if missing == unrelated_thread_id)
        );
        assert!(!journal_dir(home.path()).exists());

        store
            .reserve_input_identity(thread_id)
            .await
            .expect("live writer reservation");
        let canonical = canonical_path(home.path(), thread_id);
        let canonical_before = fs::read(&canonical).expect("canonical reservation");
        store
            .shutdown_thread(thread_id)
            .await
            .expect("shutdown live writer");

        let err = store
            .reserve_input_identity(thread_id)
            .await
            .expect_err("stale journal is not evidence of a live writer");
        assert!(
            matches!(err, ThreadStoreError::ThreadNotFound { thread_id: missing } if missing == thread_id)
        );
        assert_eq!(
            fs::read(&canonical).expect("canonical remains"),
            canonical_before
        );
        assert!(!pending_path(home.path(), thread_id).exists());
    }

    #[tokio::test]
    async fn identity_journal_survives_sqlite_less_restart_without_touching_rollout() {
        let home = TempDir::new().expect("temp dir");
        let (store, thread_id, rollout_path) = create_live_store(home.path()).await;
        let before = fs::read(&rollout_path).expect("read rollout before reservations");

        let first = store
            .reserve_input_identity(thread_id)
            .await
            .expect("first reservation");
        let second = store
            .reserve_input_identity(thread_id)
            .await
            .expect("second reservation");
        assert_eq!(
            first,
            expected_reservation(
                thread_id,
                first.identity.incarnation,
                1,
                InputIdentityContinuity::NewIncarnation,
            )
        );
        assert_eq!(
            second,
            expected_reservation(
                thread_id,
                first.identity.incarnation,
                2,
                InputIdentityContinuity::Continuing,
            )
        );
        assert_eq!(
            fs::read(&rollout_path).expect("rollout after reservations"),
            before
        );

        store
            .shutdown_thread(thread_id)
            .await
            .expect("shutdown initial writer");
        let resumed = LocalThreadStore::new(test_config(home.path()), None);
        resumed
            .resume_thread(ResumeThreadParams {
                thread_id,
                rollout_path: Some(rollout_path.clone()),
                history: None,
                include_archived: true,
                metadata: thread_metadata(),
            })
            .await
            .expect("resume with no SQLite state DB");
        let third = resumed
            .reserve_input_identity(thread_id)
            .await
            .expect("reservation after restart");
        assert_eq!(
            third,
            expected_reservation(
                thread_id,
                first.identity.incarnation,
                3,
                InputIdentityContinuity::Continuing,
            )
        );
        assert_eq!(fs::read(&rollout_path).expect("resumed rollout"), before);
    }

    #[tokio::test]
    async fn cloned_store_reservations_are_contiguous_and_leave_one_bounded_record() {
        let home = TempDir::new().expect("temp dir");
        let (store, thread_id, _) = create_live_store(home.path()).await;
        let count = 24_u64;
        let mut tasks = Vec::new();
        for _ in 0..count {
            let cloned_store = store.clone();
            tasks.push(tokio::spawn(async move {
                cloned_store.reserve_input_identity(thread_id).await
            }));
        }

        let mut reservations = Vec::with_capacity(count as usize);
        for task in tasks {
            reservations.push(task.await.expect("reservation task").expect("reservation"));
        }
        reservations.sort_by_key(|reservation| reservation.identity.sequence);
        let incarnation = reservations[0].identity.incarnation;
        for (index, reservation) in reservations.iter().enumerate() {
            let sequence = u64::try_from(index).expect("small index") + 1;
            let continuity = if sequence == 1 {
                InputIdentityContinuity::NewIncarnation
            } else {
                InputIdentityContinuity::Continuing
            };
            assert_eq!(
                reservation,
                &expected_reservation(thread_id, incarnation, sequence, continuity)
            );
        }

        let directory = journal_dir(home.path());
        let entries = fs::read_dir(&directory)
            .expect("journal directory")
            .collect::<Result<Vec<_>, _>>()
            .expect("journal entries");
        assert_eq!(entries.len(), 1);
        let canonical = canonical_path(home.path(), thread_id);
        assert!(fs::metadata(&canonical).expect("canonical record").len() <= 4096);
        assert!(!pending_path(home.path(), thread_id).exists());
    }

    #[tokio::test]
    async fn journal_directory_and_record_use_private_unix_modes() {
        let home = TempDir::new().expect("temp dir");
        let (store, thread_id, _) = create_live_store(home.path()).await;
        store
            .reserve_input_identity(thread_id)
            .await
            .expect("reservation");

        assert_eq!(
            fs::metadata(journal_dir(home.path()))
                .expect("journal directory")
                .permissions()
                .mode()
                & 0o777,
            0o700
        );
        assert_eq!(
            fs::metadata(canonical_path(home.path(), thread_id))
                .expect("canonical record")
                .permissions()
                .mode()
                & 0o777,
            0o600
        );
    }

    #[tokio::test]
    async fn invalid_canonical_records_and_oversized_files_are_preserved_with_pending() {
        enum InvalidRecord {
            Corrupt,
            UnsupportedVersion,
            WrongThread,
            ZeroSequence,
            Overflow,
            Oversized,
        }

        let cases = [
            InvalidRecord::Corrupt,
            InvalidRecord::UnsupportedVersion,
            InvalidRecord::WrongThread,
            InvalidRecord::ZeroSequence,
            InvalidRecord::Overflow,
            InvalidRecord::Oversized,
        ];
        for invalid in cases {
            let home = TempDir::new().expect("temp dir");
            let (store, thread_id, _) = create_live_store(home.path()).await;
            let directory = journal_dir(home.path());
            fs::create_dir(&directory).expect("journal directory");
            let canonical = canonical_path(home.path(), thread_id);
            let bytes = match invalid {
                InvalidRecord::Corrupt => b"{partial".to_vec(),
                InvalidRecord::UnsupportedVersion => record_bytes(thread_id, ThreadId::new(), 1, 2),
                InvalidRecord::WrongThread => record_bytes(ThreadId::new(), ThreadId::new(), 1, 1),
                InvalidRecord::ZeroSequence => record_bytes(thread_id, ThreadId::new(), 0, 1),
                InvalidRecord::Overflow => record_bytes(thread_id, ThreadId::new(), u64::MAX, 1),
                InvalidRecord::Oversized => vec![b'x'; 4097],
            };
            fs::write(&canonical, &bytes).expect("invalid canonical fixture");
            let pending = pending_path(home.path(), thread_id);
            fs::write(&pending, b"keep staging evidence").expect("pending fixture");

            let err = store
                .reserve_input_identity(thread_id)
                .await
                .expect_err("invalid canonical must fail closed");
            assert!(matches!(err, ThreadStoreError::Internal { .. }));
            assert_eq!(fs::read(&canonical).expect("canonical remains"), bytes);
            assert_eq!(
                fs::read(&pending).expect("pending remains"),
                b"keep staging evidence"
            );
        }
    }

    #[tokio::test]
    async fn deleted_canonical_starts_a_distinct_new_incarnation() {
        let home = TempDir::new().expect("temp dir");
        let (store, thread_id, _) = create_live_store(home.path()).await;
        let old = store
            .reserve_input_identity(thread_id)
            .await
            .expect("initial reservation");
        fs::remove_file(canonical_path(home.path(), thread_id)).expect("remove canonical");

        let replacement = store
            .reserve_input_identity(thread_id)
            .await
            .expect("new incarnation reservation");
        assert_ne!(replacement.identity.incarnation, old.identity.incarnation);
        assert_eq!(
            replacement,
            expected_reservation(
                thread_id,
                replacement.identity.incarnation,
                1,
                InputIdentityContinuity::NewIncarnation,
            )
        );
    }

    #[tokio::test]
    async fn bounded_partial_pending_scratch_is_removed_before_next_commit() {
        let home = TempDir::new().expect("temp dir");
        let (store, thread_id, _) = create_live_store(home.path()).await;
        store
            .reserve_input_identity(thread_id)
            .await
            .expect("initial reservation");
        let pending = pending_path(home.path(), thread_id);
        fs::write(&pending, b"partial record").expect("partial staging fixture");

        let next = store
            .reserve_input_identity(thread_id)
            .await
            .expect("recover and reserve");
        assert_eq!(next.identity.sequence.get(), 2);
        assert!(!pending.exists());
    }

    #[tokio::test]
    async fn pending_directory_and_symlink_fail_before_commit_and_retry_at_next_sequence() {
        #[derive(Clone, Copy)]
        enum PendingFixture {
            Directory,
            Symlink,
            Oversized,
        }

        for fixture in [
            PendingFixture::Directory,
            PendingFixture::Symlink,
            PendingFixture::Oversized,
        ] {
            let home = TempDir::new().expect("temp dir");
            let (store, thread_id, _) = create_live_store(home.path()).await;
            let first = store
                .reserve_input_identity(thread_id)
                .await
                .expect("initial reservation");
            let canonical = canonical_path(home.path(), thread_id);
            let canonical_before = fs::read(&canonical).expect("canonical bytes");
            let pending = pending_path(home.path(), thread_id);
            let target = home.path().join("pending-target");
            match fixture {
                PendingFixture::Directory => {
                    fs::create_dir(&pending).expect("pending directory");
                }
                PendingFixture::Symlink => {
                    fs::write(&target, b"target").expect("symlink target");
                    symlink(&target, &pending).expect("pending symlink");
                }
                PendingFixture::Oversized => {
                    fs::write(&pending, vec![b'x'; 4097]).expect("oversized pending fixture");
                }
            }

            let err = store
                .reserve_input_identity(thread_id)
                .await
                .expect_err("unexpected pending type must fail");
            assert!(matches!(err, ThreadStoreError::Internal { .. }));
            assert_eq!(
                fs::read(&canonical).expect("canonical unchanged"),
                canonical_before
            );
            assert!(fs::symlink_metadata(&pending).is_ok());
            if matches!(fixture, PendingFixture::Oversized) {
                assert_eq!(
                    fs::read(&pending).expect("oversized pending remains"),
                    vec![b'x'; 4097]
                );
            }

            match fixture {
                PendingFixture::Directory => {
                    fs::remove_dir(&pending).expect("remove pending directory fixture");
                }
                PendingFixture::Symlink | PendingFixture::Oversized => {
                    fs::remove_file(&pending).expect("remove pending file fixture");
                }
            }
            let next = store
                .reserve_input_identity(thread_id)
                .await
                .expect("retry after fixture removal");
            assert_eq!(next.identity.sequence.get(), 2);
            assert_eq!(next.identity.incarnation, first.identity.incarnation);
        }
    }

    #[tokio::test]
    async fn observed_journal_root_and_canonical_symlinks_are_rejected() {
        let home = TempDir::new().expect("temp dir");
        let (store, thread_id, _) = create_live_store(home.path()).await;
        let journal = journal_dir(home.path());
        let external = home.path().join("external-journal");
        fs::create_dir(&external).expect("external directory");
        symlink(&external, &journal).expect("journal root symlink");

        let err = store
            .reserve_input_identity(thread_id)
            .await
            .expect_err("journal root symlink must be rejected");
        assert!(matches!(err, ThreadStoreError::Internal { .. }));
        assert!(
            fs::symlink_metadata(&journal)
                .expect("root symlink remains")
                .file_type()
                .is_symlink()
        );
        assert_eq!(
            fs::read_dir(&external).expect("external directory").count(),
            0
        );

        fs::remove_file(&journal).expect("remove root symlink fixture");
        let initial = store
            .reserve_input_identity(thread_id)
            .await
            .expect("initial canonical");
        let canonical = canonical_path(home.path(), thread_id);
        let target = home.path().join("canonical-target");
        fs::write(&target, b"target").expect("canonical symlink target");
        fs::remove_file(&canonical).expect("remove canonical fixture");
        symlink(&target, &canonical).expect("canonical symlink");
        let pending = pending_path(home.path(), thread_id);
        fs::write(&pending, b"preserve pending").expect("pending fixture");

        let err = store
            .reserve_input_identity(thread_id)
            .await
            .expect_err("canonical symlink must be rejected");
        assert!(matches!(err, ThreadStoreError::Internal { .. }));
        assert_eq!(fs::read(&target).expect("target unchanged"), b"target");
        assert_eq!(
            fs::read(&pending).expect("pending unchanged"),
            b"preserve pending"
        );
        assert!(
            fs::symlink_metadata(&canonical)
                .expect("canonical symlink remains")
                .file_type()
                .is_symlink()
        );
        assert_eq!(initial.identity.sequence.get(), 1);
    }

    #[test]
    fn cancelling_a_queued_reservation_keeps_writer_serialized_and_allows_a_gap() {
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .max_blocking_threads(1)
            .build()
            .expect("current-thread runtime");

        runtime.block_on(async {
            let home = TempDir::new().expect("temp dir");
            let (store, thread_id, rollout_path) = create_live_store(home.path()).await;
            let (started_sender, started_receiver) = mpsc::sync_channel(0);
            let (release_sender, release_receiver) = mpsc::sync_channel(0);
            let blocker = tokio::task::spawn_blocking(move || {
                started_sender
                    .send(())
                    .expect("signal blocking worker started");
                let _ = release_receiver.recv();
            });
            started_receiver
                .recv_timeout(Duration::from_secs(5))
                .expect("blocking pool occupied");
            let release_on_drop = ReleaseBlockingWorker(Some(release_sender));

            let mut reservation = Box::pin(store.reserve_input_identity(thread_id));
            assert!(futures::poll!(reservation.as_mut()).is_pending());
            drop(reservation);

            let mut shutdown = Box::pin(store.shutdown_thread(thread_id));
            assert!(futures::poll!(shutdown.as_mut()).is_pending());
            drop(release_on_drop);
            shutdown
                .await
                .expect("shutdown after queued journal completes");
            blocker.await.expect("blocking pool blocker exits");

            let resumed = LocalThreadStore::new(test_config(home.path()), None);
            resumed
                .resume_thread(ResumeThreadParams {
                    thread_id,
                    rollout_path: Some(rollout_path),
                    history: None,
                    include_archived: true,
                    metadata: thread_metadata(),
                })
                .await
                .expect("resume after cancelled reservation");
            let second = resumed
                .reserve_input_identity(thread_id)
                .await
                .expect("reservation after the permitted gap");
            assert_eq!(second.identity.sequence.get(), 2);
            assert_eq!(second.continuity, InputIdentityContinuity::Continuing);
        });
    }

    #[test]
    fn queued_reservation_retains_os_writer_lock_after_original_store_is_dropped() {
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .max_blocking_threads(1)
            .build()
            .expect("current-thread runtime");

        runtime.block_on(async {
            let home = TempDir::new().expect("temp dir");
            let (store, thread_id, rollout_path) = create_live_store(home.path()).await;
            let live_writer_locks = store.live_writer_locks.clone();
            let (started_sender, started_receiver) = mpsc::sync_channel(0);
            let (release_sender, release_receiver) = mpsc::sync_channel(0);
            let blocker = tokio::task::spawn_blocking(move || {
                started_sender
                    .send(())
                    .expect("signal blocking worker started");
                let _ = release_receiver.recv();
            });
            started_receiver
                .recv_timeout(Duration::from_secs(5))
                .expect("blocking pool occupied");
            let release_on_drop = ReleaseBlockingWorker(Some(release_sender));

            let mut reservation = Box::pin(store.reserve_input_identity(thread_id));
            assert!(futures::poll!(reservation.as_mut()).is_pending());
            drop(reservation);
            drop(store);

            let competing = LocalThreadStore::new(test_config(home.path()), None);
            let resume_params = || ResumeThreadParams {
                thread_id,
                rollout_path: Some(rollout_path.clone()),
                history: None,
                include_archived: true,
                metadata: thread_metadata(),
            };
            let conflict = competing
                .resume_thread(resume_params())
                .await
                .expect_err("queued worker must retain the OS writer lock");
            assert!(matches!(conflict, ThreadStoreError::Conflict { .. }));

            drop(release_on_drop);
            blocker.await.expect("blocking pool blocker exits");
            let reservation_finished = live_writer_locks.lock(thread_id).await;
            drop(reservation_finished);
            // The Tokio guard is released just before the closure drops its complete store clone.
            // This one-worker fence proves that final drop has also completed before resume.
            let queue_fence = tokio::task::spawn_blocking(|| ());
            queue_fence.await.expect("queued reservation has completed");

            competing
                .resume_thread(resume_params())
                .await
                .expect("writer lock is released after journal commit");
            let next = competing
                .reserve_input_identity(thread_id)
                .await
                .expect("next reservation after committed gap");
            assert_eq!(
                next,
                expected_reservation(
                    thread_id,
                    next.identity.incarnation,
                    2,
                    InputIdentityContinuity::Continuing,
                )
            );
        });
    }

    struct ReleaseBlockingWorker(Option<mpsc::SyncSender<()>>);

    impl Drop for ReleaseBlockingWorker {
        fn drop(&mut self) {
            drop(self.0.take());
        }
    }

    async fn create_live_store(codex_home: &Path) -> (LocalThreadStore, ThreadId, PathBuf) {
        let store = LocalThreadStore::new(test_config(codex_home), None);
        let thread_id = ThreadId::new();
        store
            .create_thread(CreateThreadParams {
                session_id: thread_id.into(),
                thread_id,
                extra_config: None,
                forked_from_id: None,
                parent_thread_id: None,
                source: SessionSource::Exec,
                thread_source: None,
                originator: "identity-journal-test".to_string(),
                base_instructions: BaseInstructions::default(),
                dynamic_tools: Vec::new(),
                selected_capability_roots: Vec::new(),
                multi_agent_version: None,
                history_mode: ThreadHistoryMode::Legacy,
                history_base: None,
                subagent_history_start_ordinal: None,
                initial_window_id: "identity-journal-test".to_string(),
                metadata: thread_metadata(),
            })
            .await
            .expect("create live thread");
        store
            .persist_thread(thread_id, PersistContext::Standard)
            .await
            .expect("materialize rollout before lifecycle operations");
        let rollout_path = store
            .live_rollout_path(thread_id)
            .await
            .expect("live rollout path");
        (store, thread_id, rollout_path)
    }

    fn thread_metadata() -> ThreadPersistenceMetadata {
        ThreadPersistenceMetadata {
            cwd: Some(std::env::current_dir().expect("current directory")),
            model_provider: "test-provider".to_string(),
            memory_mode: ThreadMemoryMode::Enabled,
        }
    }

    fn journal_dir(home: &Path) -> PathBuf {
        home.join(JOURNAL_DIRECTORY)
    }

    fn canonical_path(home: &Path, thread_id: ThreadId) -> PathBuf {
        journal_dir(home).join(format!("{thread_id}.json"))
    }

    fn pending_path(home: &Path, thread_id: ThreadId) -> PathBuf {
        journal_dir(home).join(format!("{thread_id}.pending"))
    }

    fn record_bytes(
        thread_id: ThreadId,
        incarnation: ThreadId,
        sequence: u64,
        version: u8,
    ) -> Vec<u8> {
        serde_json::to_vec(&serde_json::json!({
            "version": version,
            "thread_id": thread_id,
            "incarnation": incarnation,
            "sequence": sequence,
        }))
        .expect("serialize journal fixture")
    }

    fn expected_reservation(
        thread_id: ThreadId,
        incarnation: crate::InputStreamIncarnation,
        sequence: u64,
        continuity: InputIdentityContinuity,
    ) -> InputIdentityReservation {
        InputIdentityReservation {
            identity: crate::ReservedInputIdentity {
                thread_id,
                incarnation,
                sequence: NonZeroU64::new(sequence).expect("test sequence is non-zero"),
            },
            continuity,
        }
    }
}

#[cfg(not(unix))]
#[tokio::test]
async fn non_unix_reservation_is_unsupported_before_journal_io() {
    use codex_protocol::ThreadId;
    use tempfile::TempDir;

    use super::super::LocalThreadStore;
    use super::super::test_support::test_config;
    use crate::ThreadStore;
    use crate::ThreadStoreError;

    let home = TempDir::new().expect("temp dir");
    let store = LocalThreadStore::new(test_config(home.path()), None);
    let err = store
        .reserve_input_identity(ThreadId::new())
        .await
        .expect_err("non-Unix backend must remain unsupported");
    assert!(matches!(
        err,
        ThreadStoreError::Unsupported {
            operation: "reserve_input_identity"
        }
    ));
    assert!(!home.path().join("thread-input-identities").exists());
}
