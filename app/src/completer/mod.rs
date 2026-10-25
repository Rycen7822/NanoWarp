mod wsl_guest_listing;

use std::collections::{HashMap, HashSet};
use std::ops::Deref;
use std::path::Path;
#[cfg(feature = "local_fs")]
use std::path::PathBuf;
use std::sync::Arc;
#[cfg(feature = "local_fs")]
use std::time::Duration;

use anyhow::Result;
use async_trait::async_trait;
use lazy_static::lazy_static;
use smol_str::SmolStr;
use typed_path::{TypedPath, TypedPathBuf};
#[cfg(feature = "local_fs")]
use warp_completer::completer::{
    self, CompleterOptions, CompletionsFallbackStrategy, MatchStrategy,
};
use warp_completer::completer::{
    AliasExpansionResult, CommandExitStatus, CommandOutput, CompletionContext, EngineDirEntry,
    EngineFileType, GeneratorContext, PathCompletionContext, PathSeparators,
    TopLevelCommandCaseSensitivity, expand_command_aliases,
};
use warp_completer::meta::Spanned;
use warp_completer::parsers::ParsedExpression;
#[cfg(feature = "local_fs")]
use warp_completer::parsers::hir::ArgType;
use warp_completer::parsers::hir::{Command, Expression, FlagType};
use warp_completer::signatures::CommandRegistry;
use warp_core::features::FeatureFlag;
use warp_util::path::{EscapeChar, ShellFamily};
#[cfg(feature = "local_fs")]
use warpui::r#async::FutureExt;

use crate::safe_warn;
use crate::terminal::model::session::{ExecuteCommandOptions, Session, SessionType};
use crate::util::AsciiDebug;

lazy_static! {
    pub static ref CURR_DIRECTORY_ENTRY: EngineDirEntry = EngineDirEntry {
        file_name: ".".to_owned(),
        file_type: EngineFileType::Directory,
    };
    pub static ref PARENT_DIRECTORY_ENTRY: EngineDirEntry = EngineDirEntry {
        file_name: "..".to_owned(),
        file_type: EngineFileType::Directory,
    };
    static ref EMPTY_COMMAND_REGISTRY: Arc<CommandRegistry> = Arc::new(CommandRegistry::empty());
}

#[derive(Clone)]
pub struct SessionContext {
    pub session: Arc<Session>,
    command_registry: Arc<CommandRegistry>,
    pub current_working_directory: TypedPathBuf,

    /// Directory listings keyed by absolute path. Callers that must reflect a directory's
    /// current contents should use `refresh_directory_entries` to re-read from disk.
    cached_directory_entries: Arc<dashmap::DashMap<TypedPathBuf, Arc<Vec<EngineDirEntry>>>>,
}

impl SessionContext {
    /// Lists `directory` afresh without permitting optional network-backed reads for local WSL
    /// sessions. Host-only WSL listings are not cached for guest completions.
    pub(crate) async fn refresh_directory_entries(
        &self,
        directory: TypedPathBuf,
    ) -> Arc<Vec<EngineDirEntry>> {
        let result = Arc::new(
            self.list_directory_entries_internal(&directory.to_path(), false)
                .await,
        );
        if !(self.session.is_wsl() && matches!(self.session.session_type(), SessionType::Local)) {
            self.cached_directory_entries
                .insert(directory, result.clone());
        }
        result
    }

