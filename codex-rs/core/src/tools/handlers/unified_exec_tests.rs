use super::*;
use crate::shell::ShellType;
use crate::shell::default_user_shell;
use crate::shell::get_shell;
use codex_exec_server::Environment;
use codex_tools::UnifiedExecShellMode;
use codex_tools::ZshForkConfig;
use codex_utils_absolute_path::AbsolutePathBuf;
use codex_utils_output_truncation::TruncationPolicy;
use pretty_assertions::assert_eq;
use std::sync::Arc;

use crate::environment_selection::TurnEnvironmentState;
use crate::function_tool::FunctionCallError;
use crate::session::step_context::StepContext;
use crate::session::tests::make_session_and_context;
#[cfg(not(windows))]
use crate::session::tests::make_session_and_context_with_rx;
use crate::tools::context::ExecCommandToolOutput;
use crate::tools::context::ToolCallSource;
use crate::tools::context::ToolInvocation;
use crate::tools::context::ToolPayload;
use crate::tools::hook_names::HookToolName;
use crate::tools::registry::CoreToolRuntime;
use crate::tools::registry::ToolExecutor;
use crate::turn_diff_tracker::TurnDiffTracker;
use tokio::sync::Mutex;

const TEST_TRUNCATION_POLICY: TruncationPolicy = TruncationPolicy::Tokens(10_000);

async fn invocation_for_payload(
    tool_name: &str,
    call_id: &str,
    payload: ToolPayload,
) -> ToolInvocation {
    let (session, turn) = make_session_and_context().await;
    let turn = Arc::new(turn);
    ToolInvocation {
        session: session.into(),
        step_context: StepContext::for_test(Arc::clone(&turn)),
        turn,
        cancellation_token: tokio_util::sync::CancellationToken::new(),
        tracker: Arc::new(Mutex::new(TurnDiffTracker::new())),
        call_id: call_id.to_string(),
        tool_name: codex_tools::ToolName::plain(tool_name),
        source: ToolCallSource::Direct,
        payload,
    }
}

#[test]
fn test_get_command_uses_default_shell_when_unspecified() -> anyhow::Result<()> {
    let json = r#"{"cmd": "echo hello"}"#;

    let args: ExecCommandArgs = parse_arguments(json)?;

    assert!(args.shell.is_none());

    let resolved = get_command(
        &args,
        Arc::new(default_user_shell()),
        &UnifiedExecShellMode::Direct,
        /*allow_login_shell*/ true,
    )
    .map_err(anyhow::Error::msg)?;
    let command = resolved.command;

    assert_eq!(command.len(), 3);
    assert_eq!(command[2], "echo hello");
    Ok(())
}

#[test]
fn test_get_command_respects_explicit_bash_shell() -> anyhow::Result<()> {
    let json = r#"{"cmd": "echo hello", "shell": "/bin/bash"}"#;

    let args: ExecCommandArgs = parse_arguments(json)?;

    assert_eq!(args.shell.as_deref(), Some("/bin/bash"));

    let resolved = get_command(
        &args,
        Arc::new(default_user_shell()),
        &UnifiedExecShellMode::Direct,
        /*allow_login_shell*/ true,
    )
    .map_err(anyhow::Error::msg)?;
    let command = resolved.command;

    assert_eq!(command.last(), Some(&"echo hello".to_string()));
    if command
        .iter()
        .any(|arg| arg.eq_ignore_ascii_case("-Command"))
    {
        assert!(command.contains(&"-NoProfile".to_string()));
    }
    Ok(())
}

#[test]
fn test_get_command_resolves_powershell_by_type() -> anyhow::Result<()> {
    let temp_dir = tempfile::tempdir()?;
    let powershell_path = temp_dir.path().join(if cfg!(windows) {
        "powershell.exe"
    } else {
        "powershell"
    });
    std::fs::write(&powershell_path, "")?;
    let json = serde_json::json!({
        "cmd": "echo hello",
        "shell": powershell_path,
    })
    .to_string();

    let args: ExecCommandArgs = parse_arguments(&json)?;

    assert_eq!(
        args.shell.as_deref(),
        Some(powershell_path.to_string_lossy().as_ref())
    );

    let resolved = get_command(
        &args,
        Arc::new(default_user_shell()),
        &UnifiedExecShellMode::Direct,
        /*allow_login_shell*/ true,
    )
    .map_err(anyhow::Error::msg)?;
    let expected_shell = get_shell(ShellType::PowerShell)
        .unwrap_or_else(|| codex_shell_command::shell_detect::ultimate_fallback_shell().into());
    assert_eq!(
        resolved.command,
        expected_shell.derive_exec_args("echo hello", /*use_login_shell*/ true)
    );
    assert_eq!(resolved.shell_type, expected_shell.shell_type);
    Ok(())
}

