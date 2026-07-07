use std::io::{self, stdout, Stdout};

use crossterm::event::{
    DisableBracketedPaste, EnableBracketedPaste, Event, EventStream, KeyEvent,
    KeyboardEnhancementFlags, PopKeyboardEnhancementFlags, PushKeyboardEnhancementFlags,
};
use crossterm::execute;
use futures::StreamExt;
use ratatui::backend::CrosstermBackend;
use ratatui::Terminal as RatatuiTerminal;
use tokio::sync::mpsc;

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

        let (input_tx, input_rx) = mpsc::unbounded_channel::<KeyEvent>();
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
                                if key_event.kind == crossterm::event::KeyEventKind::Release {
                                    continue;
                                }
                                let _ = input_tx.send(key_event);
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

        Ok((
            input_rx,
            ShutdownGuard {
                sender: Some(shutdown_tx),
            },
        ))
    }

    pub fn ratatui_terminal(&mut self) -> &mut RatatuiTerminal<CrosstermBackend<Stdout>> {
        &mut self.inner
    }

    pub fn get_frame(&mut self) -> ratatui::Frame {
        self.inner.get_frame()
    }

    pub fn clear(&mut self) -> io::Result<()> {
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
