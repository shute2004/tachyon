#![cfg(target_os = "windows")]

use super::WindowsSandboxSessionRequest;
use super::backends::legacy::LegacySessionSpawnRequest;
use super::backends::legacy::spawn_windows_sandbox_session_legacy_with_observer;
use super::spawn_windows_sandbox_session_elevated_for_permission_profile;
use super::spawn_windows_sandbox_session_for_level;
use super::spawn_windows_sandbox_session_legacy;
use crate::ipc_framed::Message;
use crate::ipc_framed::decode_bytes;
use crate::ipc_framed::read_frame;
use crate::run_windows_sandbox_capture;
use crate::run_windows_sandbox_capture_with_filesystem_overrides;
use codex_protocol::config_types::WindowsSandboxLevel;
use codex_protocol::models::PermissionProfile;
use codex_utils_absolute_path::AbsolutePathBuf;
use codex_utils_pty::ProcessDriver;
use pretty_assertions::assert_eq;
use std::collections::HashMap;
use std::fs;
use std::fs::OpenOptions;
use std::io::Seek;
use std::io::SeekFrom;
use std::path::Path;
use std::path::PathBuf;
use std::sync::Mutex;
use std::sync::MutexGuard;
use std::sync::atomic::AtomicU64;
use std::sync::atomic::Ordering;
use std::time::Duration;
use std::time::Instant;
use tempfile::TempDir;
use tokio::runtime::Builder;
use tokio::sync::broadcast;
use tokio::sync::mpsc;
use tokio::sync::oneshot;
use tokio::time::timeout;

static TEST_HOME_COUNTER: AtomicU64 = AtomicU64::new(0);
static LEGACY_PROCESS_TEST_LOCK: Mutex<()> = Mutex::new(());

fn legacy_process_test_guard() -> MutexGuard<'static, ()> {
    LEGACY_PROCESS_TEST_LOCK
        .lock()
        .expect("legacy Windows sandbox process test lock poisoned")
}

fn current_thread_runtime() -> tokio::runtime::Runtime {
    Builder::new_current_thread()
        .enable_all()
        .build()
        .expect("build tokio runtime")
}

fn sandbox_cwd() -> PathBuf {
    if let Ok(workspace_root) = std::env::var("INSTA_WORKSPACE_ROOT") {
        return PathBuf::from(workspace_root);
    }

    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .expect("repo root")
        .to_path_buf()
}

fn sandbox_home(name: &str) -> TempDir {
    let id = TEST_HOME_COUNTER.fetch_add(1, Ordering::Relaxed);
    let path = std::env::temp_dir().join(format!("codex-windows-sandbox-{name}-{id}"));
    let _ = fs::remove_dir_all(&path);
    fs::create_dir_all(&path).expect("create sandbox home");
    tempfile::TempDir::new_in(&path).expect("create sandbox home tempdir")
}

fn sandbox_log(codex_home: &Path) -> String {
    let log_path = crate::current_log_file_path(&codex_home.join(".sandbox"));
    fs::read_to_string(&log_path)
        .unwrap_or_else(|err| format!("failed to read {}: {err}", log_path.display()))
}

fn workspace_roots_for(root: &Path) -> Vec<AbsolutePathBuf> {
    vec![AbsolutePathBuf::from_absolute_path(root).expect("absolute workspace root")]
}

fn wait_for_frame_count(frames_path: &Path, expected_frames: usize) -> Vec<Message> {
    let deadline = Instant::now() + Duration::from_secs(2);
    loop {
        let mut reader = OpenOptions::new()
            .read(true)
            .open(frames_path)
            .expect("open frame file for read");
        reader
            .seek(SeekFrom::Start(0))
            .expect("seek to start of frame file");

        let mut frames = Vec::new();
        loop {
            match read_frame(&mut reader) {
                Ok(Some(frame)) => frames.push(frame.message),
                Ok(None) => break,
                Err(_) => break,
            }
        }

        if frames.len() >= expected_frames {
            return frames;
        }
        assert!(
            Instant::now() < deadline,
            "timed out waiting for {expected_frames} frames, saw {}",
            frames.len()
        );
        std::thread::sleep(Duration::from_millis(10));
    }
}

async fn collect_stdout_and_exit(
    spawned: codex_utils_pty::SpawnedProcess,
    codex_home: &Path,
    timeout_duration: Duration,
) -> (Vec<u8>, i32) {
    let codex_utils_pty::SpawnedProcess {
        session: _session,
        mut stdout_rx,
        stderr_rx: _stderr_rx,
        exit_rx,
    } = spawned;
    let stdout_task = tokio::spawn(async move {
        let mut stdout = Vec::new();
        while let Some(chunk) = stdout_rx.recv().await {
            stdout.extend(chunk);
        }
        stdout
    });
    let exit_code = timeout(timeout_duration, exit_rx)
        .await
        .unwrap_or_else(|_| panic!("timed out waiting for exit\n{}", sandbox_log(codex_home)))
        .unwrap_or(-1);
    let stdout = timeout(timeout_duration, stdout_task)
        .await
        .unwrap_or_else(|_| {
            panic!(
                "timed out waiting for stdout task\n{}",
                sandbox_log(codex_home)
            )
        })
        .expect("stdout task join");
    (stdout, exit_code)
}

