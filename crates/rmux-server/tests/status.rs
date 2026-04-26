#![cfg(unix)]

mod common;

use std::error::Error;
use std::io;
use std::time::Duration;

use common::{session_name, start_server, ClientConnection, TestHarness, PTY_TEST_LOCK};
use rmux_proto::{
    AttachMessage, AttachSessionRequest, NewSessionRequest, OptionName, Request, Response,
    ScopeSelector, SetOptionMode, SetOptionRequest, TerminalSize,
};
use tokio::io::AsyncReadExt;
use tokio::time::{timeout, Instant};

const STEP_TIMEOUT: Duration = Duration::from_secs(3);

#[tokio::test]
async fn attach_session_emits_status_row_for_single_pane_session() -> Result<(), Box<dyn Error>> {
    let _guard = PTY_TEST_LOCK.lock().await;
    let harness = TestHarness::new("status-attach");
    let socket_path = harness.socket_path().to_path_buf();
    let handle = start_server(&harness).await?;
    let alpha = session_name("alpha");

    let created = common::send_request(
        &socket_path,
        &Request::NewSession(NewSessionRequest {
            session_name: alpha.clone(),
            detached: true,
            size: Some(TerminalSize { cols: 20, rows: 4 }),
            environment: None,
        }),
    )
    .await?;
    assert!(matches!(created, Response::NewSession(_)));

    let (_response, mut attach_stream) = ClientConnection::connect(&socket_path)
        .await?
        .begin_attach(AttachSessionRequest { target: alpha })
        .await?;

    let status_text = read_attach_data_until_contains(&mut attach_stream, "[alpha]").await?;
    assert!(status_text.contains("[alpha]"));
    assert!(status_text.contains("\u{1b}[4;1H"));
    assert!(!status_text.contains('┬'));

    drop(attach_stream);
    handle.shutdown().await?;
    Ok(())
}

#[tokio::test]
async fn attach_session_status_context_populates_session_attached() -> Result<(), Box<dyn Error>> {
    let _guard = PTY_TEST_LOCK.lock().await;
    let harness = TestHarness::new("status-session-attached");
    let socket_path = harness.socket_path().to_path_buf();
    let handle = start_server(&harness).await?;
    let alpha = session_name("alpha");

    let created = common::send_request(
        &socket_path,
        &Request::NewSession(NewSessionRequest {
            session_name: alpha.clone(),
            detached: true,
            size: Some(TerminalSize { cols: 30, rows: 4 }),
            environment: None,
        }),
    )
    .await?;
    assert!(matches!(created, Response::NewSession(_)));

    for (option, value) in [
        (OptionName::StatusLeft, "attached=#{session_attached}"),
        (OptionName::StatusRight, ""),
    ] {
        let response = common::send_request(
            &socket_path,
            &Request::SetOption(SetOptionRequest {
                scope: ScopeSelector::Session(alpha.clone()),
                option,
                value: value.to_owned(),
                mode: SetOptionMode::Replace,
            }),
        )
        .await?;
        assert!(matches!(response, Response::SetOption(_)));
    }

    let (_response, mut attach_stream) = ClientConnection::connect(&socket_path)
        .await?
        .begin_attach(AttachSessionRequest { target: alpha })
        .await?;

    let status_text = read_attach_data_until_contains(&mut attach_stream, "attached=1").await?;
    assert!(status_text.contains("attached=1"));

    drop(attach_stream);
    handle.shutdown().await?;
    Ok(())
}

async fn read_attach_data_until_contains(
    stream: &mut tokio::net::UnixStream,
    needle: &str,
) -> Result<String, Box<dyn Error>> {
    let deadline = Instant::now() + STEP_TIMEOUT;
    let mut output = String::new();

    while Instant::now() < deadline {
        let message = match timeout(
            deadline.saturating_duration_since(Instant::now()),
            read_attach_message(stream),
        )
        .await
        {
            Ok(message) => message?,
            Err(_) => break,
        };

        let Some(message) = message else {
            break;
        };

        if let AttachMessage::Data(bytes) = message {
            output.push_str(&String::from_utf8_lossy(&bytes));
            if output.contains(needle) {
                return Ok(output);
            }
        }
    }

    Err(io::Error::other(format!(
        "attach stream never included expected status marker {needle:?}; output was {output:?}"
    ))
    .into())
}

async fn read_attach_message(
    stream: &mut tokio::net::UnixStream,
) -> Result<Option<AttachMessage>, Box<dyn Error>> {
    let mut tag = [0_u8; 1];
    let bytes_read = stream.read(&mut tag).await?;
    if bytes_read == 0 {
        return Ok(None);
    }

    match tag[0] {
        1 => {
            let mut length = [0_u8; 4];
            stream.read_exact(&mut length).await?;
            let payload_len = u32::from_le_bytes(length) as usize;
            let mut payload = vec![0_u8; payload_len];
            stream.read_exact(&mut payload).await?;
            Ok(Some(AttachMessage::Data(payload)))
        }
        2 => {
            let mut size = [0_u8; 4];
            stream.read_exact(&mut size).await?;
            Ok(Some(AttachMessage::Resize(rmux_proto::TerminalSize {
                cols: u16::from_le_bytes([size[0], size[1]]),
                rows: u16::from_le_bytes([size[2], size[3]]),
            })))
        }
        other => Err(rmux_proto::RmuxError::Decode(format!(
            "unknown attach-stream message tag {other}"
        ))
        .into()),
    }
}
