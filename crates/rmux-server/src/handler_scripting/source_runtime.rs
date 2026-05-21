use std::path::Path;
use std::sync::atomic::Ordering;

use rmux_core::{
    command_parser::{CommandParser, ParsedCommands},
    formats::FormatContext,
};
use rmux_proto::{
    CommandOutput, ErrorResponse, PaneTarget, Response, RmuxError, SourceFileRequest,
    SourceFileResponse, Target,
};

use super::super::RequestHandler;
use super::format_context::{format_context_for_target, parser_with_parse_time_context};
use super::queue::{QueueCommandAction, QueueExecutionContext};
use super::source_files::{
    default_config_paths, default_tmux_fallback_paths, source_inputs_for_path, source_parse_error,
    LoadedSourceFile, ParsedSourceFileCommand, SourceInput, SourcedParsedCommands,
};
use super::targets::active_session_target;
use crate::format_runtime::{render_runtime_template, RuntimeFormatContext};
use crate::{ConfigFileSelection, ConfigLoadOptions};

impl RequestHandler {
    pub(crate) async fn load_startup_config(&self, config_load: ConfigLoadOptions) {
        self.config_loading_depth.fetch_add(1, Ordering::Relaxed);
        let queue_errors = !matches!(config_load.selection(), ConfigFileSelection::Files(_));
        let (paths, tmux_fallback_paths) = match config_load.selection() {
            ConfigFileSelection::Disabled => {
                self.config_loading_depth.fetch_sub(1, Ordering::Relaxed);
                return;
            }
            ConfigFileSelection::Default => (default_config_paths(), default_tmux_fallback_paths()),
            ConfigFileSelection::Files(files) => (
                files
                    .iter()
                    .map(|path| path.to_string_lossy().into_owned())
                    .collect(),
                Vec::new(),
            ),
        };

        let command = ParsedSourceFileCommand {
            paths,
            quiet: true,
            parse_only: false,
            verbose: false,
            expand_paths: false,
            target: None,
            caller_cwd: config_load.cwd().map(Path::to_path_buf),
            stdin: None,
            current_file: None,
        };

        let loaded = match self.load_source_file_command(&command, 1).await {
            Ok(loaded) => loaded,
            Err(error) => {
                if queue_errors {
                    self.startup_config_errors.lock().await.push(error);
                }
                self.config_loading_depth.fetch_sub(1, Ordering::Relaxed);
                return;
            }
        };

        let (mut loaded, is_tmux_fallback) = if loaded.is_empty() && !tmux_fallback_paths.is_empty()
        {
            let fallback_command = ParsedSourceFileCommand {
                paths: tmux_fallback_paths,
                quiet: true,
                ..command.clone()
            };
            match self.load_source_file_command(&fallback_command, 1).await {
                Ok(loaded) => (loaded, true),
                Err(_) => {
                    self.config_loading_depth.fetch_sub(1, Ordering::Relaxed);
                    return;
                }
            }
        } else {
            (loaded, false)
        };

        let mut errors = Vec::new();
        if let Some(error) = loaded.take_error() {
            errors.push(error);
        }
        if let Err(error) = self
            .execute_loaded_source_file(
                std::process::id(),
                loaded,
                QueueExecutionContext::new(command.caller_cwd.clone()),
                1,
            )
            .await
        {
            if !is_tmux_fallback {
                errors.push(error);
            }
        }
        if queue_errors {
            if let Some(error) = super::aggregate_rmux_errors(errors) {
                self.startup_config_errors.lock().await.push(error);
            }
        }
        self.config_loading_depth.fetch_sub(1, Ordering::Relaxed);
    }

    pub(in crate::handler) async fn handle_source_file(
        &self,
        requester_pid: u32,
        request: SourceFileRequest,
    ) -> Response {
        let mut command = ParsedSourceFileCommand::from(request);
        if command.target.is_none() {
            command.target = self.implicit_source_file_target(requester_pid).await;
        }
        let mut loaded = match self.load_source_file_command(&command, 1).await {
            Ok(loaded) => loaded,
            Err(error) => return Response::Error(ErrorResponse { error }),
        };
        let mut errors = Vec::new();
        if let Some(error) = loaded.take_error() {
            errors.push(error);
        }

        let mut stdout = std::mem::take(&mut loaded.stdout);
        if !command.parse_only {
            match self
                .execute_loaded_source_file(
                    requester_pid,
                    loaded,
                    QueueExecutionContext::new(command.caller_cwd.clone())
                        .with_current_target(command.target.clone().map(Target::Pane)),
                    1,
                )
                .await
            {
                Ok(output) => stdout.extend_from_slice(output.stdout()),
                Err(error) => errors.push(error),
            }
        }

        if let Some(error) = super::aggregate_rmux_errors(errors) {
            return Response::Error(ErrorResponse { error });
        }

        if stdout.is_empty() {
            Response::SourceFile(SourceFileResponse::no_output())
        } else {
            Response::SourceFile(SourceFileResponse::from_output(CommandOutput::from_stdout(
                stdout,
            )))
        }
    }

    async fn implicit_source_file_target(&self, requester_pid: u32) -> Option<PaneTarget> {
        let session_name = match self.current_session_candidate(requester_pid).await {
            Some(session_name) => Some(session_name),
            None => self.preferred_session_name().await.ok(),
        }?;
        let state = self.state.lock().await;
        match active_session_target(&state.sessions, &session_name) {
            Some(Target::Pane(target)) => Some(target),
            _ => None,
        }
    }