const LEGACY_BACKEND_DISABLED_ERROR: &str = "The same-user legacy Windows sandbox backend is disabled because it cannot enforce filesystem deletion boundaries; use the elevated Windows sandbox backend";

fn assert_legacy_backend_rejected(
    api: &str,
    error: anyhow::Error,
    launch_marker: &Path,
    outside_file: &Path,
    codex_home: &Path,
) {
    assert_eq!(
        error.to_string(),
        LEGACY_BACKEND_DISABLED_ERROR,
        "{api}: {error:#}"
    );
    assert!(
        !launch_marker.exists(),
        "{api}: launch marker should remain absent"
    );
    assert_eq!(
        fs::read(outside_file).expect("read scratch outside file"),
        b"outside bytes".to_vec(),
        "{api}: sandbox command should not delete the scratch outside file"
    );
    assert!(
        !codex_home.exists(),
        "{api}: rejected launch should not create codex_home"
    );
}

#[test]
fn restricted_token_rejects_managed_network_before_spawn() {
    current_thread_runtime().block_on(async {
        let cwd = sandbox_cwd();
        let codex_home = sandbox_home("restricted-token-managed-network");
        let permission_profile = PermissionProfile::workspace_write();
        let error = spawn_windows_sandbox_session_for_level(WindowsSandboxSessionRequest {
            permission_profile: &permission_profile,
            workspace_roots: &[],
            codex_home: codex_home.path(),
            command: Vec::new(),
            cwd: cwd.as_path(),
            env_map: HashMap::new(),
            windows_sandbox_level: WindowsSandboxLevel::RestrictedToken,
            proxy_enforced: true,
            network_proxy_restricting_sid: None,
            proxy_settings_mode: crate::WindowsSandboxProxySettingsMode::Preserve,
            timeout_ms: None,
            read_roots_override: None,
            read_roots_include_platform_defaults: false,
            write_roots_override: None,
            deny_read_paths_override: &[],
            deny_write_paths_override: &[],
            tty: false,
            stdin_open: false,
            use_private_desktop: false,
        })
        .await
        .expect_err("managed networking must fail before spawning an unelevated sandbox");

        assert_eq!(
            error.to_string(),
            "managed networking requires the elevated Windows sandbox backend"
        );
    });
}