#[test]
fn test_get_command_respects_explicit_cmd_shell() -> anyhow::Result<()> {
    let json = r#"{"cmd": "echo hello", "shell": "cmd"}"#;

    let args: ExecCommandArgs = parse_arguments(json)?;

    assert_eq!(args.shell.as_deref(), Some("cmd"));

    let resolved = get_command(
        &args,
        Arc::new(default_user_shell()),
        &UnifiedExecShellMode::Direct,
        /*allow_login_shell*/ true,
    )
    .map_err(anyhow::Error::msg)?;
    let command = resolved.command;

    assert_eq!(command[2], "echo hello");
    Ok(())
}

#[test]
fn test_get_command_rejects_explicit_login_when_disallowed() -> anyhow::Result<()> {
    let json = r#"{"cmd": "echo hello", "login": true}"#;

    let args: ExecCommandArgs = parse_arguments(json)?;
    let err = get_command(
        &args,
        Arc::new(default_user_shell()),
        &UnifiedExecShellMode::Direct,
        /*allow_login_shell*/ false,
    )
    .expect_err("explicit login should be rejected");

    assert!(
        err.contains("login shell is disabled by config"),
        "unexpected error: {err}"
    );
    Ok(())
}

#[tokio::test]
async fn exec_command_rejects_login_when_selected_environment_disallows_it() {
    let (session, mut turn) = make_session_and_context().await;
    assert!(turn.config.permissions.allow_login_shell);
    let TurnEnvironmentState::Ready(environment) = turn
        .environments
        .environments
        .first_mut()
        .expect("primary environment")
    else {
        panic!("primary environment should be ready");
    };
    environment.config_mut().allow_login_shell = false;

    let turn = Arc::new(turn);
    let invocation = ToolInvocation {
        session: session.into(),
        step_context: StepContext::for_test(Arc::clone(&turn)),
        turn,
        cancellation_token: tokio_util::sync::CancellationToken::new(),
        tracker: Arc::new(Mutex::new(TurnDiffTracker::new())),
        call_id: "login-disallowed".to_string(),
        tool_name: codex_tools::ToolName::plain("exec_command"),
        source: ToolCallSource::Direct,
        payload: ToolPayload::Function {
            arguments: serde_json::json!({ "cmd": "echo hello", "login": true }).to_string(),
        },
    };

    let Err(FunctionCallError::RespondToModel(message)) =
        ExecCommandHandler::default().handle(invocation).await
    else {
        panic!("expected login-shell rejection");
    };
    assert_eq!(
        message,
        "login shell is disabled by config; omit `login` or set it to false."
    );
}

#[test]
fn test_get_command_rejects_explicit_shell_in_zsh_fork_mode() -> anyhow::Result<()> {
    let json = r#"{"cmd": "echo hello", "shell": "/bin/bash"}"#;
    let args: ExecCommandArgs = parse_arguments(json)?;
    let shell_zsh_path = AbsolutePathBuf::from_absolute_path(if cfg!(windows) {
        r"C:\opt\codex\zsh"
    } else {
        "/opt/codex/zsh"
    })?;
    let shell_mode = UnifiedExecShellMode::ZshFork(ZshForkConfig {
        shell_zsh_path,
        main_execve_wrapper_exe: AbsolutePathBuf::from_absolute_path(if cfg!(windows) {
            r"C:\opt\codex\codex-execve-wrapper"
        } else {
            "/opt/codex/codex-execve-wrapper"
        })?,
    });

    let err = get_command(
        &args,
        Arc::new(default_user_shell()),
        &shell_mode,
        /*allow_login_shell*/ true,
    )
    .expect_err("explicit shell should be rejected");

    assert!(
        err.contains("`shell` is not supported for local zsh-fork exec"),
        "unexpected error: {err}"
    );
    Ok(())
}

