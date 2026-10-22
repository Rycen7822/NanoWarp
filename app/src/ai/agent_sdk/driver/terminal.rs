use super::AgentDriverError;
use crate::ai::agent::redaction::redact_secrets;
use crate::terminal::TerminalView;
use crate::terminal::model::block::{BlockId, BlockState};
use crate::terminal::model::session::ExecuteCommandOptions;
use crate::terminal::shell::ShellType;
use futures::channel::oneshot;
use instant::Instant;
use std::future::Future;
use std::pin::Pin;
use std::task::{Context, Poll};
use std::time::Duration;
use uuid::Uuid;
use warp_completer::completer::CommandOutput;
use warp_core::command::ExitCode;
use warp_util::path::ShellFamily;
use warpui::r#async::FutureExt;
use warpui::{AppContext, Entity, ModelContext, ModelHandle, ViewHandle};

#[derive(Debug, Clone)]
pub(crate) enum BootstrapError {
    /// The PTY or shell process failed before the bootstrap script completed.
    /// When `reason` is `Some`, the message is
    /// "Shell spawn failed: {reason}. Check the Warp logs for details."
    /// When `reason` is `None`, it is
    /// "Shell spawn failed. Check the Warp logs for details."
    PtySpawnFailed { reason: Option<String> },
    /// The bootstrap script did not complete within the expected time.
    TimedOut,
    /// An unexpected internal error in the bootstrap channel
    /// (e.g. the sender was dropped without sending).
    InternalError,
}

impl std::fmt::Display for BootstrapError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            BootstrapError::PtySpawnFailed { reason: Some(r) } => {
                write!(
                    f,
                    "Shell spawn failed: {r}. Check the Warp logs for details."
                )
            }
            BootstrapError::PtySpawnFailed { reason: None } => {
                write!(f, "Shell spawn failed. Check the Warp logs for details.")
            }
            BootstrapError::TimedOut => write!(
                f,
                "Terminal session did not start within the expected time. \
                 Check the Warp logs for details."
            ),
            BootstrapError::InternalError => {
                write!(f, "An unexpected internal error occurred during bootstrap.")
            }
        }
    }
}

impl std::error::Error for BootstrapError {}

const TERMINAL_SESSION_BOOTSTRAP_TIMEOUT: Duration = Duration::from_secs(60);

struct WaitingCommand {
    exit_tx: oneshot::Sender<Result<ExitCode, AgentDriverError>>,
    started_block_id: Option<BlockId>,
}

pub(crate) struct TerminalDriver {
    terminal_view: ViewHandle<TerminalView>,
    bootstrap_tx: Option<oneshot::Sender<Result<(), BootstrapError>>>,
    bootstrap_rx: Option<oneshot::Receiver<Result<(), BootstrapError>>>,
    waiting_command: Option<WaitingCommand>,
    pending_command_start: Option<(String, oneshot::Sender<Result<BlockId, AgentDriverError>>)>,
    shell_exited: bool,
    last_command: Option<String>,
}

impl Entity for TerminalDriver {
    type Event = ();
}

impl TerminalDriver {
    pub(crate) fn create_from_existing_view(
        terminal_view: ViewHandle<TerminalView>,
        ctx: &mut AppContext,
    ) -> ModelHandle<Self> {
        ctx.add_model(|ctx| Self::new(terminal_view, ctx))
    }

    fn new(terminal_view: ViewHandle<TerminalView>, ctx: &mut ModelContext<Self>) -> Self {
        ctx.subscribe_to_view(&terminal_view, move |me, _, event, ctx| {
            me.handle_terminal_view_event(event, ctx);
        });

        let (bootstrap_tx_inner, bootstrap_rx) = oneshot::channel::<Result<(), BootstrapError>>();

        // If bootstrap already completed before we subscribed, resolve the
        // channel immediately so `wait_for_session_bootstrapped` doesn't block.
        let already_bootstrapped = terminal_view.read(ctx, |terminal, _| {
            terminal
                .model
                .lock()
                .block_list()
                .is_bootstrapping_precmd_done()
        });
        let bootstrap_tx = if already_bootstrapped {
            let _ = bootstrap_tx_inner.send(Ok(()));
            None
        } else {
            Some(bootstrap_tx_inner)
        };

        Self {
            terminal_view,
            bootstrap_tx,
            bootstrap_rx: Some(bootstrap_rx),
            waiting_command: None,
            pending_command_start: None,
            shell_exited: false,
            last_command: None,
        }
    }

    pub fn block_output_plaintext(&self, block_id: &BlockId, ctx: &AppContext) -> Option<String> {
        let terminal = self.terminal_view.as_ref(ctx);
        let model = terminal.model.lock();
        let block = model.block_list().block_with_id(block_id)?;
        let grid = if block.state() == BlockState::BeforeExecution {
            block.prompt_and_command_grid()
        } else {
            block.output_grid()
        };
        Some(grid.contents_to_string(
            false, // include_escape_sequences
            None,  // max_rows: full visible output
        ))
    }

