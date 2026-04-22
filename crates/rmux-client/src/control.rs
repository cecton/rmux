//! Blocking tmux-compatible control-mode client transport.

use std::io::{self, Read, Write};
use std::net::Shutdown;
use std::os::unix::net::UnixStream;
use std::sync::mpsc;
use std::thread;

use rmux_proto::{
    ClientTerminalContext, ControlMode, ControlModeRequest, Request, Response, CONTROL_CONTROL_END,
    CONTROL_CONTROL_START,
};

use crate::{
    connection::{read_response_frame_exact, Connection, ControlModeUpgrade, ControlTransition},
    ClientError,
};

impl Connection {
    /// Requests a control-mode upgrade and, on success, yields the raw Unix
    /// stream for tmux-compatible text control traffic.
    pub fn begin_control_mode(
        mut self,
        mode: ControlMode,
        client_terminal: ClientTerminalContext,
    ) -> Result<ControlTransition, ClientError> {
        self.write_request(&Request::ControlMode(ControlModeRequest {
            mode,
            client_terminal,
        }))?;
        let response = read_response_frame_exact(self.stream_mut())?;

        match response {
            Response::ControlMode(response) => Ok(ControlTransition::Upgraded(
                self.into_control_upgrade(response)?,
            )),
            other => Ok(ControlTransition::Rejected(other)),
        }
    }
}

/// Drives a control-mode session using the process stdio streams.
pub fn drive_control_mode(
    upgrade: ControlModeUpgrade,
    initial_commands: &[String],
) -> Result<(), ClientError> {
    let stdin = io::stdin();
    let stdout = io::stdout();
    drive_control_mode_with_stdio(upgrade, initial_commands, stdin, stdout.lock())
}

/// Drives a control-mode session using explicit input and output streams.
pub fn drive_control_mode_with_stdio<R, W>(
    upgrade: ControlModeUpgrade,
    initial_commands: &[String],
    mut input: R,
    mut output: W,
) -> Result<(), ClientError>
where
    R: Read + Send + 'static,
    W: Write,
{
    let mode = upgrade.mode();
    if mode.is_control_control() {
        output
            .write_all(CONTROL_CONTROL_START.as_bytes())
            .map_err(ClientError::Io)?;
        output.flush().map_err(ClientError::Io)?;
    }

    let stream = upgrade.into_stream();
    write_initial_commands(&stream, initial_commands)?;
    stream.set_nonblocking(false).map_err(ClientError::Io)?;
    let mut writer = stream.try_clone().map_err(ClientError::Io)?;
    let (stdin_done_tx, stdin_done_rx) = mpsc::sync_channel(1);
    let stdin_thread = thread::spawn(move || {
        let result = io::copy(&mut input, &mut writer).map(|_| ());
        let _ = writer.shutdown(Shutdown::Write);
        let _ = stdin_done_tx.send(result);
    });

    let copy_result = copy_control_output(stream, &mut output).map_err(ClientError::Io);
    let stdin_result = poll_input_thread(&stdin_done_rx)?;
    if copy_result.is_ok() && output_needs_suffix(mode) {
        output
            .write_all(CONTROL_CONTROL_END.as_bytes())
            .map_err(ClientError::Io)?;
        output.flush().map_err(ClientError::Io)?;
    }

    if stdin_result.is_some() {
        stdin_thread
            .join()
            .map_err(|_| ClientError::Io(io::Error::other("control input thread panicked")))?;
    }

    copy_result?;
    if let Some(stdin_result) = stdin_result {
        stdin_result.map_err(ClientError::Io)?;
    }
    Ok(())
}

fn output_needs_suffix(mode: ControlMode) -> bool {
    mode.is_control_control()
}

fn poll_input_thread(
    stdin_done_rx: &mpsc::Receiver<io::Result<()>>,
) -> Result<Option<io::Result<()>>, ClientError> {
    match stdin_done_rx.try_recv() {
        Ok(result) => Ok(Some(result)),
        Err(mpsc::TryRecvError::Empty) => Ok(None),
        Err(mpsc::TryRecvError::Disconnected) => Err(ClientError::Io(io::Error::other(
            "control input thread terminated unexpectedly",
        ))),
    }
}

fn write_initial_commands(
    stream: &UnixStream,
    initial_commands: &[String],
) -> Result<(), ClientError> {
    if initial_commands.is_empty() {
        return Ok(());
    }

    let mut writer = stream.try_clone().map_err(ClientError::Io)?;
    for command in initial_commands {
        writer
            .write_all(command.as_bytes())
            .and_then(|()| writer.write_all(b"\n"))
            .map_err(ClientError::Io)?;
    }
    Ok(())
}

fn copy_control_output(mut stream: UnixStream, output: &mut impl Write) -> io::Result<()> {
    let mut buffer = [0_u8; 8192];

    loop {
        let bytes_read = stream.read(&mut buffer)?;
        if bytes_read == 0 {
            return Ok(());
        }
        output.write_all(&buffer[..bytes_read])?;
        output.flush()?;
    }
}

#[cfg(test)]
mod tests {
    use std::io::{Cursor, Write};
    use std::sync::mpsc;
    use std::time::Duration;

    use rmux_proto::{ControlMode, ControlModeResponse};

    use super::drive_control_mode_with_stdio;
    use crate::connection::ControlModeUpgrade;

    #[test]
    fn control_control_mode_wraps_output_with_dcs_sequences() {
        let (left, right) = std::os::unix::net::UnixStream::pair().expect("socket pair");
        let writer = std::thread::spawn(move || {
            let mut right = right;
            right.write_all(b"%exit\n").expect("write output");
        });

        let mut output = Vec::new();
        drive_control_mode_with_stdio(
            ControlModeUpgrade {
                response: ControlModeResponse {
                    mode: ControlMode::ControlControl,
                },
                stream: left,
            },
            &[],
            Cursor::new(Vec::<u8>::new()),
            &mut output,
        )
        .expect("control mode succeeds");
        writer.join().expect("writer thread");

        let rendered = String::from_utf8(output).expect("utf8");
        assert!(rendered.starts_with(rmux_proto::CONTROL_CONTROL_START));
        assert!(rendered.contains("%exit\n"));
        assert!(rendered.ends_with(rmux_proto::CONTROL_CONTROL_END));
    }

    #[test]
    fn control_mode_returns_after_server_exit_without_waiting_for_input_eof() {
        let (left, right) = std::os::unix::net::UnixStream::pair().expect("socket pair");
        let (input_reader, input_writer) =
            std::os::unix::net::UnixStream::pair().expect("input socket pair");
        let server = std::thread::spawn(move || {
            let mut right = right;
            right.write_all(b"%exit\n").expect("write exit");
        });
        let (done_tx, done_rx) = mpsc::channel();
        let worker = std::thread::spawn(move || {
            let mut output = Vec::new();
            let result = drive_control_mode_with_stdio(
                ControlModeUpgrade {
                    response: ControlModeResponse {
                        mode: ControlMode::Plain,
                    },
                    stream: left,
                },
                &[],
                input_reader,
                &mut output,
            );
            done_tx
                .send((result, output))
                .expect("report control mode result");
        });

        let done = done_rx.recv_timeout(Duration::from_secs(1));
        drop(input_writer);
        worker.join().expect("worker thread");
        server.join().expect("server thread");

        let (result, output) = done.expect("control mode should exit promptly");
        result.expect("control mode succeeds");
        assert_eq!(String::from_utf8(output).expect("utf8"), "%exit\n");
    }
}
