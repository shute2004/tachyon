use super::*;
use crate::outgoing_message::ConnectionId;
use crate::outgoing_message::OutgoingMessageSender;
use crate::skills_watcher::SkillsWatcher;
use crate::thread_state::ConnectionCapabilities;
use crate::thread_state::ThreadStateManager;
use crate::thread_status::ThreadWatchManager;
use codex_analytics::AnalyticsEventsClient;
use codex_core::StartThreadOptions;
use codex_exec_server::EnvironmentManager;
use codex_login::CodexAuth;
use std::collections::HashSet;
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::Mutex;
use tokio::sync::Semaphore;
use tokio::sync::mpsc;

#[tokio::test]
async fn connection_closed_during_thread_start_still_starts_listener() -> anyhow::Result<()> {
    let home = tempfile::TempDir::new()?;
    let config = codex_core::config::ConfigBuilder::default()
        .codex_home(home.path().to_path_buf())
        .build()
        .await?;
    let thread_manager = Arc::new(
        codex_core::test_support::thread_manager_with_models_provider_and_home(
            CodexAuth::create_dummy_chatgpt_auth_for_testing(),
            config.model_provider.clone(),
            config.codex_home.to_path_buf(),
            Arc::new(EnvironmentManager::default_for_tests()),
        ),
    );
    let new_thread = thread_manager
        .start_thread(StartThreadOptions::new(config.clone()))
        .await?;
    let thread_id = new_thread.thread_id;
    let conversation = Arc::clone(&new_thread.thread);

    let (outgoing_tx, _outgoing_rx) = mpsc::channel(8);
    let outgoing = Arc::new(OutgoingMessageSender::new(
        outgoing_tx,
        AnalyticsEventsClient::disabled(),
    ));
    let thread_state_manager = ThreadStateManager::new();
    let thread_watch_manager = ThreadWatchManager::new();
    thread_watch_manager
        .upsert_thread_silently(&thread_id.to_string())
        .await;
    let skills_watcher = SkillsWatcher::new(
        thread_manager.skills_service(),
        &config.codex_home,
        Arc::clone(&outgoing),
    );
    let listener_task_context = ListenerTaskContext {
        thread_manager: Arc::clone(&thread_manager),
        thread_state_manager: thread_state_manager.clone(),
        outgoing,
        pending_thread_unloads: Arc::new(Mutex::new(HashSet::new())),
        thread_watch_manager,
        thread_list_state_permit: Arc::new(Semaphore::new(1)),
        fallback_model_provider: config.model_provider_id.clone(),
        codex_home: config.codex_home.to_path_buf(),
        skills_watcher: Arc::clone(&skills_watcher),
        turn_cost_worker: None,
    };

    assert!(matches!(
        ensure_conversation_listener(
            listener_task_context.clone(),
            thread_id,
            ConnectionId(1),
            /*raw_events_enabled*/ false,
        )
        .await
        .expect("closed connection still installs a cleanup listener"),
        EnsureConversationListenerResult::ConnectionClosed
    ));
    let thread_state = thread_state_manager.thread_state(thread_id).await;
    {
        let thread_state = thread_state.lock().await;
        assert!(thread_state.listener_matches(&conversation));
        assert!(thread_state.listener_command_tx().is_some());
    }
    assert!(!thread_state_manager.has_subscribers(thread_id).await);

    let live_connection = ConnectionId(2);
    thread_state_manager
        .connection_initialized(live_connection, ConnectionCapabilities::default())
        .await;
    assert!(matches!(
        ensure_conversation_listener(
            listener_task_context,
            thread_id,
            live_connection,
            /*raw_events_enabled*/ false,
        )
        .await
        .expect("live connection attaches to the existing listener"),
        EnsureConversationListenerResult::Attached
    ));
    assert!(thread_state_manager.has_subscribers(thread_id).await);
    assert!(
        thread_state_manager
            .current_listener_command_tx(thread_id)
            .is_some_and(|sender| !sender.is_closed())
    );

    thread_state_manager.clear_all_listeners().await;
    skills_watcher.shutdown();
    let shutdown = thread_manager
        .shutdown_all_threads_bounded(Duration::from_secs(10))
        .await;
    assert!(shutdown.timed_out.is_empty());
    assert!(shutdown.submit_failed.is_empty());
    Ok(())
}
