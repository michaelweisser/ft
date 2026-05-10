use std::{
    sync::mpsc::{self, Receiver, Sender},
    thread,
    time::Duration,
};

use anyhow::Result;
use crossterm::event::{self, Event as CtEvent, KeyEvent, KeyEventKind, MouseEvent};

/// Events flowing through the TUI loop. `Tick` fires once per second so the
/// sidebar clock can update without forcing a full redraw on every keystroke.
/// `Mouse` and `Resize` payloads are routed but not consumed in session 1;
/// later sessions will drive layout caches off `Resize`.
#[derive(Debug, Clone)]
pub enum Event {
    Key(KeyEvent),
    #[allow(dead_code)] // routed but not consumed yet; reserved for future sessions
    Mouse(MouseEvent),
    #[allow(dead_code)] // routed but not consumed yet; reserved for future sessions
    Resize(u16, u16),
    Tick,
}

/// Channel-backed event source: a background thread polls crossterm and sends
/// `Event::Tick` on a 1s cadence; the main loop drains via `next()`.
pub struct EventStream {
    rx: Receiver<Event>,
    _tx: Sender<Event>,
}

impl EventStream {
    pub fn new(tick_rate: Duration) -> Self {
        let (tx, rx) = mpsc::channel();
        let crossterm_tx = tx.clone();
        thread::spawn(move || crossterm_loop(crossterm_tx, tick_rate));
        Self { rx, _tx: tx }
    }

    /// Block until the next event arrives. Errors only on channel teardown.
    pub fn next(&self) -> Result<Event> {
        self.rx.recv().map_err(Into::into)
    }

    /// Discard every event received during `window`. Used after returning
    /// from `$EDITOR` so terminal-response escape sequences (e.g. DCS
    /// `XTGETTCAP` replies) and any keys typed during the editor session
    /// don't leak into the next read — without this, the `/` byte of a
    /// DCS reply puts the search view into edit mode and the rest of the
    /// reply gets typed into the query buffer.
    pub fn drain(&self, window: Duration) {
        let deadline = std::time::Instant::now() + window;
        loop {
            // Empty the channel of everything currently queued.
            while self.rx.try_recv().is_ok() {}
            if std::time::Instant::now() >= deadline {
                return;
            }
            // Yield so the background crossterm thread has a chance to
            // ingest more bytes from stdin and push them through.
            std::thread::sleep(Duration::from_millis(20));
        }
    }
}

fn crossterm_loop(tx: Sender<Event>, tick_rate: Duration) {
    let mut last_tick = std::time::Instant::now();
    loop {
        let timeout = tick_rate
            .checked_sub(last_tick.elapsed())
            .unwrap_or(Duration::ZERO);
        let has_event = event::poll(timeout).unwrap_or(false);
        if has_event {
            match event::read() {
                Ok(CtEvent::Key(k)) if k.kind == KeyEventKind::Press => {
                    if tx.send(Event::Key(k)).is_err() {
                        return;
                    }
                }
                Ok(CtEvent::Mouse(m)) => {
                    if tx.send(Event::Mouse(m)).is_err() {
                        return;
                    }
                }
                Ok(CtEvent::Resize(w, h)) => {
                    if tx.send(Event::Resize(w, h)).is_err() {
                        return;
                    }
                }
                Ok(_) => {}
                Err(_) => return,
            }
        }
        if last_tick.elapsed() >= tick_rate {
            if tx.send(Event::Tick).is_err() {
                return;
            }
            last_tick = std::time::Instant::now();
        }
    }
}
