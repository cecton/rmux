use std::error::Error;
use std::path::PathBuf;
use std::process::Command;

use rmux_core::command_parser::COMMAND_TABLE;

#[test]
fn rmux_manpage_renders_with_man_l() -> Result<(), Box<dyn Error>> {
    let manpage = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("rmux.1");
    let output = Command::new("man")
        .arg("-l")
        .arg(&manpage)
        .env("MANPAGER", "cat")
        .env("PAGER", "cat")
        .output()?;

    let rendered = String::from_utf8_lossy(&output.stdout);

    assert_eq!(output.status.code(), Some(0));
    assert!(rendered.contains("RMUX"));
    assert!(rendered.contains("list-commands"));
    assert!(rendered.contains("choose-window"));
    assert!(rendered.contains("display-menu"));
    assert!(rendered.contains("display-popup"));
    assert!(rendered.contains("clear-prompt-history"));
    assert!(rendered.contains("show-prompt-history"));
    for entry in COMMAND_TABLE {
        assert!(
            rendered.contains(entry.name),
            "expected rendered manpage to expose command {}",
            entry.name
        );
    }
    Ok(())
}