#[tokio::test]
async fn shell_mode_for_environment_uses_direct_mode_for_remote_environments() -> anyhow::Result<()>
{
    let shell_zsh_path = AbsolutePathBuf::from_absolute_path(if cfg!(windows) {
        r"C:\opt\codex\zsh"
    } else {
        "/opt/codex/zsh"
    })?;
    let shell_mode = UnifiedExecShellMode::ZshFork(ZshForkConfig {
        shell_zsh_path,
        main_execve_wrapper_exe: AbsolutePathBuf::from_absolute_path(if cfg!(windows) {
            r"C:\opt\codex\codex-execve-wrapper"
        } else {
            "/opt/codex/codex-execve-wrapper"
        })?,
    });
    let local_environment = Environment::default_for_tests();
    let remote_environment =
        Environment::create_for_tests(Some("ws://127.0.0.1:1/remote-exec-server".to_string()))?;

    assert_eq!(
        shell_mode_for_environment(&shell_mode, &local_environment),
        shell_mode
    );
    assert_eq!(
        shell_mode_for_environment(&shell_mode, &remote_environment),
        UnifiedExecShellMode::Direct
    );

    Ok(())
}

#[tokio::test]
#[cfg(not(windows))]
async fn exec_permissions_use_captured_turn_and_live_session_grants() {
    use codex_protocol::models::AdditionalPermissionProfile;
    use codex_protocol::models::FileSystemPermissions;
    use codex_utils_path_uri::PathUri;

    let (session, mut turn, rx) = make_session_and_context_with_rx().await;
    let turn_mut = Arc::get_mut(&mut turn).expect("single thread settings ref");
    Arc::make_mut(&mut turn_mut.config)
        .permissions
        .approval_policy
        .set(codex_protocol::protocol::AskForApproval::OnRequest)
        .expect("test setup should allow updating approval policy");
    let turn_b = session.new_default_turn().await;

    let cwd = PathUri::parse("file:///C:/workspace").expect("valid Windows cwd");
    let granted_permissions = AdditionalPermissionProfile {
        file_system: Some(FileSystemPermissions::from_read_write_path_uris(
            /*read*/ Some(Vec::new()),
            /*write*/
            Some(vec![
                PathUri::parse("file:///C:/workspace/granted").expect("valid Windows grant"),
            ]),
        )),
        ..Default::default()
    };
    let other_turn_permissions = AdditionalPermissionProfile {
        file_system: Some(FileSystemPermissions::from_read_write_path_uris(
            /*read*/ Some(Vec::new()),
            /*write*/
            Some(vec![
                PathUri::parse("file:///C:/workspace/other-turn").expect("valid other-turn grant"),
            ]),
        )),
        ..Default::default()
    };
    turn.record_granted_permissions_for_environment(
        codex_exec_server::REMOTE_ENVIRONMENT_ID,
        granted_permissions.clone(),
        false,
    )
    .await;
    turn_b
        .record_granted_permissions_for_environment(
            codex_exec_server::REMOTE_ENVIRONMENT_ID,
            other_turn_permissions.clone(),
            false,
        )
        .await;
    *session.active_turn.lock().await = Some(crate::state::ActiveTurn::default());

    let environment = {
        let TurnEnvironmentState::Ready(environment) = turn
            .environments
            .environments
            .first()
            .expect("primary environment")
        else {
            panic!("primary environment should be ready");
        };
        let mut environment = (*environment).clone();
        environment.selection.environment_id = codex_exec_server::REMOTE_ENVIRONMENT_ID.to_string();
        environment.selection.cwd = cwd.clone();
        environment.selection.workspace_roots = vec![cwd.clone()];
        environment.config_mut().workspace_roots = vec![cwd.clone()];
        environment.environment = Arc::new(
            Environment::create_for_tests(Some("ws://127.0.0.1:1/remote-exec-server".to_string()))
                .expect("remote environment"),
        );
        environment
    };
    let effective = crate::tools::handlers::apply_granted_turn_permissions(
        &session,
        &turn,
        &environment,
        &cwd,
        crate::sandboxing::SandboxPermissions::WithAdditionalPermissions,
        Some(granted_permissions.clone()),
    )
    .await;

    assert!(effective.permissions_preapproved);
    assert_eq!(
        effective.additional_permissions,
        Some(granted_permissions.clone())
    );

    let requested_permissions = codex_protocol::request_permissions::RequestPermissionProfile {
        network: Some(codex_protocol::models::NetworkPermissions {
            enabled: Some(true),
        }),
        ..Default::default()
    };
    let expected_response = codex_protocol::request_permissions::RequestPermissionsResponse {
        permissions: requested_permissions.clone(),
        scope: codex_protocol::request_permissions::PermissionGrantScope::Session,
        strict_auto_review: false,
    };
    let mut request_environment = turn
        .environments
        .primary()
        .expect("primary environment")
        .selection();
    request_environment.environment_id = codex_exec_server::REMOTE_ENVIRONMENT_ID.to_string();
    let native_cwd = request_environment
        .cwd
        .to_abs_path()
        .expect("primary environment cwd should be native");
    let step_context = StepContext::for_test(Arc::clone(&turn));
    let call_id = "session-network-grant".to_string();
    let request_handle = tokio::spawn({
        let session = Arc::clone(&session);
        let step_context = Arc::clone(&step_context);
        let call_id = call_id.clone();
        let request_environment = request_environment.clone();
        async move {
            session
                .request_permissions_for_environment(
                    &step_context,
                    call_id,
                    codex_protocol::request_permissions::RequestPermissionsArgs {
                        environment_id: Some(codex_exec_server::REMOTE_ENVIRONMENT_ID.to_string()),
                        reason: Some("allow network access".to_string()),
                        permissions: requested_permissions,
                    },
                    request_environment,
                    tokio_util::sync::CancellationToken::new(),
                )
                .await
        }
    });
    let request = tokio::time::timeout(std::time::Duration::from_secs(1), async {
        loop {
            let event = rx
                .recv()
                .await
                .expect("permission event channel should stay open");
            if let codex_protocol::protocol::EventMsg::RequestPermissions(request) = event.msg {
                break request;
            }
        }
    })
    .await
    .expect("request_permissions event timed out");
    assert_eq!(request.call_id, call_id);
    assert_eq!(
        request.environment_id.as_deref(),
        Some(codex_exec_server::REMOTE_ENVIRONMENT_ID)
    );
    assert_eq!(request.cwd, Some(native_cwd));
    session
        .notify_request_permissions_response(&request.call_id, expected_response.clone())
        .await;
    let response = tokio::time::timeout(std::time::Duration::from_secs(1), request_handle)
        .await
        .expect("request_permissions future timed out")
        .expect("request_permissions join error");
    assert_eq!(response, Some(expected_response));

    let effective_with_late_session_grant = crate::tools::handlers::apply_granted_turn_permissions(
        &session,
        &turn,
        &environment,
        &cwd,
        crate::sandboxing::SandboxPermissions::WithAdditionalPermissions,
        None,
    )
    .await;
    let effective_permissions = effective_with_late_session_grant
        .additional_permissions
        .expect("captured turn and current session grants should be applied");
    assert_eq!(
        &effective_permissions.file_system,
        &granted_permissions.file_system
    );
    assert_eq!(
        &effective_permissions.network,
        &Some(codex_protocol::models::NetworkPermissions {
            enabled: Some(true),
        })
    );
    assert_ne!(effective_permissions, other_turn_permissions);
}