    async fn list_directory_entries_internal(
        &self,
        directory: &TypedPath<'_>,
        allow_network_reads: bool,
    ) -> Vec<EngineDirEntry> {
        #[cfg(not(windows))]
        let _ = allow_network_reads;
        match self.session.session_type() {
            SessionType::Local => {
                // The host cannot resolve an `IO_REPARSE_TAG_LX_SYMLINK` over `\\wsl$`
                // (APP-3993): it can't classify a symlink-to-directory correctly, and it can't
                // traverse *through* a symlinked directory to list its contents at all. For
                // completions, a WSL session asks the guest for the listing directly, following
                // symlinks (`-L`) so both problems are avoided at the source, rather than patching
                // up a host listing afterwards. A slow or failing guest falls back to the plain
                // host listing below rather than emptying the completion list.
                #[cfg(windows)]
                if allow_network_reads
                    && self.session.is_wsl()
                    && let Some(entries) = wsl_guest_listing::list_entries(self, directory).await
                {
                    return entries;
                }

                let dir = match self.session.maybe_convert_to_native_path(directory) {
                    Ok(dir) => dir,
                    Err(err) => {
                        log::warn!("Failed to convert path: {err:#}");
                        return Vec::new();
                    }
                };
                // We intentionally use the synchronous `std::fs::read_dir`,
                // despite this being an async function, because the overhead
                // of switching threads is very expensive relative to the
                // amount of work being done.  Converting the a `DirEntry`
                // to `EngineDirEntry` can usually be done without additional
                // syscalls (though one is necessary if the entry is a
                // symlink).
                //
                // It's possible that it would be better to use
                // `async_fs::read_dir` if the directory is on a network mount,
                // but I don't think it's worth optimizing for that case.
                let Some(read_dir) = std::fs::read_dir(dir.as_path()).ok() else {
                    return vec![];
                };

                read_dir
                    .filter_map(|res| res.and_then(EngineDirEntry::try_from).ok())
                    .collect::<Vec<_>>()
            }
            SessionType::WarpifiedRemote { .. } => {
                let env_vars = self
                    .session
                    .path()
                    .as_deref()
                    .map(|path| HashMap::from_iter([("PATH".to_string(), path.to_string())]));

                let Some(ls_command) = ls_script_for_dir(directory) else {
                    return vec![];
                };

                // The in-band command executor doesn't support executing from
                // from an arbitrary directory, so we need to cd into the
                // directory we want within the ls script.
                let command_output_result = self
                    .session
                    .execute_command(
                        &ls_command,
                        None,
                        env_vars,
                        ExecuteCommandOptions::default(),
                    )
                    .await;

                match command_output_result {
                    Ok(command_output) => match command_output.status {
                        CommandExitStatus::Success => {
                            parse_ls_script_output(command_output.output()).unwrap_or_else(|| {
                                log::warn!(
                                    "Executing `ls` on remote box returned malformed or truncated output: `{:?}`",
                                    AsciiDebug(command_output.output())
                                );
                                vec![]
                            })
                        }
                        CommandExitStatus::Failure => {
                            safe_warn!(
                                safe: ("Executing `ls` on remote box failed with non-zero status code."),
                                full: ("Executing `ls` on remote box failed with error: {}", &String::from_utf8_lossy(command_output.output()))
                            );
                            vec![]
                        }
                    },
                    Err(err) => {
                        log::warn!("Executing `ls` on remote box failed with error {err:?}");
                        vec![]
                    }
                }
            }
        }
    }
}

#[async_trait]
impl PathCompletionContext for SessionContext {
    fn home_directory(&self) -> Option<&str> {
        self.session.home_dir()
    }

    fn cdpath(&self) -> Option<&str> {
        self.session.cdpath()
    }

    fn pwd(&self) -> TypedPath<'_> {
        self.current_working_directory.to_path()
    }

    fn shell_family(&self) -> ShellFamily {
        self.session.shell_family()
    }

    async fn list_directory_entries(&self, directory: TypedPathBuf) -> Arc<Vec<EngineDirEntry>> {
        if let Some(entries) = self.cached_directory_entries.get(&directory) {
            return entries.clone();
        }

        let result = self
            .list_directory_entries_internal(&directory.to_path(), true)
            .await;

        let result = Arc::new(result);
        self.cached_directory_entries
            .insert(directory, result.clone());
        result
    }

    fn path_separators(&self) -> PathSeparators {
        self.session.path_separators()
    }
}

#[async_trait]
impl GeneratorContext for SessionContext {
    async fn execute_command_at_pwd(
        &self,
        shell_command: &str,
        session_env_vars: Option<HashMap<String, String>>,
    ) -> Result<CommandOutput> {
        let mut env_vars = session_env_vars.unwrap_or_default();
        // We need to run the command with the PATH var set explicitly even if we have session env vars
        // because if the user opened Warp through a parent process that didn't have the PATH var set
        // (i.e. outside of a shell, for example opening the app via Finder),
        // the subshell won't inherit the PATH var, but we need the PATH var
        // to reference executables we might run as part of generators.
        if let Some(path) = self.session.path().as_deref() {
            env_vars.insert("PATH".to_string(), path.to_string());
        }

        let env_vars_option = if env_vars.is_empty() {
            None
        } else {
            Some(env_vars)
        };

        self.session
            .execute_command(
                shell_command,
                self.pwd().to_str(),
                env_vars_option,
                ExecuteCommandOptions {
                    run_command_in_same_shell_as_session: !FeatureFlag::RunGeneratorsWithCmdExe
                        .is_enabled(),
                },
            )
            .await
    }