#[test]
fn legacy_launch_apis_reject_before_launch_or_mutation() {
    let runtime = current_thread_runtime();
    let tempdir = TempDir::new().expect("create legacy rejection fixture");
    let workspace = tempdir.path().join("workspace");
    fs::create_dir_all(&workspace).expect("create fixture workspace");
    let script_path = workspace.join("would-mutate.cmd");
    let outside_file = tempdir.path().join("scratch-outside.txt");
    let launch_marker = tempdir.path().join("launch-marker.txt");
    let codex_home = tempdir.path().join("absent-codex-home");
    fs::write(&outside_file, b"outside bytes").expect("seed scratch outside file");
    fs::write(
        &script_path,
        format!(
            "@echo off\r\necho launched > \"{}\"\r\ndel /f /q \"{}\"\r\n",
            launch_marker.display(),
            outside_file.display()
        ),
    )
    .expect("write guarded launch script");
    assert!(
        !codex_home.exists(),
        "codex_home fixture should start absent"
    );

    let workspace_roots = workspace_roots_for(workspace.as_path());
    let command = || {
        vec![
            "C:\\Windows\\System32\\cmd.exe".to_string(),
            "/d".to_string(),
            "/c".to_string(),
            format!("\"{}\"", script_path.display()),
        ]
    };

    for (profile_name, permission_profile) in [
        ("read-only", PermissionProfile::read_only()),
        ("workspace-write", PermissionProfile::workspace_write()),
    ] {
        for tty in [false, true] {
            let api = format!("{profile_name} direct legacy session (tty={tty})");
            let error = runtime
                .block_on(spawn_windows_sandbox_session_legacy(
                    &permission_profile,
                    &workspace_roots,
                    &codex_home,
                    command(),
                    workspace.as_path(),
                    HashMap::new(),
                    Some(5_000),
                    &[],
                    &[],
                    tty,
                    /*stdin_open*/ false,
                    /*use_private_desktop*/ false,
                ))
                .err()
                .expect("direct legacy session API should reject before spawning");
            assert_legacy_backend_rejected(&api, error, &launch_marker, &outside_file, &codex_home);
        }

        let api = format!("{profile_name} RestrictedToken level selection");
        let error = runtime
            .block_on(spawn_windows_sandbox_session_for_level(
                WindowsSandboxSessionRequest {
                    permission_profile: &permission_profile,
                    workspace_roots: &workspace_roots,
                    codex_home: &codex_home,
                    command: command(),
                    cwd: workspace.as_path(),
                    env_map: HashMap::new(),
                    windows_sandbox_level: WindowsSandboxLevel::RestrictedToken,
                    proxy_enforced: false,
                    network_proxy_restricting_sid: None,
                    proxy_settings_mode: crate::WindowsSandboxProxySettingsMode::Preserve,
                    timeout_ms: Some(5_000),
                    read_roots_override: None,
                    read_roots_include_platform_defaults: false,
                    write_roots_override: None,
                    deny_read_paths_override: &[],
                    deny_write_paths_override: &[],
                    tty: false,
                    stdin_open: false,
                    use_private_desktop: false,
                },
            ))
            .err()
            .expect("RestrictedToken level should reject before spawning");
        assert_legacy_backend_rejected(&api, error, &launch_marker, &outside_file, &codex_home);

        let api = format!("{profile_name} legacy capture");
        let error = run_windows_sandbox_capture(
            &permission_profile,
            &workspace_roots,
            &codex_home,
            command(),
            workspace.as_path(),
            HashMap::new(),
            Some(5_000),
            /*cancellation*/ None,
            /*use_private_desktop*/ false,
        )
        .err()
        .expect("legacy capture API should reject before launching");
        assert_legacy_backend_rejected(&api, error, &launch_marker, &outside_file, &codex_home);

        let api = format!("{profile_name} legacy capture with filesystem overrides");
        let error = run_windows_sandbox_capture_with_filesystem_overrides(
            &permission_profile,
            &workspace_roots,
            &codex_home,
            command(),
            workspace.as_path(),
            HashMap::new(),
            Some(5_000),
            /*cancellation*/ None,
            &[],
            &[],
            /*use_private_desktop*/ false,
        )
        .err()
        .expect("legacy capture override API should reject before launching");
        assert_legacy_backend_rejected(&api, error, &launch_marker, &outside_file, &codex_home);

        let observer_called = std::cell::Cell::new(false);
        let request = LegacySessionSpawnRequest::new(
            &permission_profile,
            &workspace_roots,
            &codex_home,
            command(),
            workspace.as_path(),
            HashMap::new(),
            Some(5_000),
            &[],
            &[],
            /*tty*/ false,
            /*stdin_open*/ false,
            /*use_private_desktop*/ false,
        );
        let api = format!("{profile_name} legacy session with observer");
        let error = runtime
            .block_on(spawn_windows_sandbox_session_legacy_with_observer(
                request,
                |_, _| observer_called.set(true),
            ))
            .err()
            .expect("observer legacy API should reject before launching");
        assert!(
            !observer_called.get(),
            "{api}: launch observer must not run"
        );
        assert_legacy_backend_rejected(&api, error, &launch_marker, &outside_file, &codex_home);
    }
}

#[test]
fn elevated_non_tty_cmd_forwards_env_output_and_exit() {
    let _guard = legacy_process_test_guard();
    let runtime = current_thread_runtime();
    runtime.block_on(async move {
        let cwd = sandbox_cwd();
        let codex_home = sandbox_home("elevated-non-tty-cmd");
        let permission_profile = PermissionProfile::workspace_write();
        let env_map = HashMap::from([(
            "CODEX_ELEVATED_TEST".to_string(),
            "ELEVATED-ENV-OK".to_string(),
        )]);
        let spawned = spawn_windows_sandbox_session_elevated_for_permission_profile(
            &permission_profile,
            workspace_roots_for(cwd.as_path()).as_slice(),
            codex_home.path(),
            vec![
                "C:\\Windows\\System32\\cmd.exe".to_string(),
                "/d".to_string(),
                "/c".to_string(),
                "echo %CODEX_ELEVATED_TEST% & exit /b 23".to_string(),
            ],
            cwd.as_path(),
            env_map,
            /*proxy_enforced*/ false,
            /*network_proxy_restricting_sid*/ None,
            Some(5_000),
            /*read_roots_override*/ None,
            /*read_roots_include_platform_defaults*/ true,
            /*write_roots_override*/ None,
            &[],
            &[],
            /*tty*/ false,
            /*stdin_open*/ false,
            /*use_private_desktop*/ true,
        )
        .await
        .expect("spawn elevated non-tty cmd session");
        let (stdout, exit_code) =
            collect_stdout_and_exit(spawned, codex_home.path(), Duration::from_secs(10)).await;
        let stdout = String::from_utf8_lossy(&stdout);
        assert_eq!(exit_code, 23, "stdout={stdout:?}");
        assert!(stdout.contains("ELEVATED-ENV-OK"), "stdout={stdout:?}");
    });
}