#[tokio::test]
async fn exec_command_pre_tool_use_payload_uses_raw_command() {
    let payload = ToolPayload::Function {
        arguments: serde_json::json!({ "cmd": "printf exec command" }).to_string(),
    };
    let (session, turn) = make_session_and_context().await;
    let turn = Arc::new(turn);
    let handler = ExecCommandHandler::default();

    assert_eq!(
        handler.pre_tool_use_payload(&ToolInvocation {
            session: session.into(),
            step_context: StepContext::for_test(Arc::clone(&turn)),
            turn,
            cancellation_token: tokio_util::sync::CancellationToken::new(),
            tracker: Arc::new(Mutex::new(TurnDiffTracker::new())),
            call_id: "call-43".to_string(),
            tool_name: codex_tools::ToolName::plain("exec_command"),
            source: crate::tools::context::ToolCallSource::Direct,
            payload,
        }),
        Some(crate::tools::registry::PreToolUsePayload {
            tool_name: HookToolName::bash(),
            tool_input: serde_json::json!({ "command": "printf exec command" }),
        })
    );
}

#[tokio::test]
async fn exec_command_pre_tool_use_payload_skips_write_stdin() {
    let payload = ToolPayload::Function {
        arguments: serde_json::json!({ "chars": "echo hi" }).to_string(),
    };
    let (session, turn) = make_session_and_context().await;
    let turn = Arc::new(turn);
    let handler = WriteStdinHandler;

    assert_eq!(
        handler.pre_tool_use_payload(&ToolInvocation {
            session: session.into(),
            step_context: StepContext::for_test(Arc::clone(&turn)),
            turn,
            cancellation_token: tokio_util::sync::CancellationToken::new(),
            tracker: Arc::new(Mutex::new(TurnDiffTracker::new())),
            call_id: "call-44".to_string(),
            tool_name: codex_tools::ToolName::plain("write_stdin"),
            source: crate::tools::context::ToolCallSource::Direct,
            payload,
        }),
        None
    );
}