    fn supports_parallel_execution(&self) -> bool {
        self.session.supports_parallel_command_execution()
    }
}

impl CompletionContext for SessionContext {
    fn generator_context(&self) -> Option<&dyn GeneratorContext> {
        Some(self)
    }

    fn path_completion_context(&self) -> Option<&dyn PathCompletionContext> {
        Some(self)
    }

    fn top_level_commands(&self) -> Box<dyn Iterator<Item = &str> + '_> {
        Box::new(self.session.top_level_commands())
    }

    fn command_case_sensitivity(&self) -> TopLevelCommandCaseSensitivity {
        self.session.command_case_sensitivity()
    }

    fn escape_char(&self) -> EscapeChar {
        self.session.shell_family().escape_char()
    }

    fn aliases(&self) -> Box<dyn Iterator<Item = (&str, &str)> + '_> {
        let session_aliases = self
            .session
            .aliases()
            .iter()
            .map(|(alias, command)| (alias.as_str(), command.as_str()));
        Box::new(session_aliases)
    }

    fn alias_command(&self, alias: &str) -> Option<&str> {
        self.session.aliases().get(alias).map(Deref::deref)
    }

    fn abbreviations(&self) -> Option<&HashMap<SmolStr, String>> {
        Some(self.session.abbreviations())
    }

    fn functions(&self) -> Option<&HashSet<SmolStr>> {
        Some(self.session.functions())
    }

    fn builtins(&self) -> Option<&HashSet<SmolStr>> {
        Some(self.session.builtins())
    }

    fn command_registry(&self) -> &CommandRegistry {
        &self.command_registry
    }

    fn environment_variable_names(&self) -> Option<&HashSet<SmolStr>> {
        Some(self.session.environment_variable_names())
    }

    fn shell_supports_autocd(&self) -> Option<bool> {
        Some(self.session.shell().supports_autocd())
    }

    fn shell_family(&self) -> Option<ShellFamily> {
        Some(self.session.shell_family())
    }
}

impl SessionContext {
    pub fn new(
        session: impl Into<Arc<Session>>,
        command_registry: Arc<CommandRegistry>,
        current_working_directory: TypedPathBuf,
    ) -> Self {
        Self {
            session: session.into(),
            command_registry,
            current_working_directory,
            cached_directory_entries: Arc::new(Default::default()),
        }
    }
}

/// `CompletionContext` implementation for "global" completions, that provide completions on all
/// commands in the `command_registry` rather than providing session-specific completions.
///
/// This `CompletionContext` is not coupled to a specific session and thus does not provide path or
/// generator execution, which wouldn't have clear semantics without being coupled to a session.
#[derive(Clone)]
pub struct SessionAgnosticContext {
    command_registry: Arc<CommandRegistry>,
}

impl SessionAgnosticContext {
    pub fn new(command_registry: Arc<CommandRegistry>) -> Self {
        Self { command_registry }
    }
}

impl CompletionContext for SessionAgnosticContext {
    fn top_level_commands(&self) -> Box<dyn Iterator<Item = &str> + '_> {
        Box::new(self.command_registry.registered_commands())
    }

    fn command_registry(&self) -> &CommandRegistry {
        &self.command_registry
    }

    fn environment_variable_names(&self) -> Option<&HashSet<SmolStr>> {
        None
    }

    fn shell_supports_autocd(&self) -> Option<bool> {
        None
    }

    fn path_completion_context(&self) -> Option<&dyn PathCompletionContext> {
        None
    }

    fn generator_context(&self) -> Option<&dyn GeneratorContext> {
        None
    }
}

/// List files and directories in a directory; used for completions on a remote machine.
/// This uses `find` instead of `ls` due to the challenges of parsing `ls` output for
/// unusual file names (e.g.: ones including newlines).
/// We intentionally ignore '.' and '..' here as we add those suggestions manually.
fn ls_script_for_dir(directory: &TypedPath) -> Option<String> {
    // We need to cd into the directory we want completions for
    let Some(dir_str) = directory.to_str() else {
        log::warn!("Non-unicode character found in path: `{directory:?}`");
        return None;
    };
    let escaped_dir = warp_util::path::ShellFamily::Posix.shell_escape(dir_str);

    // Get all directories with -print0, which makes all items end in `\0` (null character)
    // Get all files with -print0, which makes all items end in `\0`
    // Separate the two lists with `\0`
    // Ex: `a\0b\0\c\0\0d.txt\0e.txt\0f.txt\0`
    // Then do the same for anything that is not a directory, and call it a 'File'.
    //
    // Follow symlinks when classifying entries, so a symlink to a directory completes as a
    // directory (like a standard terminal).
    let command = format!(
        r#"
cd {escaped_dir} && 
find -L . -maxdepth 1 -type d -print0 &&
printf '%b' '\0' &&
find -L . -maxdepth 1 -not -type d -print0
            "#
    )
    // Ensure all newlines are escaped, and that the command is a single line, since some
    // in-band executors run commands a single line at a time.
    .replace("\n", " ");

    Some(command)
}

