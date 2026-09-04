//! Exposes shared installed-plugin reconciliation with hook lifecycle updates.

use super::*;
use crate::effective_plugin_change::trust_materialized_plugin_hooks;
use crate::request_serialization::RequestSerializationAccess;
use crate::request_serialization::RequestSerializationQueueKey;
use crate::request_serialization::RequestSerializationQueues;
use codex_app_server_protocol::PluginReconcileChangedPlugin;
use codex_app_server_protocol::PluginReconcileParams;
use codex_app_server_protocol::PluginReconcileResponse;
use codex_core_plugins::LoadedPlugin;
use codex_core_plugins::PluginLoadOutcome;
use codex_core_plugins::remote::RemotePluginShareDiscoverability;
use std::collections::BTreeMap;
use std::collections::BTreeSet;

#[derive(Clone, Copy, Default, PartialEq, Eq)]
struct PluginCapabilities {
    has_mcps: bool,
    has_apps: bool,
    has_hooks: bool,
    has_skills: bool,
}

impl PluginCapabilities {
    fn from_loaded_plugin(plugin: &LoadedPlugin) -> Self {
        Self {
            has_mcps: !plugin.mcp_servers.is_empty(),
            has_apps: !plugin.apps.is_empty(),
            has_hooks: !plugin.hook_sources.is_empty(),
            has_skills: plugin.has_enabled_skills || !plugin.skill_roots.is_empty(),
        }
    }

    fn union(self, other: Self) -> Self {
        Self {
            has_mcps: self.has_mcps || other.has_mcps,
            has_apps: self.has_apps || other.has_apps,
            has_hooks: self.has_hooks || other.has_hooks,
            has_skills: self.has_skills || other.has_skills,
        }
    }

    fn into_response(self, id: String) -> PluginReconcileChangedPlugin {
        PluginReconcileChangedPlugin {
            id,
            has_mcps: self.has_mcps,
            has_apps: self.has_apps,
            has_hooks: self.has_hooks,
            has_skills: self.has_skills,
        }
    }
}

#[derive(Clone, Copy, Default)]
struct LoadedPluginState {
    enabled: bool,
    capabilities: PluginCapabilities,
    is_remote: bool,
}

fn loaded_plugin_states(outcome: &PluginLoadOutcome) -> BTreeMap<String, LoadedPluginState> {
    outcome
        .plugins()
        .iter()
        .map(|plugin| {
            (
                plugin.config_name.clone(),
                LoadedPluginState {
                    enabled: plugin.enabled,
                    capabilities: PluginCapabilities::from_loaded_plugin(plugin),
                    is_remote: plugin.remote_plugin_id.is_some(),
                },
            )
        })
        .collect()
}

fn changed_plugins(
    before: &PluginLoadOutcome,
    after: &PluginLoadOutcome,
    materialized_plugin_ids: impl IntoIterator<Item = String>,
    removed_plugin_ids: impl IntoIterator<Item = String>,
) -> Vec<PluginReconcileChangedPlugin> {
    let before = loaded_plugin_states(before);
    let after = loaded_plugin_states(after);
    let materialized_plugin_ids = materialized_plugin_ids.into_iter().collect::<BTreeSet<_>>();
    let removed_plugin_ids = removed_plugin_ids.into_iter().collect::<BTreeSet<_>>();
    let mut candidate_ids = before.keys().cloned().collect::<BTreeSet<_>>();
    candidate_ids.extend(after.keys().cloned());
    candidate_ids.extend(materialized_plugin_ids.iter().cloned());
    candidate_ids.extend(removed_plugin_ids.iter().cloned());

    candidate_ids
        .into_iter()
        .filter_map(|id| {
            let old = before.get(&id).copied();
            let new = after.get(&id).copied();
            let forced = materialized_plugin_ids.contains(&id) || removed_plugin_ids.contains(&id);
            let capabilities = old
                .map(|state| state.capabilities)
                .unwrap_or_default()
                .union(new.map(|state| state.capabilities).unwrap_or_default());
            let changed = forced
                || match (old, new) {
                    (Some(old), Some(new)) => {
                        old.enabled != new.enabled || old.capabilities != new.capabilities
                    }
                    // A remote plugin disappearing from the effective load is a removal even if
                    // stale cache cleanup could not record it separately.
                    (Some(old), None) => old.is_remote,
                    (None, Some(_)) => false,
                    (None, None) => false,
                };
            changed.then(|| capabilities.into_response(id))
        })
        .collect()
}

