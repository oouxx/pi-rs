//! Terminal setup with ratatui and crossterm.
//!
//! Handles raw mode, bracketed paste, and input event streaming.
//!
//! NOTE: the kitty keyboard protocol is NOT enabled here. Enabling it
//! unconditionally breaks Ctrl+C/Ctrl+D on terminals that do not implement
//! the protocol (crossterm 0.28's CSI-u parsing then fails to decode the
//! key). Proper support requires probing the terminal first (grok-build's
//! `terminal/probe.rs` + `kitty_keyboard.rs`); until then, standard
//! crossterm key events keep every key working everywhere.

use std::io::{self, stdout, Stdout};

use crossterm::event::{
    DisableBracketedPaste, EnableBracketedPaste, Event, EventStream, KeyEvent, MouseEventKind,
};
use crossterm::execute;
use futures::StreamExt;
use ratatui::backend::CrosstermBackend;
use tokio::sync::mpsc;

/// Terminal abstraction wrapping ratatui's CrosstermBackend.
pub struct Terminal {
    inner: ratatui::Terminal<CrosstermBackend<Stdout>>,
    columns: u16,
    rows: u16,
}

impl Terminal {
    pub fn new() -> io::Result<Self> {
        let backend = CrosstermBackend::new(stdout());
        let inner = ratatui::Terminal::new(backend)?;
        let (cols, rows) = crossterm::terminal::size().unwrap_or((80, 24));
        Ok(Self { inner, columns: cols, rows })
    }

    pub fn columns(&self) -> u16 { self.columns }
    pub fn rows(&self) -> u16 { self.rows }

    /// Enter raw mode, enable bracketed paste + mouse capture, start the
    /// input event loop. Returns (input_rx, shutdown_guard).
    pub fn start(&mut self) -> io::Result<(mpsc::UnboundedReceiver<InputEvent>, ShutdownGuard)> {
        crossterm::terminal::enable_raw_mode()?;

        execute!(io::stdout(), EnableBracketedPaste)?;
        // 滚轮滚动 transcript（内部 scrollback 的输入源）。
        execute!(io::stdout(), crossterm::event::EnableMouseCapture)?;

        let (input_tx, input_rx) = mpsc::unbounded_channel();
        let (shutdown_tx, mut shutdown_rx) = tokio::sync::oneshot::channel::<()>();

        let mut event_stream = EventStream::new();
        tokio::spawn(async move {
            loop {
                tokio::select! {
                    _ = &mut shutdown_rx => break,
                    event = event_stream.next() => {
                        match event {
                            Some(Ok(Event::Key(key_event))) => {
                                if key_event.kind == crossterm::event::KeyEventKind::Release {
                                    // IME composition may emit characters as release events
                                    if key_event.code != crossterm::event::KeyCode::Char('\0') {
                                        let _ = input_tx.send(InputEvent::Key(key_event));
                                    }
                                    continue;
                                }
                                let _ = input_tx.send(InputEvent::Key(key_event));
                            }
                            Some(Ok(Event::Paste(text))) => {
                                // Forward pasted text (IME composition, clipboard) as char-by-char key events
                                for ch in text.chars() {
                                    let ev = KeyEvent::new(
                                        crossterm::event::KeyCode::Char(ch),
                                        crossterm::event::KeyModifiers::NONE,
                                    );
                                    let _ = input_tx.send(InputEvent::Key(ev));
                                }
                            }
                            Some(Ok(Event::Mouse(m))) => {
                                // 滚轮 → 滚动事件（wheelScrollLines = 1，对齐 TS）。
                                match m.kind {
                                    MouseEventKind::ScrollUp => {
                                        let _ = input_tx.send(InputEvent::ScrollUp);
                                    }
                                    MouseEventKind::ScrollDown => {
                                        let _ = input_tx.send(InputEvent::ScrollDown);
                                    }
                                    _ => {}
                                }
                            }
                            Some(Ok(_)) => {}
                            Some(Err(_)) => break,
                            None => break,
                        }
                    }
                }
            }
            let _ = crossterm::terminal::disable_raw_mode();
            let _ = crossterm::execute!(std::io::stdout(), crossterm::event::DisableMouseCapture);
        });

        Ok((input_rx, ShutdownGuard { sender: Some(shutdown_tx) }))
    }

    pub fn ratatui_terminal(&mut self) -> &mut ratatui::Terminal<CrosstermBackend<Stdout>> {
        &mut self.inner
    }

    pub fn clear_screen(&mut self) -> io::Result<()> {
        execute!(
            io::stdout(),
            crossterm::terminal::Clear(crossterm::terminal::ClearType::All)
        )?;
        Ok(())
    }

    pub fn refresh_size(&mut self) {
        if let Ok((cols, rows)) = crossterm::terminal::size() {
            self.columns = cols;
            self.rows = rows;
        }
    }
}

/// Guard that shuts down the terminal input loop on drop.
/// Unified input event: keys plus mouse-wheel scroll (the internal
/// scrollback's input source).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum InputEvent {
    Key(KeyEvent),
    ScrollUp,
    ScrollDown,
}

pub struct ShutdownGuard {
    sender: Option<tokio::sync::oneshot::Sender<()>>,
}

impl ShutdownGuard {
    pub fn shutdown(mut self) {
        if let Some(tx) = self.sender.take() {
            let _ = tx.send(());
        }
        let _ = crossterm::terminal::disable_raw_mode();
        let _ = execute!(io::stdout(), DisableBracketedPaste, crossterm::cursor::Show);
    }
}

impl Drop for ShutdownGuard {
    fn drop(&mut self) {
        if let Some(tx) = self.sender.take() {
            let _ = tx.send(());
        }
        let _ = crossterm::terminal::disable_raw_mode();
    }
}