/// Parses the null-delimited output of the script `ls_script_for_dir` builds into directory and
/// file entries, or `None` if the output doesn't match that script's guaranteed wire format --
/// a dirs list, a `\0` separator, and a files list, with every entry (including the separator
/// and, when the files list is non-empty, its own entries) `\0`-terminated.
fn parse_ls_script_output(output: &[u8]) -> Option<Vec<EngineDirEntry>> {
    let mut entries = Vec::new();
    let mut segments = output.split(|&byte| byte == b'\0');

    let dirs = segments
        .by_ref()
        // We use two consecutive null characters to separate files and folders, so detect that
        // here. Note that take_while consumes the first entry that returns false -- the
        // dirs/files separator itself.
        .take_while(|segment| !segment.is_empty())
        .filter_map(|segment| dir_entry_from_segment(segment, EngineFileType::Directory));
    entries.extend(dirs);

    // What's left after the separator is the files list followed by its own trailing `\0`,
    // which `find -print0` guarantees on every complete pass, empty or not (an empty pass
    // contributes no bytes of its own, but the separator's `\0` is still the final byte). If
    // nothing is left at all, the separator was never reached: the output was truncated before
    // the second `find` pass ran, or was empty to begin with. If the last remaining segment
    // isn't empty, the files list itself was cut off mid-entry. Either way, this isn't a real,
    // complete listing.
    let remaining: Vec<&[u8]> = segments.collect();
    let (last, files) = remaining.split_last()?;
    if !last.is_empty() {
        return None;
    }

    entries.extend(
        files
            .iter()
            .filter_map(|segment| dir_entry_from_segment(segment, EngineFileType::File)),
    );

    Some(entries)
}

/// Converts one `\0`-delimited `find -print0` segment into an entry, or `None` if it's the `.`
/// entry `find` itself emits, or the segment isn't valid UTF-8.
fn dir_entry_from_segment(segment: &[u8], file_type: EngineFileType) -> Option<EngineDirEntry> {
    let path_str = std::str::from_utf8(segment).ok()?;
    if path_str == "." {
        return None;
    }
    let file_name = Path::new(path_str).file_name()?.to_str()?.to_owned();
    Some(EngineDirEntry {
        file_name,
        file_type,
    })
}

#[cfg(feature = "local_fs")]
const ARG_GENERATOR_VALIDATION_TIMEOUT: Duration = Duration::from_millis(150);

