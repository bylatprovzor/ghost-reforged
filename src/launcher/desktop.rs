use super::{
    app::{App, Message},
    tray::Event,
};
use iced::{Task, window};

impl App {
    pub(super) fn hide_to_tray(&mut self, id: window::Id) -> Task<Message> {
        if !self.tray_available {
            // Keep the existing close path if Windows cannot provide a tray icon.
            return Task::done(Message::QuitRequested(id));
        }
        self.hidden = true;
        self.focused = false;
        window::set_mode(id, window::Mode::Hidden)
    }

    pub(super) fn tray_event(&mut self, event: Event) -> Task<Message> {
        match event {
            Event::Available(available) => {
                self.tray_available = available;
                if !available && self.hidden {
                    return self.restore_window();
                }
                Task::none()
            }
            Event::Open => self.restore_window(),
            Event::Quit => window::latest().and_then(|id| Task::done(Message::QuitRequested(id))),
        }
    }

    fn restore_window(&mut self) -> Task<Message> {
        self.hidden = false;
        self.preview.request();
        window::latest().and_then(|id| {
            window::set_mode(id, window::Mode::Windowed)
                .chain(window::minimize(id, false))
                .chain(window::gain_focus(id))
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn close_hides_without_quitting_and_tray_restores_even_with_a_modal() {
        let dir = tempfile::tempdir().unwrap();
        let mut app = App::new(dir.path(), None);
        app.tray_available = true;
        app.motion.modal = super::super::motion::Tween::settled(1.0);
        let _ = app.update(Message::CloseRequested(window::Id::unique()));
        assert!(app.hidden);
        assert!(!app.focused);
        assert!(app.closing.is_none());
        let _ = app.update(Message::Tray(Event::Open));
        assert!(!app.hidden);
        assert!(app.closing.is_none());
    }

    #[test]
    fn unavailable_tray_never_strands_a_hidden_window() {
        let dir = tempfile::tempdir().unwrap();
        let mut app = App::new(dir.path(), None);
        app.tray_available = true;
        let _ = app.update(Message::CloseRequested(window::Id::unique()));
        let _ = app.update(Message::Tray(Event::Available(false)));
        assert!(!app.hidden);
        assert!(!app.tray_available);
    }
}