#[tokio::test]
async fn exec_command_post_tool_use_payload_uses_output_for_noninteractive_one_shot_commands() {
    let payload = ToolPayload::Function {
        arguments: serde_json::json!({ "cmd": "echo three", "tty": false }).to_string(),
    };
    let output = ExecCommandToolOutput {
        event_call_id: "call-43".to_string(),
        chunk_id: "chunk-1".to_string(),
        wall_time: std::time::Duration::from_millis(498),
        raw_output: b"three".to_vec(),
        truncation_policy: TEST_TRUNCATION_POLICY,
        max_output_tokens: None,
        process_id: None,
        exit_code: Some(0),
        original_token_count: None,
        output_omitted_bytes: None,
        hook_command: Some("echo three".to_string()),
    };
    let invocation = invocation_for_payload("exec_command", "call-43", payload).await;
    let handler = ExecCommandHandler::default();
    assert_eq!(
        handler.post_tool_use_payload(&invocation, &output),
        Some(crate::tools::registry::PostToolUsePayload {
            tool_name: HookToolName::bash(),
            tool_use_id: "call-43".to_string(),
            tool_input: serde_json::json!({ "command": "echo three" }),
            tool_response: serde_json::json!("three"),
        })
    );
}

#[tokio::test]
async fn exec_command_post_tool_use_payload_uses_output_for_interactive_completion() {
    let payload = ToolPayload::Function {
        arguments: serde_json::json!({ "cmd": "echo three", "tty": true }).to_string(),
    };
    let output = ExecCommandToolOutput {
        event_call_id: "call-44".to_string(),
        chunk_id: "chunk-1".to_string(),
        wall_time: std::time::Duration::from_millis(498),
        raw_output: b"three".to_vec(),
        truncation_policy: TEST_TRUNCATION_POLICY,
        max_output_tokens: None,
        process_id: None,
        exit_code: Some(0),
        original_token_count: None,
        output_omitted_bytes: None,
        hook_command: Some("echo three".to_string()),
    };
    let invocation = invocation_for_payload("exec_command", "call-44", payload).await;
    let handler = ExecCommandHandler::default();

    assert_eq!(
        handler.post_tool_use_payload(&invocation, &output),
        Some(crate::tools::registry::PostToolUsePayload {
            tool_name: HookToolName::bash(),
            tool_use_id: "call-44".to_string(),
            tool_input: serde_json::json!({ "command": "echo three" }),
            tool_response: serde_json::json!("three"),
        })
    );
}

#[tokio::test]
async fn exec_command_post_tool_use_payload_skips_running_sessions() {
    let payload = ToolPayload::Function {
        arguments: serde_json::json!({ "cmd": "echo three", "tty": false }).to_string(),
    };
    let output = ExecCommandToolOutput {
        event_call_id: "event-45".to_string(),
        chunk_id: "chunk-1".to_string(),
        wall_time: std::time::Duration::from_millis(498),
        raw_output: b"three".to_vec(),
        truncation_policy: TEST_TRUNCATION_POLICY,
        max_output_tokens: None,
        process_id: Some(45),
        exit_code: None,
        original_token_count: None,
        output_omitted_bytes: None,
        hook_command: Some("echo three".to_string()),
    };
    let invocation = invocation_for_payload("exec_command", "call-45", payload).await;
    let handler = ExecCommandHandler::default();
    assert_eq!(handler.post_tool_use_payload(&invocation, &output), None);
}