/// Validates that the arg is valid given its type (e.g. filepath exists if it's a filepath arg).
/// This uses a file system call, so this function should be called only in background threads.
#[cfg_attr(not(feature = "local_fs"), allow(unused_variables))]
async fn is_arg_valid(
    full_command: &str,
    arg: &Spanned<ParsedExpression>,
    ctx: &SessionContext,
    session_env_vars: Option<&HashMap<String, String>>,
) -> bool {
    let Expression::ValidatableArgument(arg_types_to_validate) = arg.expression() else {
        return true;
    };
    // The expression shouldn't be parsed as a `ValidatableArgument` if the arg types are empty,
    // but we check this just in case.
    if arg_types_to_validate.is_empty() {
        return true;
    }
    cfg_if::cfg_if! {
        if #[cfg(feature = "local_fs")] {
            // If we have arg types to validate, the arg must pass validation for at least one of them.
            // If the argument has one or more generators, validate these last because they're more expensive
            // and we can check all generators together using completions suggestions.
            let mut has_generator_arg_type = false;
            for arg_type in arg_types_to_validate {
                match arg_type {
                    ArgType::File => {
                        let mut path_arg = PathBuf::from(arg.value().as_str());
                        if path_arg.is_relative()
                            && let Ok(working_dir) = PathBuf::try_from(ctx.current_working_directory.clone()) {
                                path_arg = working_dir.join(path_arg);
                            }
                        if path_arg.is_file() {
                            return true;
                        }
                    }
                    ArgType::Folder => {
                        let mut path_arg = PathBuf::from(arg.value().as_str());
                        if path_arg.is_relative()
                            && let Ok(working_dir) = PathBuf::try_from(ctx.current_working_directory.clone()) {
                                path_arg = working_dir.join(path_arg);
                            }
                        if path_arg.is_dir() {
                            return true;
                        }
                    }
                    ArgType::Generator(_) => {
                        has_generator_arg_type = true;
                    }
                };
            }
            if has_generator_arg_type {
                // We don't have completions implemented for feature flags like --features=with_local_server.
                // If arg is the span of `with_local_server`, attempting to complete on --features= to validate it will return no results.
                // We should only use completions to validate the arg if the previous character is whitespace, until completions handles this case.
                let prev_char = full_command.get(..arg.span.start()).and_then(|s| s.chars().next_back());
                if prev_char.is_some_and(|c| !c.is_whitespace()) {
                    return true;
                }
                // Running completions runs all generators, so we only need to do this once.
                // TODO(roland): this also generates completions from sources other than generators, which are unnecessary.
                // If performance becomes a concern, consider validating against generators sequentially and returning early if valid.
                // We use completions suggestions because it's simpler to implement and read.
                let completions_future = completer::suggestions(
                    full_command,
                    arg.span.start(),
                    session_env_vars,
                    CompleterOptions {
                        match_strategy: MatchStrategy::CaseSensitive,
                        fallback_strategy: CompletionsFallbackStrategy::None,
                        suggest_file_path_completions_only: false,
                        parse_quotes_as_literals: false,
                    },
                    ctx,
                );

                // If the completions call times out, assume the arg is valid.
                // This is necessary because some generators can hang (e.g. kubectl commands if the cluster isn't running).
                let Ok(completion_result) = completions_future.with_timeout(ARG_GENERATOR_VALIDATION_TIMEOUT).await else {
                    log::debug!("Generator validation for arg `{}` in command `{}` timed out - assuming it's valid", arg.value().as_str(), full_command);
                    return true;
                };

                let Some(completion_result) = completion_result else {
                    return true;
                };
                for suggestion in completion_result.suggestions {
                    if suggestion.display() == arg.value().as_str() {
                        return true;
                    }
                }
            }
            // If we didn't pass validation for any of the possible arg types, this arg is invalid.
            log::debug!("arg `{}` in command `{}` failed validation", arg.value().as_str(), full_command);
            false
        } else {
            true
        }
    }
}

/// Validates the command is valid.
/// Currently uses completions specs to check if parsing is successful, and validates
/// that any filepaths args actually exist on disk.
/// This uses a file system call, so this function should be called only in background threads.
pub(crate) async fn is_command_valid(
    command: &str,
    ctx: Option<&SessionContext>,
    session_env_vars: Option<&HashMap<String, String>>,
) -> bool {
    if !FeatureFlag::ValidateAutosuggestions.is_enabled() {
        return true;
    }
    let Some(ctx) = ctx else {
        return true;
    };
    let AliasExpansionResult {
        expanded_command_line,
        classified_command,
        ..
    } = expand_command_aliases(command, false, ctx).await;

    let Some(classified_command) = classified_command else {
        return true;
    };

    // We assume the command is valid on parse error because
    // 1. Our completion specs are not always comprehensive (unknown args/options cause parse error)
    // 2. Our parsing logic has some bugs that need to be investigated (INT-816)
    if classified_command.error.is_some() {
        log::debug!(
            "Assuming command `{}` is valid because it failed to parse: {:?}",
            expanded_command_line,
            classified_command.error.unwrap()
        );
        return true;
    }
    // If we can't classify the command, it means we don't have completion specs for it.
    // Assume it's valid.
    let Command::Classified(shell_command) = classified_command.command else {
        return true;
    };
    if let Some(positionals) = &shell_command.args.positionals {
        for positional in positionals {
            if !is_arg_valid(&expanded_command_line, positional, ctx, session_env_vars).await {
                return false;
            }
        }
    }
    if let Some(flags) = shell_command.args.flags {
        for flag in flags.iter() {
            if let FlagType::Argument { value } = &flag.flag_type
                && !is_arg_valid(&expanded_command_line, value, ctx, session_env_vars).await
            {
                return false;
            }
        }
    }
    true
}

#[cfg(test)]
#[path = "test.rs"]
mod tests;
