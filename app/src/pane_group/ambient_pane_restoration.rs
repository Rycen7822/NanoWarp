use warpui::{SingletonEntity, ViewContext, ViewHandle};

use crate::ai::agent::api::ServerConversationToken;
use crate::ai::agent_conversations_model::{AgentConversationsModel, AgentConversationsModelEvent};
use crate::ai::ambient_agents::{AmbientAgentTaskId, ExecutionLocation};
use crate::ai::blocklist::BlocklistAIHistoryModel;
use crate::pane_group::{PaneGroup, PaneId};
use crate::terminal::TerminalView;
use crate::terminal::model::terminal_model::ConversationTranscriptViewerStatus;
use crate::view_components::ToastFlavor;

impl PaneGroup {
    /// Restore the existing Native task identities into read-only transcript viewers.
    pub(in crate::pane_group) fn register_pending_ambient_restorations(
        &mut self,
        pending: Vec<(AmbientAgentTaskId, PaneId)>,
        ctx: &mut ViewContext<Self>,
    ) {
        if pending.is_empty() {
            return;
        }
        ctx.subscribe_to_model(
            &AgentConversationsModel::handle(ctx),
            |me, _, event, ctx| {
                if matches!(
                    event,
                    AgentConversationsModelEvent::TasksUpdated
                        | AgentConversationsModelEvent::ConversationsLoaded
                ) {
                    me.process_pending_ambient_restorations(ctx);
                }
            },
        );
        for (task_id, _) in &pending {
            AgentConversationsModel::handle(ctx).update(ctx, |model, ctx| {
                model.get_or_async_fetch_task_data(task_id, ctx);
            });
        }
        self.pending_ambient_agent_conversation_restorations = pending.into_iter().collect();
        self.process_pending_ambient_restorations(ctx);
    }

    pub(in crate::pane_group) fn process_pending_ambient_restorations(
        &mut self,
        ctx: &mut ViewContext<Self>,
    ) {
        let pending: Vec<_> = self
            .pending_ambient_agent_conversation_restorations
            .iter()
            .map(|(task_id, pane_id)| (*task_id, *pane_id))
            .collect();
        for (task_id, pane_id) in pending {
            let task = AgentConversationsModel::as_ref(ctx).get_task_data(&task_id);
            if task.is_none()
                && AgentConversationsModel::as_ref(ctx)
                    .task_fetch_error(&task_id)
                    .is_none()
            {
                continue;
            }
            let Some(target_view) = self.terminal_view_from_pane_id(pane_id, ctx) else {
                self.pending_ambient_agent_conversation_restorations
                    .remove(&task_id);
                continue;
            };
            self.pending_ambient_agent_conversation_restorations
                .remove(&task_id);

            target_view.update(ctx, |view, _| {
                view.model
                    .lock()
                    .set_conversation_transcript_viewer_status(Some(
                        ConversationTranscriptViewerStatus::ViewingAmbientConversation(task_id),
                    ));
            });

            // The persisted identity is shared with old remote tasks. Only a confirmed
            // Native task may load a transcript; no execution session is restored here.
            let token = task
                .filter(|task| task.execution_location == Some(ExecutionLocation::Local))
                .and_then(|task| {
                    task.conversation_id()
                        .map(|id| ServerConversationToken::new(id.to_string()))
                });
            if let Some(token) = token {
                Self::fetch_and_load_transcript(target_view, token, task_id, ctx);
            } else {
                target_view.update(ctx, |view, ctx| {
                    view.show_persistent_toast(
                        "Unable to restore this task's history.".into(),
                        ToastFlavor::Error,
                        ctx,
                    );
                });
            }
        }
    }

    /// Reuse the existing transcript loader; failed loads retain a read-only task identity.
    fn fetch_and_load_transcript(
        target_view: ViewHandle<TerminalView>,
        server_conversation_token: ServerConversationToken,
        task_id: AmbientAgentTaskId,
        ctx: &mut ViewContext<Self>,
    ) {
        let future = BlocklistAIHistoryModel::handle(ctx).update(ctx, |history_model, ctx| {
            history_model.load_conversation_by_server_token(&server_conversation_token, ctx)
        });
        ctx.spawn(future, move |group, conversation, ctx| {
            if let Some(conversation) = conversation {
                group.load_data_into_transcript_viewer(
                    target_view,
                    conversation,
                    Some(task_id),
                    ctx,
                );
            } else {
                target_view.update(ctx, |view, ctx| {
                    view.show_persistent_toast(
                        "Unable to restore this task's history.".into(),
                        ToastFlavor::Error,
                        ctx,
                    );
                });
            }
        });
    }
}
