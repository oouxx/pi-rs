//! Terminal setup with ratatui and crossterm (raw mode, bracketed paste, input
//! event streaming).
//!
//! NOTE: the kitty keyboard protocol is NOT enabled — enabling it unconditionally
//! breaks Ctrl+C/Ctrl+D on terminals that don't implement it (crossterm 0.28's
//! CSI-u parsing fails). Proper support needs terminal probing first.
use std::io::{self, stdout, Stdout};

use crossterm::event::{
    DisableBracketedPaste, DisableFocusChange, EnableBracketedPaste, EnableFocusChange, Event,
    EventStream, KeyEvent, MouseEvent, MouseEventKind,
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
        // 滚轮滚动 transcript + app-owned 文本选择（内部 scrollback 的输入源）。
        execute!(io::stdout(), crossterm::event::EnableMouseCapture)?;
        // Focus reporting (`?1004h`, TS `ENABLE_ALL_MOTION_MOUSE` includes it):
        // the model clears an in-progress mouse selection on focus loss.
        execute!(io::stdout(), EnableFocusChange)?;

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
                                // Forward bracketed paste as a single event so
                                // the editor can run TS `handlePaste` (normalize,
                                // filter, large-paste marker) instead of
                                // inserting char-by-char.
                                let _ = input_tx.send(InputEvent::Paste(text));
                            }
                            Some(Ok(Event::Mouse(m))) => {
                                // 滚轮 → 滚动事件（wheelScrollLines = 1，对齐 TS）；
                                // 其余鼠标事件（press/drag/release/move）转发给
                                // app-owned 文本选择（TS `handleMouseEvent`）。
                                match m.kind {
                                    MouseEventKind::ScrollUp => {
                                        let _ = input_tx.send(InputEvent::ScrollUp);
                                    }
                                    MouseEventKind::ScrollDown => {
                                        let _ = input_tx.send(InputEvent::ScrollDown);
                                    }
                                    _ => {
                                        let _ = input_tx.send(InputEvent::Mouse(m));
                                    }
                                }
                            }
                            // Focus loss clears an in-progress selection grip
                            // (TS `FOCUS_OUT` handling).
                            Some(Ok(Event::FocusLost)) => {
                                let _ = input_tx.send(InputEvent::FocusLost);
                            }
                            // Resize must be forwarded: otherwise the app never learns
                            // of the new size, and stale rows remain on the physical
                            // screen after shrink+grow (the diff can't see them).
                            Some(Ok(Event::Resize(cols, rows))) => {
                                let _ = input_tx.send(InputEvent::Resize(cols, rows));
                            }
                            Some(Ok(_)) => {}
                            Some(Err(_)) => break,
                            None => break,
                        }
                    }
                }
            }
            let _ = crossterm::terminal::disable_raw_mode();
            let _ = crossterm::execute!(
                std::io::stdout(),
                crossterm::event::DisableMouseCapture,
                DisableFocusChange
            );
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
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum InputEvent {
    Key(KeyEvent),
    ScrollUp,
    ScrollDown,
    /// Terminal resize (crossterm `Event::Resize`) — must reach the model so
    /// the transcript re-wraps and the screen fully repaints.
    Resize(u16, u16),
    /// Bracketed paste (crossterm `Event::Paste`) — routed through the
    /// editor's `handle_paste` (TS `handlePaste`).
    Paste(String),
    /// Non-wheel mouse events (press / drag / release / move) for the
    /// application-owned fullscreen text selection.
    Mouse(MouseEvent),
    /// Terminal focus loss (crossterm `Event::FocusLost`, `?1004h`) — cancels
    /// an in-progress selection, mirroring the TS `FOCUS_OUT` path.
    FocusLost,
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
