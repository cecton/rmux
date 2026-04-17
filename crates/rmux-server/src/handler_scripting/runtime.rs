use std::future::Future;
use std::process::{Command, Output, Stdio};

use rmux_proto::RmuxError;

use crate::terminal::TerminalProfile;

pub(in super::super) fn spawn_background_async<Fut, Factory>(
    thread_name: &'static str,
    factory: Factory,
) where
    Factory: FnOnce() -> Fut + Send + 'static,
    Fut: Future<Output = ()> + 'static,
{
    let _ = std::thread::Builder::new()
        .name(thread_name.to_owned())
        .spawn(move || {
            let Ok(runtime) = tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
            else {
                return;
            };
            runtime.block_on(factory());
        });
}

pub(super) async fn run_shell_foreground(
    command: String,
    profile: &TerminalProfile,
    show_stderr: bool,
) -> Result<Output, RmuxError> {
    let cwd = profile.cwd().to_path_buf();
    let shell = profile.shell().to_path_buf();
    let environment = profile
        .environment()
        .map(|(name, value)| (name.to_owned(), value.to_owned()))
        .collect::<Vec<_>>();
    tokio::task::spawn_blocking(move || {
        let mut command_builder = Command::new(shell);
        command_builder
            .arg("-c")
            .arg(command)
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .current_dir(cwd)
            .env_clear();
        for (name, value) in environment {
            command_builder.env(name, value);
        }
        if !show_stderr {
            command_builder.stderr(Stdio::piped());
        }
        command_builder.output()
    })
    .await
    .map_err(|error| RmuxError::Server(format!("run-shell task failed: {error}")))?
    .map_err(|error| RmuxError::Server(format!("failed to run shell command: {error}")))
}

pub(super) fn run_shell_status_error(output: &Output) -> String {
    let stderr = String::from_utf8_lossy(&output.stderr);
    let stderr = stderr.trim();

    if stderr.is_empty() {
        format!("run-shell command exited with {}", output.status)
    } else {
        format!("run-shell command exited with {}: {stderr}", output.status)
    }
}

pub(super) async fn shell_condition_is_true(
    command: String,
    profile: &TerminalProfile,
) -> Result<bool, RmuxError> {
    let cwd = profile.cwd().to_path_buf();
    let shell = profile.shell().to_path_buf();
    let environment = profile
        .environment()
        .map(|(name, value)| (name.to_owned(), value.to_owned()))
        .collect::<Vec<_>>();
    tokio::task::spawn_blocking(move || {
        let mut command_builder = Command::new(shell);
        command_builder
            .arg("-c")
            .arg(command)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .current_dir(cwd)
            .env_clear();
        for (name, value) in environment {
            command_builder.env(name, value);
        }
        command_builder.status()
    })
    .await
    .map_err(|error| RmuxError::Server(format!("if-shell condition task failed: {error}")))?
    .map(|status| status.success())
    .map_err(|error| RmuxError::Server(format!("failed to run if-shell condition: {error}")))
}