#[test]
fn finish_driver_spawn_keeps_stdin_open_when_requested() {
    let runtime = current_thread_runtime();
    runtime.block_on(async move {
        let (writer_tx, mut writer_rx) = mpsc::channel::<Vec<u8>>(1);
        let (_stdout_tx, stdout_rx) = broadcast::channel::<Vec<u8>>(1);
        let (exit_tx, exit_rx) = oneshot::channel::<i32>();
        drop(exit_tx);

        let spawned = super::finish_driver_spawn(
            ProcessDriver {
                writer_tx,
                stdout_rx,
                stderr_rx: None,
                exit_rx,
                terminator: None,
                writer_handle: None,
                resizer: None,
                tty: false,
            },
            /*stdin_open*/ true,
        );

        spawned
            .session
            .writer_sender()
            .send(b"open".to_vec())
            .await
            .expect("stdin should stay open");
        assert_eq!(writer_rx.recv().await, Some(b"open".to_vec()));
    });
}

#[test]
fn finish_driver_spawn_closes_stdin_when_not_requested() {
    let runtime = current_thread_runtime();
    runtime.block_on(async move {
        let (writer_tx, _writer_rx) = mpsc::channel::<Vec<u8>>(1);
        let (_stdout_tx, stdout_rx) = broadcast::channel::<Vec<u8>>(1);
        let (exit_tx, exit_rx) = oneshot::channel::<i32>();
        drop(exit_tx);

        let spawned = super::finish_driver_spawn(
            ProcessDriver {
                writer_tx,
                stdout_rx,
                stderr_rx: None,
                exit_rx,
                terminator: None,
                writer_handle: None,
                resizer: None,
                tty: false,
            },
            /*stdin_open*/ false,
        );

        assert!(
            spawned
                .session
                .writer_sender()
                .send(b"closed".to_vec())
                .await
                .is_err(),
            "stdin should be closed when streaming input is disabled"
        );
    });
}

#[test]
fn runner_stdin_writer_sends_close_stdin_after_input_eof() {
    let runtime = current_thread_runtime();
    runtime.block_on(async move {
        let tempdir = TempDir::new().expect("create tempdir");
        let frames_path = tempdir.path().join("runner-stdin-frames.bin");
        let file = OpenOptions::new()
            .create(true)
            .truncate(true)
            .read(true)
            .write(true)
            .open(&frames_path)
            .expect("create frame file");
        let outbound_tx = super::start_runner_pipe_writer(file);
        let (writer_tx, writer_rx) = mpsc::channel::<Vec<u8>>(1);
        let writer_handle = super::start_runner_stdin_writer(
            writer_rx,
            outbound_tx,
            /*normalize_newlines*/ false,
            /*stdin_open*/ true,
        );

        writer_tx
            .send(b"hello".to_vec())
            .await
            .expect("send stdin bytes");
        drop(writer_tx);
        writer_handle.await.expect("join stdin writer");

        let frames = wait_for_frame_count(&frames_path, 2);

        match &frames[0] {
            Message::Stdin { payload } => {
                let bytes = decode_bytes(&payload.data_b64).expect("decode stdin payload");
                assert_eq!(bytes, b"hello".to_vec());
            }
            other => panic!("expected stdin frame, got {other:?}"),
        }

        match &frames[1] {
            Message::CloseStdin { .. } => {}
            other => panic!("expected close-stdin frame, got {other:?}"),
        }
    });
}

#[test]
fn runner_resizer_sends_resize_frame() {
    let runtime = current_thread_runtime();
    runtime.block_on(async move {
        let tempdir = TempDir::new().expect("create tempdir");
        let frames_path = tempdir.path().join("runner-resize-frames.bin");
        let file = OpenOptions::new()
            .create(true)
            .truncate(true)
            .read(true)
            .write(true)
            .open(&frames_path)
            .expect("create frame file");
        let outbound_tx = super::start_runner_pipe_writer(file);
        let mut resizer = super::make_runner_resizer(outbound_tx);

        resizer(codex_utils_pty::TerminalSize {
            rows: 45,
            cols: 132,
        })
        .expect("send resize frame");

        let frames = wait_for_frame_count(&frames_path, 1);
        match &frames[0] {
            Message::Resize { payload } => {
                assert_eq!(payload.rows, 45);
                assert_eq!(payload.cols, 132);
            }
            other => panic!("expected resize frame, got {other:?}"),
        }
    });
}
