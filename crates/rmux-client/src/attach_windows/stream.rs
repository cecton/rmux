use std::io::{self, Write};

use rmux_proto::{
    encode_attach_message, AttachFrameDecoder, AttachMessage, AttachedKeystroke, RmuxError,
    TerminalSize,
};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::sync::mpsc;

use crate::ClientError;

use super::metrics::AttachMetricsRecorder;
use super::screen::{
    contains_subslice, AttachScreenTracker, AttachStopDetector, ALT_SCREEN_EXIT_FALLBACK,
    DETACHED_BANNER_PREFIX, EXITED_BANNER,
};

pub(super) async fn drive_async_attach<Reader, Writer, Output>(
    reader: Reader,
    writer: Writer,
    initial_bytes: Vec<u8>,
    input_rx: mpsc::UnboundedReceiver<Vec<u8>>,
    resize_rx: mpsc::UnboundedReceiver<TerminalSize>,
    output: Output,
    screen_tracker: AttachScreenTracker,
) -> std::result::Result<(), ClientError>
where
    Reader: tokio::io::AsyncRead + Unpin,
    Writer: tokio::io::AsyncWrite + Unpin,
    Output: Write,
{
    let mut metrics = AttachMetricsRecorder::from_env();
    let result = drive_async_attach_loop(
        reader,
        writer,
        initial_bytes,
        input_rx,
        resize_rx,
        output,
        screen_tracker,
        &mut metrics,
    )
    .await;
    metrics.flush();
    result
}

#[allow(clippy::too_many_arguments)]
async fn drive_async_attach_loop<Reader, Writer, Output>(
    mut reader: Reader,
    mut writer: Writer,
    initial_bytes: Vec<u8>,
    mut input_rx: mpsc::UnboundedReceiver<Vec<u8>>,
    mut resize_rx: mpsc::UnboundedReceiver<TerminalSize>,
    mut output: Output,
    screen_tracker: AttachScreenTracker,
    metrics: &mut AttachMetricsRecorder,
) -> std::result::Result<(), ClientError>
where
    Reader: tokio::io::AsyncRead + Unpin,
    Writer: tokio::io::AsyncWrite + Unpin,
    Output: Write,
{
    let mut decoder = AttachFrameDecoder::new();
    decoder.push_bytes(&initial_bytes);
    let mut read_buffer = [0_u8; super::READ_BUFFER_SIZE];
    let mut stop_detector = AttachStopDetector::new(screen_tracker.clone());

    loop {
        if drain_attach_messages(
            &mut decoder,
            &mut writer,
            &mut output,
            &screen_tracker,
            &mut stop_detector,
            metrics,
        )
        .await?
        .should_exit()
        {
            return Ok(());
        }

        tokio::select! {
            bytes = input_rx.recv() => {
                let Some(bytes) = bytes else {
                    continue;
                };
                for chunk in super::input::attach_input_chunks(&bytes) {
                    write_async_attach_message(
                        &mut writer,
                        AttachMessage::Keystroke(AttachedKeystroke::new(chunk.to_vec())),
                    ).await?;
                }
            }
            size = resize_rx.recv() => {
                let Some(size) = size else {
                    continue;
                };
                write_async_attach_message(
                    &mut writer,
                    AttachMessage::Resize(size),
                ).await?;
            }
            read = reader.read(&mut read_buffer) => {
                let bytes_read = match read {
                    Ok(0) => {
                        if screen_tracker.was_stopped() {
                            return Ok(());
                        }
                        return Err(ClientError::Io(io::Error::new(
                            io::ErrorKind::UnexpectedEof,
                            "attach stream closed before attach-stop sequence",
                        )));
                    }
                    Ok(bytes_read) => bytes_read,
                    Err(error) if error.kind() == io::ErrorKind::Interrupted => continue,
                    Err(error)
                        if screen_tracker.was_stopped()
                            && matches!(
                                error.kind(),
                                io::ErrorKind::ConnectionReset | io::ErrorKind::BrokenPipe
                            ) =>
                    {
                        return Ok(());
                    }
                    Err(error) => return Err(ClientError::Io(error)),
                };
                decoder.push_bytes(&read_buffer[..bytes_read]);
            }
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum DrainOutcome {
    Continue,
    Exit,
}

impl DrainOutcome {
    const fn should_exit(self) -> bool {
        matches!(self, Self::Exit)
    }
}

async fn drain_attach_messages<Writer, Output>(
    decoder: &mut AttachFrameDecoder,
    writer: &mut Writer,
    output: &mut Output,
    screen_tracker: &AttachScreenTracker,
    stop_detector: &mut AttachStopDetector,
    metrics: &mut AttachMetricsRecorder,
) -> std::result::Result<DrainOutcome, ClientError>
where
    Writer: tokio::io::AsyncWrite + Unpin,
    Output: Write,
{
    while let Some(message) = decoder.next_message().map_err(ClientError::from)? {
        match message {
            AttachMessage::Data(bytes) => {
                metrics.observe_data_frame(&bytes);
                if contains_subslice(&bytes, ALT_SCREEN_EXIT_FALLBACK)
                    || contains_subslice(&bytes, DETACHED_BANNER_PREFIX)
                    || contains_subslice(&bytes, EXITED_BANNER)
                {
                    screen_tracker.mark_stopped();
                }
                stop_detector.observe(&bytes);
                output.write_all(&bytes).map_err(ClientError::Io)?;
                output.flush().map_err(ClientError::Io)?;
            }
            AttachMessage::KeyDispatched(_) => {}
            AttachMessage::DetachKill | AttachMessage::DetachExec(_) => {
                return Ok(DrainOutcome::Exit);
            }
            AttachMessage::Lock(_) | AttachMessage::Suspend => {
                write_async_attach_message(writer, AttachMessage::Unlock).await?;
            }
            AttachMessage::Resize(_) => {
                return Err(ClientError::Protocol(RmuxError::Decode(
                    "received unexpected resize message from attach stream".to_owned(),
                )));
            }
            AttachMessage::Unlock => {
                return Err(ClientError::Protocol(RmuxError::Decode(
                    "received unexpected unlock message from attach stream".to_owned(),
                )));
            }
            AttachMessage::Keystroke(_) => {
                return Err(ClientError::Protocol(RmuxError::Decode(
                    "received unexpected keystroke message from attach stream".to_owned(),
                )));
            }
        }
    }

    Ok(DrainOutcome::Continue)
}

async fn write_async_attach_message<Writer>(
    writer: &mut Writer,
    message: AttachMessage,
) -> std::result::Result<(), ClientError>
where
    Writer: tokio::io::AsyncWrite + Unpin,
{
    let frame = encode_attach_message(&message).map_err(ClientError::from)?;
    writer.write_all(&frame).await.map_err(ClientError::Io)
}