impl PluginRequestProcessor {
    #[tracing::instrument(level = "debug", skip_all, fields(reason = ?params.reason))]
    pub(crate) async fn plugin_reconcile(
        &self,
        params: PluginReconcileParams,
        config_processor: ConfigRequestProcessor,
        request_serialization_queues: &RequestSerializationQueues,
    ) -> Result<Option<ClientResponsePayload>, JSONRPCErrorError> {
        let config = self.load_latest_config(/*fallback_cwd*/ None).await?;
        let plugins_input = config.plugins_config_input();
        let auth = self.auth_manager.auth().await;
        if !plugins_input.plugins_enabled
            || !auth.as_ref().is_some_and(CodexAuth::uses_codex_backend)
        {
            return Ok(Some(PluginReconcileResponse::default().into()));
        }

        let plugins_manager = self.thread_manager.plugins_manager();
        let plugins_before = plugins_manager.plugins_for_config(&plugins_input).await;

        // Match background bundle sync: remote_plugin controls catalog visibility, not sync.
        // The shared reconciler owns synchronization, auth checks, and cache publication.
        let outcome = self
            .thread_manager
            .plugins_manager()
            .reconcile_remote_installed_plugins(&plugins_input, auth.as_ref())
            .await
            .map_err(|err| {
                internal_error(format!(
                    "failed to reconcile remote installed plugins: {err}"
                ))
            })?;

        let plugins_after = plugins_manager.plugins_for_config(&plugins_input).await;
        let changed_plugins = changed_plugins(
            &plugins_before,
            &plugins_after,
            outcome
                .materialized_remote_plugins
                .iter()
                .map(|plugin| plugin.plugin_id.as_key()),
            outcome.removed_cache_plugin_ids.clone(),
        );
        let hooks_changed = changed_plugins.iter().any(|plugin| plugin.has_hooks);
        // Preserve materialization-owned trust: unchanged background passes cannot recover it.
        // Serialize hook updates with config writes, after releasing the bundle writer gate.
        // Changes without hooks or eligible materializations still skip this queue.
        if hooks_changed
            || outcome.materialized_remote_plugins.iter().any(|plugin| {
                plugin.scope == RemotePluginScope::Workspace
                    && plugin.discoverability == Some(RemotePluginShareDiscoverability::Listed)
            })
        {
            let materializations = outcome.materialized_remote_plugins.clone();
            let processor = self.clone();
            let (complete, completion) = tokio::sync::oneshot::channel();
            request_serialization_queues
                .enqueue_background(
                    RequestSerializationQueueKey::Global("config"),
                    RequestSerializationAccess::Exclusive,
                    async move {
                        let result = trust_materialized_plugin_hooks(
                            materializations,
                            &processor.auth_manager,
                            &processor.thread_manager,
                            &processor.config_manager,
                            &config_processor,
                        )
                        .await;
                        // Removals and disablements have no trust write to rebuild loaded hooks.
                        // Rebuild after the trust attempt, even if it failed, to drop stale hooks.
                        if hooks_changed {
                            processor.thread_manager.refresh_hook_runtimes().await;
                        }
                        let _ = complete.send(result);
                    },
                )
                .await;
            completion
                .await
                .map_err(|err| {
                    internal_error(format!("plugin hook trust update was cancelled: {err}"))
                })?
                .map_err(|err| {
                    internal_error(format!("failed to trust materialized plugin hooks: {err}"))
                })?;
        }

        Ok(Some(
            PluginReconcileResponse {
                changed_plugins,
                failed_remote_plugin_ids: outcome.failed_remote_plugin_ids,
                failed_materialization_remote_plugin_ids: outcome
                    .failed_materialization_remote_plugin_ids,
            }
            .into(),
        ))
    }
}