    fn shell_exited_error(&self) -> AgentDriverError {
        AgentDriverError::SetupCommandExitedShell {
            command: self
                .last_command
                .clone()
                .unwrap_or_else(|| "<unknown>".to_string()),
        }
    }

    pub fn execute_command(
        &mut self,
        command: &str,
        ctx: &mut ModelContext<Self>,
    ) -> Result<
        impl Future<Output = Result<CommandHandle, AgentDriverError>> + use<>,
        AgentDriverError,
    > {
        // The shell process has exited, so no further commands can run in
        // this session. Fail fast with the shell-exit error so callers
        // (e.g. environment setup) report the failure instead of waiting
        // forever on a command that can never start.
        if self.shell_exited {
            return Err(self.shell_exited_error());
        }

        let (exit_tx, exit_rx) = oneshot::channel::<Result<ExitCode, AgentDriverError>>();
        let (start_tx, start_rx) = oneshot::channel::<Result<BlockId, AgentDriverError>>();

        // We should not be able to execute a command while we are still waiting on another one.
        // This is enforced by the caller by waiting on rx before continuing.
        if self.waiting_command.is_some() || self.pending_command_start.is_some() {
            return Err(AgentDriverError::InvalidRuntimeState);
        }

        let command_string = command.to_string();
        let command_id = Uuid::new_v4();
        let submitted_at = Instant::now();
        log::info!("Terminal command lifecycle: event=submitted command_id={command_id}");
        // Store a secret-redacted copy for shell-exit attribution: the text
        // flows into error reports (server task status, Sentry) if the shell
        // dies, so never retain the raw command here.
        let mut redacted_command = command_string.clone();
        redact_secrets(&mut redacted_command);
        self.last_command = Some(redacted_command);
        self.terminal_view.update(ctx, |terminal, ctx| {
            self.waiting_command = Some(WaitingCommand {
                exit_tx,
                started_block_id: None,
            });
            self.pending_command_start = Some((command_string, start_tx));
            terminal.execute_command_or_set_pending(command, ctx);
        });

        Ok(async move {
            let block_id = start_rx
                .await
                .map_err(|_| AgentDriverError::InvalidRuntimeState)??;
            log::info!(
                "Terminal command lifecycle: event=started command_id={command_id} block_id={block_id:?} elapsed_ms={}",
                submitted_at.elapsed().as_millis()
            );
            Ok(CommandHandle {
                exit_status_rx: exit_rx,
                block_id,
                command_id,
                submitted_at,
            })
        })
    }

    pub fn execute_silent_command(
        &self,
        command: String,
        ctx: &ModelContext<Self>,
    ) -> impl Future<Output = Result<CommandOutput, AgentDriverError>> + use<> {
        let session = self.terminal_view.read(ctx, |terminal, app| {
            terminal
                .active_block_session_id()
                .and_then(|id| terminal.sessions_model().as_ref(app).get(id))
        });
        async move {
            let session = session.ok_or(AgentDriverError::InvalidRuntimeState)?;
            session
                .execute_command(&command, None, None, ExecuteCommandOptions::default())
                .await
                .map_err(|e| {
                    log::warn!("silent command failed: {e:#}");
                    AgentDriverError::InvalidRuntimeState
                })
        }
    }

    pub fn active_session_shell_type(&self, ctx: &AppContext) -> Option<ShellType> {
        self.terminal_view
            .read(ctx, |terminal, app| terminal.active_session_shell_type(app))
    }

    fn build_cd_command(&self, target: &str, ctx: &AppContext) -> String {
        let shell_family = self.terminal_view.read(ctx, |terminal, app| {
            terminal
                .active_session_shell_type(app)
                .map(ShellFamily::from)
                .unwrap_or(ShellFamily::Posix)
        });
        let escaped_target = shell_family.shell_escape(target);
        format!("cd {escaped_target}")
    }

    pub fn cd(
        &mut self,
        target: &str,
        ctx: &mut ModelContext<Self>,
    ) -> Result<
        impl Future<Output = Result<CommandHandle, AgentDriverError>> + use<>,
        AgentDriverError,
    > {
        let cd_command = self.build_cd_command(target, ctx);
        self.execute_command(&cd_command, ctx)
    }

    pub fn cd_silent(
        &self,
        target: &str,
        ctx: &ModelContext<Self>,
    ) -> impl Future<Output = Result<CommandOutput, AgentDriverError>> + use<> {
        let cd_command = self.build_cd_command(target, ctx);
        self.execute_silent_command(cd_command, ctx)
    }

