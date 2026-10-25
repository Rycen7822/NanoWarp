use async_channel::Sender;
pub use warp_terminal::shell::{shell_escape_single_quotes, shell_quote_arg};
use warp_util::path::ShellFamily;

use crate::terminal::model::session::command_executor::{
    InBandCommand, InBandCommandCancelledEvent,
};
use crate::terminal::shell::ShellType;

pub(super) fn serialize_environment_variables<'s>(
    pairs: impl IntoIterator<Item = (&'s str, &'s str)>,
    shell_type: ShellType,
) -> String {
    let (prefix, separator, postfix) = match shell_type {
        ShellType::Fish => ("set -x ", " ", ";"),
        ShellType::Bash | ShellType::Zsh => ("", "=", ""),
        ShellType::PowerShell => ("$env:", " = ", ";"),
    };
    let shell_family = ShellFamily::from(shell_type);
    pairs
        .into_iter()
        .map(|(name, value)| {
            let name = shell_family.escape(name);
            let value = match shell_family {
                ShellFamily::Posix => shell_family.escape(value).into_owned(),
                ShellFamily::PowerShell => format!("'{}'", value.replace("'", "''")),
            };
            format!("{prefix}{name}{separator}{value}{postfix}")
        })
        .collect::<Vec<_>>()
        .join(" ")
}

/// Set of events sent by command executors.
pub enum ExecutorCommandEvent {
    /// The command should be executed.
    ExecuteCommand {
        command: InBandCommand,
        /// A Sender that can be used to signal that the command has been cancelled.
        /// Lets us unblock the command in the executor.
        cancel_tx: Sender<InBandCommandCancelledEvent>,
    },
    /// The command identified by `id` should be cancelled.
    CancelCommand { id: String },
}
