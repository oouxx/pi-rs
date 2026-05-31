use std::io::{self, stdout, Stdout};

use crossterm::event::{
    DisableBracketedPaste, EnableBracketedPaste, Event, EventStream,
    KeyboardEnhancementFlags, PopKeyboardEnhancementFlags, PushKeyboardEnhancementFlags,
};
use crossterm::execute;
use futures::StreamExt;
use ratatui::backend::CrosstermBackend;
use ratatui::Terminal as RatatuiTerminal;
use tokio::sync::mpsc;

use crate::keys::set_kitty_protocol_active;

/// Terminal abstraction wrapping ratatui's CrosstermBackend.
/// Provides raw mode, kitty protocol negotiation, input events, and output.
pub struct Terminal {
    inner: RatatuiTerminal<CrosstermBackend<Stdout>>,
    columns: u16,
    rows: u16,
    kitty_active: bool,
}

impl Terminal {
    pub fn new() -> io::Result<Self> {
        let backend = CrosstermBackend::new(stdout());
        let inner = RatatuiTerminal::new(backend)?;
        let (cols, rows) = crossterm::terminal::size().unwrap_or((80, 24));
        Ok(Self {
            inner,
            columns: cols,
            rows,
            kitty_active: false,
        })
    }

    pub fn columns(&self) -> u16 {
        self.columns
    }

    pub fn rows(&self) -> u16 {
        self.rows
    }

    pub fn kitty_protocol_active(&self) -> bool {
        self.kitty_active
    }

    /// Enter raw mode, enable kitty keyboard protocol, start input event loop.
    /// Returns a channel receiver for input events and a shutdown sender.
    pub fn start(&mut self) -> io::Result<(mpsc::UnboundedReceiver<String>, ShutdownGuard)> {
        crossterm::terminal::enable_raw_mode()?;

        // Enable bracketed paste and kitty keyboard enhancement
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
        set_kitty_protocol_active(true);

        let (input_tx, input_rx) = mpsc::unbounded_channel::<String>();
        let (shutdown_tx, mut shutdown_rx) = tokio::sync::oneshot::channel::<()>();

        let mut event_stream = EventStream::new();
        tokio::spawn(async move {
            loop {
                tokio::select! {
                    _ = &mut shutdown_rx => {
                        break;
                    }
                    event = event_stream.next() => {
                        match event {
                            Some(Ok(Event::Key(key_event))) => {
                                let data = crossterm_key_to_string(&key_event);
                                if !data.is_empty() {
                                    let _ = input_tx.send(data);
                                }
                            }
                            Some(Ok(Event::Resize(cols, _rows))) => {
                                // Resize will be picked up on next render
                                let _ = cols;
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

    /// Access the underlying ratatui terminal for rendering.
    pub fn ratatui_terminal(&mut self) -> &mut RatatuiTerminal<CrosstermBackend<Stdout>> {
        &mut self.inner
    }

    /// Get a frame for rendering.
    pub fn get_frame(&mut self) -> ratatui::Frame {
        self.inner.get_frame()
    }

    /// Clear the terminal and reset cursor.
    pub fn clear(&mut self) -> io::Result<()> {
        execute!(io::stdout(), crossterm::terminal::Clear(crossterm::terminal::ClearType::All))?;
        Ok(())
    }

    /// Update the internally tracked terminal size.
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
        set_kitty_protocol_active(false);
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

/// Convert a crossterm KeyEvent to its raw string representation for input parsing.
fn crossterm_key_to_string(key: &crossterm::event::KeyEvent) -> String {
    use crossterm::event::{KeyCode::*, KeyModifiers};

    match key.code {
        Char(c) => {
            if key.modifiers.contains(KeyModifiers::CONTROL) && c.is_ascii_alphabetic() {
                // Ctrl+A..Z → \x01..\x1a
                (c.to_ascii_lowercase() as u8 - b'a' + 1).to_string()
            } else if key.modifiers.contains(KeyModifiers::ALT) {
                format!("\x1b{}", c)
            } else {
                c.to_string()
            }
        }
        Enter => "\r".to_string(),
        Tab => "\t".to_string(),
        Backspace => "\x7f".to_string(),
        Esc => "\x1b".to_string(),
        Delete => "\x1b[3~".to_string(),
        Up => "\x1b[A".to_string(),
        Down => "\x1b[B".to_string(),
        Left => "\x1b[D".to_string(),
        Right => "\x1b[C".to_string(),
        Home => "\x1b[H".to_string(),
        End => "\x1b[F".to_string(),
        PageUp => "\x1b[5~".to_string(),
        PageDown => "\x1b[6~".to_string(),
        Insert => "\x1b[2~".to_string(),
        F(n) => match n {
            1 => "\x1bOP".to_string(),
            2 => "\x1bOQ".to_string(),
            3 => "\x1bOR".to_string(),
            4 => "\x1bOS".to_string(),
            n if n <= 12 => format!("\x1b[{}~", 10 + n),
            _ => String::new(),
        },
        _ => String::new(),
    }
}