#[tokio::test]
async fn write_stdin_post_tool_use_payload_uses_original_exec_call_id_and_command_on_completion() {
    let payload = ToolPayload::Function {
        arguments: serde_json::json!({
            "session_id": 45,
            "chars": "",
        })
        .to_string(),
    };
    let output = ExecCommandToolOutput {
        event_call_id: "exec-call-45".to_string(),
        chunk_id: "chunk-2".to_string(),
        wall_time: std::time::Duration::from_millis(498),
        raw_output: b"finished\n".to_vec(),
        truncation_policy: TEST_TRUNCATION_POLICY,
        max_output_tokens: None,
        process_id: None,
        exit_code: Some(0),
        original_token_count: None,
        output_omitted_bytes: None,
        hook_command: Some("sleep 1; echo finished".to_string()),
    };
    let invocation = invocation_for_payload("write_stdin", "write-stdin-call", payload).await;
    let handler = WriteStdinHandler;

    assert_eq!(
        handler.post_tool_use_payload(&invocation, &output),
        Some(crate::tools::registry::PostToolUsePayload {
            tool_name: HookToolName::bash(),
            tool_use_id: "exec-call-45".to_string(),
            tool_input: serde_json::json!({ "command": "sleep 1; echo finished" }),
            tool_response: serde_json::json!("finished\n"),
        })
    );
}

#[tokio::test]
async fn write_stdin_post_tool_use_payload_keeps_parallel_session_metadata_separate() {
    let payload = ToolPayload::Function {
        arguments: serde_json::json!({ "session_id": 45, "chars": "" }).to_string(),
    };
    let output_a = ExecCommandToolOutput {
        event_call_id: "exec-call-a".to_string(),
        chunk_id: "chunk-a".to_string(),
        wall_time: std::time::Duration::from_millis(498),
        raw_output: b"alpha\n".to_vec(),
        truncation_policy: TEST_TRUNCATION_POLICY,
        max_output_tokens: None,
        process_id: None,
        exit_code: Some(0),
        original_token_count: None,
        output_omitted_bytes: None,
        hook_command: Some("sleep 2; echo alpha".to_string()),
    };
    let output_b = ExecCommandToolOutput {
        event_call_id: "exec-call-b".to_string(),
        chunk_id: "chunk-b".to_string(),
        wall_time: std::time::Duration::from_millis(498),
        raw_output: b"beta\n".to_vec(),
        truncation_policy: TEST_TRUNCATION_POLICY,
        max_output_tokens: None,
        process_id: None,
        exit_code: Some(0),
        original_token_count: None,
        output_omitted_bytes: None,
        hook_command: Some("sleep 1; echo beta".to_string()),
    };
    let invocation_b = invocation_for_payload("write_stdin", "write-call-b", payload.clone()).await;
    let invocation_a = invocation_for_payload("write_stdin", "write-call-a", payload).await;
    let handler = WriteStdinHandler;

    let payloads = [
        handler.post_tool_use_payload(&invocation_b, &output_b),
        handler.post_tool_use_payload(&invocation_a, &output_a),
    ];

    assert_eq!(
        payloads,
        [
            Some(crate::tools::registry::PostToolUsePayload {
                tool_name: HookToolName::bash(),
                tool_use_id: "exec-call-b".to_string(),
                tool_input: serde_json::json!({ "command": "sleep 1; echo beta" }),
                tool_response: serde_json::json!("beta\n"),
            }),
            Some(crate::tools::registry::PostToolUsePayload {
                tool_name: HookToolName::bash(),
                tool_use_id: "exec-call-a".to_string(),
                tool_input: serde_json::json!({ "command": "sleep 2; echo alpha" }),
                tool_response: serde_json::json!("alpha\n"),
            }),
        ]
    );
}