    pub fn wait_for_session_bootstrapped(
        &mut self,
    ) -> impl Future<Output = Result<(), BootstrapError>> + use<> {
        let bootstrap_rx = self.bootstrap_rx.take();

        async move {
            if let Some(rx) = bootstrap_rx {
                // Map channel cancellation (sender dropped without sending)
                // to InternalError — this shouldn't happen in practice.
                let inner = async move { rx.await.unwrap_or(Err(BootstrapError::InternalError)) };
                match inner.with_timeout(TERMINAL_SESSION_BOOTSTRAP_TIMEOUT).await {
                    Ok(result) => result,
                    Err(_timeout) => Err(BootstrapError::TimedOut),
                }
            } else {
                // bootstrap_rx already consumed — shouldn't happen in normal flow.
                Err(BootstrapError::InternalError)
            }
        }
    }

    fn handle_terminal_view_event(
        &mut self,
        event: &crate::terminal::view::Event,
        ctx: &mut ModelContext<Self>,
    ) {
        match event {
            crate::terminal::view::Event::SessionBootstrapped => {
                if let Some(tx) = self.bootstrap_tx.take() {
                    let _ = tx.send(Ok(()));
                }
            }
            crate::terminal::view::Event::PtySpawnFailed { reason } => {
                // Signal the bootstrap waiter immediately so it doesn't wait
                // for the full 60 s timeout when a spawn failure has already
                // been confirmed.
                if let Some(tx) = self.bootstrap_tx.take() {
                    let _ = tx.send(Err(BootstrapError::PtySpawnFailed {
                        reason: Some(reason.clone()),
                    }));
                }
            }
            crate::terminal::view::Event::Exited => {
                log::warn!(
                    "Terminal command lifecycle: event=shell_exited pending_start={} waiting_exit={}",
                    self.pending_command_start.is_some(),
                    self.waiting_command.is_some()
                );
                // The shell process exited before bootstrap completed —
                // cancel the wait immediately rather than sitting out the
                // full 60 s timeout. No specific reason is known at this
                // point; the logs will have details.
                if let Some(tx) = self.bootstrap_tx.take() {
                    let _ = tx.send(Err(BootstrapError::PtySpawnFailed { reason: None }));
                }

                // The shell is gone: no further command can start or finish.
                // Fail any in-flight command (e.g. an environment setup
                // command) with the shell-exit error so the run reports the
                // failure instead of hanging until the sandbox is killed.
                self.shell_exited = true;
                if let Some((_, sender)) = self.pending_command_start.take() {
                    let _ = sender.send(Err(self.shell_exited_error()));
                }
                if let Some(command) = self.waiting_command.take() {
                    let _ = command.exit_tx.send(Err(self.shell_exited_error()));
                }
            }
            crate::terminal::view::Event::ExecuteCommand(event) => {
                if let Some((_expected_command, sender)) = self
                    .pending_command_start
                    .take_if(|(cmd, _)| *cmd == event.command)
                {
                    let block_id = self.terminal_view.read(ctx, |terminal, _| {
                        terminal.model.lock().block_list().active_block_id().clone()
                    });
                    if let Some(command) = self.waiting_command.as_mut() {
                        command.started_block_id = Some(block_id.clone());
                    }
                    let _ = sender.send(Ok(block_id));
                }
            }
            crate::terminal::view::Event::BlockCompleted { block, .. } => {
                // Missing preexec hooks leave did_execute false even for commands that ran.
                if let Some(command) = self
                    .waiting_command
                    .take_if(|command| command.started_block_id.as_ref() == Some(&block.id))
                {
                    let _ = command.exit_tx.send(Ok(block.exit_code));
                } else if let Some(command) = self.waiting_command.as_ref() {
                    log::info!(
                        "Ignoring terminal block completion while waiting for command: completed_block_id={:?} started_block_id={:?}",
                        block.id,
                        command.started_block_id
                    );
                }
            }
            _ => (),
        }
    }
}

pub(crate) struct CommandHandle {
    exit_status_rx: oneshot::Receiver<Result<ExitCode, AgentDriverError>>,
    block_id: BlockId,
    command_id: Uuid,
    submitted_at: Instant,
}

impl CommandHandle {
    /// The block ID of the command that was executed.
    pub fn block_id(&self) -> &BlockId {
        &self.block_id
    }
}

impl Future for CommandHandle {
    type Output = Result<ExitCode, AgentDriverError>;

    fn poll(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
        let result = Pin::new(&mut self.exit_status_rx)
            .poll(cx)
            .map(|result| match result {
                Ok(exit_status) => exit_status,
                Err(_) => Err(AgentDriverError::InvalidRuntimeState),
            });
        if let Poll::Ready(status) = &result {
            log::info!(
                "Terminal command lifecycle: event=finished command_id={} block_id={:?} elapsed_ms={} exit_code={:?} exit_status_received={}",
                self.command_id,
                self.block_id,
                self.submitted_at.elapsed().as_millis(),
                status.as_ref().ok().map(|code| code.value()),
                status.is_ok()
            );
        }
        result
    }
}

#[cfg(test)]
#[path = "terminal_tests.rs"]
mod tests;
