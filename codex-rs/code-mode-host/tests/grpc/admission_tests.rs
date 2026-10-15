//! Initial admission may retry transient failures; established work must not replay.
use std::sync::atomic::AtomicUsize;
use std::sync::atomic::Ordering;

use super::*;

#[tokio::test]
async fn session_admission_retries_only_transient_failures_and_is_bounded() -> Result<()> {
    for (code, failures, expected_attempts, succeeds) in [
        (Code::Unavailable, 1, 2, true),
        (Code::ResourceExhausted, 1, 2, true),
        (Code::Unavailable, usize::MAX, 3, false),
        // A trailers-only OK response is an empty successful lease, which admission maps to Unavailable.
        (Code::Ok, 1, 2, true),
        (Code::Ok, usize::MAX, 3, false),
        (Code::InvalidArgument, 1, 1, false),
        (Code::Unknown, 1, 1, false),
    ] {
        let open_attempts = Arc::new(AtomicUsize::new(0));
        let execute_attempts = Arc::new(AtomicUsize::new(0));
        let open_count = Arc::clone(&open_attempts);
        let execute_count = Arc::clone(&execute_attempts);
        let app = axum::Router::new()
            .fallback_service(grpc::code_mode_host_server::CodeModeHostServer::new(
                codex_code_mode_host::GrpcCodeModeHost::new(),
            ))
            .layer(axum::middleware::from_fn(
                move |request: axum::extract::Request, next: axum::middleware::Next| {
                    let open_count = Arc::clone(&open_count);
                    let execute_count = Arc::clone(&execute_count);
                    async move {
                        if request.uri().path().ends_with("/OpenSession")
                            && open_count.fetch_add(1, Ordering::SeqCst) < failures
                        {
                            return tonic::Status::new(code, "transient admission test")
                                .into_http::<axum::body::Body>();
                        }
                        if request.uri().path().ends_with("/Execute") {
                            execute_count.fetch_add(1, Ordering::SeqCst);
                        }
                        next.run(request).await
                    }
                },
            ));
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await?;
        let address = listener.local_addr()?;
        let server = tokio_util::task::AbortOnDropHandle::new(tokio::spawn(async move {
            axum::serve(listener, app)
                .await
                .expect("admission test server");
        }));
        let provider = GrpcCodeModeSessionProvider::new(format!("http://{address}"));
        let result = timeout(
            TEST_TIMEOUT,
            provider.create_session(Arc::new(NoopCodeModeSessionDelegate)),
        )
        .await?;
        assert_eq!(
            result.is_ok(),
            succeeds,
            "status {code}: {}",
            result.as_ref().err().cloned().unwrap_or_default()
        );
        assert_eq!(open_attempts.load(Ordering::SeqCst), expected_attempts);
        if let Ok(session) = result {
            assert_eq!(
                execute(&session, request("text(42);")).await?,
                text_response("1", "42")
            );
            assert_eq!(execute_attempts.load(Ordering::SeqCst), 1);
            timeout(TEST_TIMEOUT, session.shutdown())
                .await?
                .map_err(anyhow::Error::msg)?;
        } else {
            assert_eq!(execute_attempts.load(Ordering::SeqCst), 0);
        }
        drop(server);
    }
    Ok(())
}

#[tokio::test]
async fn execute_unavailable_is_not_retried_or_reopened() -> Result<()> {
    let open_attempts = Arc::new(AtomicUsize::new(0));
    let execute_attempts = Arc::new(AtomicUsize::new(0));
    let open_count = Arc::clone(&open_attempts);
    let execute_count = Arc::clone(&execute_attempts);
    let app = axum::Router::new()
        .fallback_service(grpc::code_mode_host_server::CodeModeHostServer::new(
            codex_code_mode_host::GrpcCodeModeHost::new(),
        ))
        .layer(axum::middleware::from_fn(
            move |request: axum::extract::Request, next: axum::middleware::Next| {
                let open_count = Arc::clone(&open_count);
                let execute_count = Arc::clone(&execute_count);
                async move {
                    if request.uri().path().ends_with("/OpenSession") {
                        open_count.fetch_add(1, Ordering::SeqCst);
                    }
                    if request.uri().path().ends_with("/Execute") {
                        execute_count.fetch_add(1, Ordering::SeqCst);
                        return tonic::Status::unavailable("transient execute test")
                            .into_http::<axum::body::Body>();
                    }
                    next.run(request).await
                }
            },
        ));
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await?;
    let address = listener.local_addr()?;
    let server = tokio_util::task::AbortOnDropHandle::new(tokio::spawn(async move {
        axum::serve(listener, app)
            .await
            .expect("execution retry test server");
    }));
    let provider = GrpcCodeModeSessionProvider::new(format!("http://{address}"));
    let session = timeout(
        TEST_TIMEOUT,
        provider.create_session(Arc::new(NoopCodeModeSessionDelegate)),
    )
    .await?
    .map_err(anyhow::Error::msg)?;
    let error = execute(&session, request("text(42);"))
        .await
        .expect_err("an unavailable Execute response should fail");
    assert!(
        error.to_string().contains("transient execute test"),
        "unexpected Execute error: {error:#}"
    );
    assert_eq!(execute_attempts.load(Ordering::SeqCst), 1);
    assert_eq!(open_attempts.load(Ordering::SeqCst), 1);
    timeout(TEST_TIMEOUT, session.shutdown())
        .await?
        .map_err(anyhow::Error::msg)?;
    drop(server);
    Ok(())
}

#[tokio::test]
async fn session_admission_recovers_after_connection_reset() -> Result<()> {
    use tokio::io::AsyncReadExt;

    let execute_attempts = Arc::new(AtomicUsize::new(0));
    let execute_count = Arc::clone(&execute_attempts);
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await?;
    let address = listener.local_addr()?;
    let server = tokio_util::task::AbortOnDropHandle::new(tokio::spawn(async move {
        // Drop the first connection with unread HTTP/2 bytes to send TCP RST.
        // Serve the next connection normally; no timer or launcher mock is needed.
        let (mut stream, _) = listener.accept().await?;
        stream.read_exact(&mut [0]).await?;
        drop(stream);
        let app = axum::Router::new()
            .fallback_service(grpc::code_mode_host_server::CodeModeHostServer::new(
                codex_code_mode_host::GrpcCodeModeHost::new(),
            ))
            .layer(axum::middleware::from_fn(
                move |request: axum::extract::Request, next: axum::middleware::Next| {
                    let execute_count = Arc::clone(&execute_count);
                    async move {
                        if request.uri().path().ends_with("/Execute") {
                            execute_count.fetch_add(1, Ordering::SeqCst);
                        }
                        next.run(request).await
                    }
                },
            ));
        axum::serve(listener, app).await
    }));
    let provider = GrpcCodeModeSessionProvider::new(format!("http://{address}"));
    let session = timeout(
        TEST_TIMEOUT,
        provider.create_session(Arc::new(NoopCodeModeSessionDelegate)),
    )
    .await?
    .map_err(anyhow::Error::msg)?;
    assert_eq!(
        execute(&session, request("text(42);")).await?,
        text_response("1", "42")
    );
    assert_eq!(execute_attempts.load(Ordering::SeqCst), 1);
    timeout(TEST_TIMEOUT, session.shutdown())
        .await?
        .map_err(anyhow::Error::msg)?;
    drop(server);
    Ok(())
}
