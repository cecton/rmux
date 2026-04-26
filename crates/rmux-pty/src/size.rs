pub use rmux_types::TerminalSize;

#[cfg(unix)]
use rustix::termios::Winsize;

#[cfg(unix)]
pub(crate) const fn into_winsize(size: TerminalSize) -> Winsize {
    Winsize {
        ws_row: size.rows,
        ws_col: size.cols,
        ws_xpixel: 0,
        ws_ypixel: 0,
    }
}

#[cfg(unix)]
pub(crate) const fn from_winsize(winsize: Winsize) -> TerminalSize {
    TerminalSize {
        cols: winsize.ws_col,
        rows: winsize.ws_row,
    }
}
