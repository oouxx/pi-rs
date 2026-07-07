//! Terminal setup with ratatui and crossterm.
//!
//! Handles raw mode, kitty keyboard protocol, and input event streaming.

use std::io::{self, stdout, Stdout};

use crossterm::event::{
    DisableBracketedPaste, EnableBracketedPaste, Event, EventStream, KeyEvent,
    KeyboardEnhancementFlags, PopKeyboardEnhancementFlags, PushKeyboardEnhancementFlags,
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
    kitty_active: bool,
}

impl Terminal {
    pub fn new() -> io::Result<Self> {
        let backend = CrosstermBackend::new(stdout());
        let inner = ratatui::Terminal::new(backend)?;
        let (cols, rows) = crossterm::terminal::size().unwrap_or((80, 24));
        Ok(Self { inner, columns: cols, rows, kitty_active: false })
    }

    pub fn columns(&self) -> u16 { self.columns }
    pub fn rows(&self) -> u16 { self.rows }
    pub fn kitty_protocol_active(&self) -> bool { self.kitty_active }

    /// Enter raw mode, enable kitty keyboard protocol, start input event loop.
    /// Returns (input_rx, shutdown_guard).
    pub fn start(&mut self) -> io::Result<(mpsc::UnboundedReceiver<KeyEvent>, ShutdownGuard)> {
        crossterm::terminal::enable_raw_mode()?;

        execute!(
            io::stdout(),
            EnableBracketedPaste,
            PushKeyboardEnhancementFlags(
                KeyboardEnhancementFlags::DISAMBIGUATE_ESCAPE_CODES
                    | KeyboardEnhancementFlags::REPORT_EVENT_TYPES
                    | KeyboardEnhancementFlags::REPORT_ALTERNATE_KEYS
            ),
        )?;

        self.kitty_active = true;

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
                                        let _ = input_tx.send(key_event);
                                    }
                                    continue;
                                }
                                let _ = input_tx.send(key_event);
                            }
                            Some(Ok(Event::Paste(text))) => {
                                // Forward pasted text (IME composition, clipboard) as char-by-char key events
                                for ch in text.chars() {
                                    let ev = KeyEvent::new(
                                        crossterm::event::KeyCode::Char(ch),
                                        crossterm::event::KeyModifiers::NONE,
                                    );
                                    let _ = input_tx.send(ev);
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
pub struct ShutdownGuard {
    sender: Option<tokio::sync::oneshot::Sender<()>>,
}

impl ShutdownGuard {
    pub fn shutdown(mut self) {
        if let Some(tx) = self.sender.take() {
            let _ = tx.send(());
        }
        let _ = crossterm::terminal::disable_raw_mode();
        let _ = execute!(
            io::stdout(),
            DisableBracketedPaste,
            PopKeyboardEnhancementFlags,
            crossterm::cursor::Show,
        );
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
