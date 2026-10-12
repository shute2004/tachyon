use std::future::pending;
use std::future::ready;
use std::io;
use std::io::Write as _;
use std::sync::Arc;

use pretty_assertions::assert_eq;
use tokio::sync::oneshot;

use super::FileReadHandleManager;
use super::MAX_OPEN_FILE_READS;

fn test_file(contents: &[u8]) -> std::fs::File {
    let mut file = tempfile::tempfile().expect("temporary file");
    file.write_all(contents).expect("write temporary file");
    file
}

#[tokio::test]
async fn stalled_open_does_not_block_other_opens_or_reads() {
    let manager = FileReadHandleManager::default();
    let mut stalled = Box::pin(manager.open(
        "stalled".to_string(),
        pending::<io::Result<tokio::fs::File>>(),
    ));
    assert!(futures::poll!(stalled.as_mut()).is_pending());

    let file = tokio::fs::File::from_std(test_file(b"other"));
    let mut unrelated = Box::pin(manager.open("other".to_string(), ready(Ok(file))));
    assert!(matches!(
        futures::poll!(unrelated.as_mut()),
        std::task::Poll::Ready(Ok(_))
    ));
    let block = manager
        .read_block("other", 0, 8)
        .await
        .expect("unrelated file should be readable");
    assert_eq!(block.bytes, b"other");

    drop(stalled);
    manager.close_all().await;
}

#[tokio::test]
async fn pending_opens_are_bounded_before_the_open_future_is_polled() {
    let manager = FileReadHandleManager::default();
    let mut pending_opens = (0..MAX_OPEN_FILE_READS)
        .map(|index| {
            Box::pin(manager.open(
                format!("pending-{index}"),
                pending::<io::Result<tokio::fs::File>>(),
            ))
        })
        .collect::<Vec<_>>();
    for open in &mut pending_opens {
        assert!(futures::poll!(open.as_mut()).is_pending());
    }

    let mut overflow = Box::pin(manager.open(
        "overflow".to_string(),
        pending::<io::Result<tokio::fs::File>>(),
    ));
    assert!(matches!(
        futures::poll!(overflow.as_mut()),
        std::task::Poll::Ready(Err(error)) if error.kind() == io::ErrorKind::InvalidInput
    ));

    drop(overflow);
    drop(pending_opens);
}

#[tokio::test]
async fn cancelled_and_failed_opens_release_permits() {
    let manager = FileReadHandleManager::default();

    for index in 0..=MAX_OPEN_FILE_READS {
        let mut open = Box::pin(manager.open(
            format!("cancelled-{index}"),
            pending::<io::Result<tokio::fs::File>>(),
        ));
        assert!(futures::poll!(open.as_mut()).is_pending());
        drop(open);
    }

    for index in 0..=MAX_OPEN_FILE_READS {
        let error = manager
            .open(
                format!("failed-{index}"),
                ready(Err(io::Error::other("expected open failure"))),
            )
            .await
            .expect_err("the open future should fail");
        assert_eq!(error.kind(), io::ErrorKind::Other);
    }
}

#[tokio::test]
async fn close_releases_capacity_while_a_reader_retains_the_file() {
    let manager = FileReadHandleManager::default();
    let source = test_file(b"held");

    for index in 0..MAX_OPEN_FILE_READS {
        let handle_id = if index == 0 {
            "held".to_string()
        } else {
            format!("occupied-{index}")
        };
        let file = tokio::fs::File::from_std(source.try_clone().expect("clone test file"));
        manager
            .open(handle_id, ready(Ok(file)))
            .await
            .expect("all open slots should be usable");
    }

    let retained_file = {
        let handles = manager.handles.lock().await;
        Arc::clone(&handles.get("held").expect("held handle").file)
    };
    let error = manager
        .open(
            "overflow".to_string(),
            pending::<io::Result<tokio::fs::File>>(),
        )
        .await
        .expect_err("all permits should be held before close");
    assert_eq!(error.kind(), io::ErrorKind::InvalidInput);

    manager.close("held").await;
    let file = tokio::fs::File::from_std(source.try_clone().expect("clone test file"));
    manager
        .open("replacement".to_string(), ready(Ok(file)))
        .await
        .expect("close should free capacity while the file Arc is retained");
    let block = super::read_block_at(&retained_file, 0, 4).expect("retained file remains open");
    assert_eq!(block.bytes, b"held");
    manager.close_all().await;
}

#[tokio::test]
async fn duplicate_in_flight_ids_cannot_replace_the_winning_file() {
    let manager = FileReadHandleManager::default();
    let late_file = test_file(b"late");
    let (release_tx, release_rx) = oneshot::channel();
    let mut late = Box::pin(manager.open("duplicate".to_string(), async move {
        release_rx
            .await
            .map_err(|_| io::Error::other("late open was cancelled"))?;
        Ok(tokio::fs::File::from_std(late_file))
    }));
    assert!(futures::poll!(late.as_mut()).is_pending());

    let first_file = tokio::fs::File::from_std(test_file(b"first"));
    assert_eq!(
        manager
            .open("duplicate".to_string(), ready(Ok(first_file)))
            .await
            .expect("first open should win"),
        "duplicate"
    );
    release_tx.send(()).expect("release late open");
    let error = late.await.expect_err("late duplicate should be rejected");
    assert_eq!(error.kind(), io::ErrorKind::InvalidInput);

    let block = manager
        .read_block("duplicate", 0, 8)
        .await
        .expect("winning handle should remain readable");
    assert_eq!(block.bytes, b"first");
    manager.close_all().await;
}