    pub(super) async fn execute_queued_source_file(
        &self,
        _requester_pid: u32,
        mut command: ParsedSourceFileCommand,
        context: &QueueExecutionContext,
    ) -> Result<QueueCommandAction, RmuxError> {
        let depth = context.source_file_depth.saturating_add(1);
        command.current_file = context.current_file.clone();
        let mut loaded = self.load_source_file_command(&command, depth).await?;
        let error = loaded.take_error();

        if command.parse_only || loaded.is_empty() {
            return Ok(QueueCommandAction::Normal {
                output: nonempty_stdout(loaded.stdout),
                error,
            });
        }

        Ok(QueueCommandAction::InsertAfter {
            batches: loaded
                .commands
                .into_iter()
                .map(|batch| {
                    (
                        batch.commands,
                        context.for_sourced_commands(depth, batch.current_file),
                    )
                })
                .collect(),
            output: nonempty_stdout(loaded.stdout),
            error,
        })
    }

    async fn execute_loaded_source_file(
        &self,
        requester_pid: u32,
        loaded: LoadedSourceFile,
        context: QueueExecutionContext,
        depth: usize,
    ) -> Result<CommandOutput, RmuxError> {
        let mut stdout = Vec::new();
        let mut errors = Vec::new();
        for batch in loaded.commands {
            match self
                .execute_parsed_commands(
                    requester_pid,
                    batch.commands,
                    context.for_sourced_commands(depth, batch.current_file),
                )
                .await
            {
                Ok(output) => stdout.extend_from_slice(output.stdout()),
                Err(error) => errors.push(error),
            }
        }

        match super::aggregate_rmux_errors(errors) {
            Some(error) => Err(error),
            None => Ok(CommandOutput::from_stdout(stdout)),
        }
    }

    async fn load_source_file_command(
        &self,
        command: &ParsedSourceFileCommand,
        depth: usize,
    ) -> Result<LoadedSourceFile, RmuxError> {
        if depth > super::SOURCE_FILE_NESTING_LIMIT {
            return Err(RmuxError::Server("too many nested files".to_owned()));
        }

        let mut loaded = LoadedSourceFile::default();

        for path in &command.paths {
            let expanded_path = if command.expand_paths {
                self.render_source_file_path(
                    path,
                    command.target.as_ref(),
                    command.current_file.as_deref(),
                )
                .await?
            } else {
                path.clone()
            };
            let inputs = match source_inputs_for_path(
                &expanded_path,
                command.caller_cwd.as_deref(),
                command.quiet,
                command.stdin.as_deref(),
            ) {
                Ok(inputs) => inputs,
                Err(error) => {
                    loaded.push_error(error);
                    continue;
                }
            };
            for input in inputs {
                let parsed = match self
                    .parse_source_input(&input, command.target.as_ref())
                    .await
                {
                    Ok(parsed) => parsed,
                    Err(error) => {
                        loaded.push_error(error);
                        continue;
                    }
                };
                if command.verbose {
                    append_verbose_commands(&mut loaded.stdout, &parsed);
                }
                if !command.parse_only {
                    loaded.commands.push(SourcedParsedCommands {
                        commands: parsed,
                        current_file: Some(input.current_file.clone()),
                    });
                }
            }
        }

        Ok(loaded)
    }

    async fn render_source_file_path(
        &self,
        path: &str,
        target: Option<&PaneTarget>,
        current_file: Option<&str>,
    ) -> Result<String, RmuxError> {
        let attached_count = if let Some(target) = target {
            self.attached_count(target.session_name()).await
        } else {
            0
        };
        let state = self.state.lock().await;
        let mut context = match target {
            Some(target) => {
                format_context_for_target(&state, &Target::Pane(target.clone()), attached_count)?
            }
            None => RuntimeFormatContext::new(FormatContext::new()).with_state(&state),
        };

        if let Some(current_file) = current_file {
            context = context.with_named_value("current_file", current_file);
        }
        Ok(render_runtime_template(path, &context, false))
    }

    async fn parse_source_input(
        &self,
        input: &SourceInput,
        target: Option<&PaneTarget>,
    ) -> Result<ParsedCommands, RmuxError> {
        let attached_count = if let Some(target) = target {
            self.attached_count(target.session_name()).await
        } else {
            0
        };
        let state = self.state.lock().await;
        let mut parser = CommandParser::new().with_environment_store(&state.environment);
        let context = match target {
            Some(target) => {
                format_context_for_target(&state, &Target::Pane(target.clone()), attached_count)?
                    .with_named_value("current_file", &input.current_file)
            }
            None => RuntimeFormatContext::new(
                FormatContext::new().with_named_value("current_file", &input.current_file),
            )
            .with_state(&state),
        };
        parser = parser_with_parse_time_context(parser, &context);
        parser
            .parse(&input.contents)
            .map_err(|error| source_parse_error(input, error))
    }
}

fn append_verbose_commands(stdout: &mut Vec<u8>, parsed: &ParsedCommands) {
    if parsed.is_empty() {
        return;
    }
    stdout.extend_from_slice(parsed.to_tmux_string().as_bytes());
    stdout.push(b'\n');
}

fn nonempty_stdout(stdout: Vec<u8>) -> Option<CommandOutput> {
    if stdout.is_empty() {
        None
    } else {
        Some(CommandOutput::from_stdout(stdout))
    }
}
