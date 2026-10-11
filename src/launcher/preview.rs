use ghost_reforged::window_capture::{self, Frame};
use iced::widget::image;
use std::{
    sync::mpsc::{self, Receiver},
    thread,
    time::{Duration, Instant},
};

#[derive(Default)]
pub struct Preview {
    pub expanded: bool,
    pub image: Option<image::Handle>,
    pub error: Option<String>,
    pub updated: Option<Instant>,
    target: Option<u32>,
    epoch: u64,
    pending: Option<Instant>,
    flight: Option<Receiver<(u64, Result<Frame, String>)>>,
    next_allowed: Option<Instant>,
    retries: u8,
}

impl Preview {
    pub fn expand(&mut self) {
        if self.image.is_some() && !self.expanded {
            self.expanded = true;
            self.request();
        }
    }

    pub fn needs_poll(&self) -> bool {
        self.pending.is_some() || self.flight.is_some() || (self.expanded && self.target.is_some())
    }

    pub fn target(&mut self, pid: Option<u32>) {
        if self.target != pid {
            self.target = pid;
            self.epoch = self.epoch.wrapping_add(1);
            self.image = None;
            self.updated = None;
            self.error = None;
            self.pending = None;
            self.request();
        }
    }

    pub fn request(&mut self) {
        if self.target.is_some() {
            self.pending
                .get_or_insert_with(|| Instant::now() + Duration::from_millis(200));
            self.retries = 0;
        }
    }

    pub fn poll(&mut self) {
        if let Some(receiver) = &self.flight {
            match receiver.try_recv() {
                Ok((epoch, result)) => {
                    self.flight = None;
                    // Accept the latest completed frame of this client, even if
                    // a newer state is queued. Busy lobbies must not starve the preview.
                    if self.target.is_some() && epoch == self.epoch {
                        match result {
                            Ok(frame) => {
                                self.image = Some(image::Handle::from_rgba(
                                    frame.width,
                                    frame.height,
                                    frame.rgba,
                                ));
                                self.updated = Some(Instant::now());
                                self.error = None;
                            }
                            Err(error) => {
                                self.error = Some(error);
                                if self.retries < 2 {
                                    self.retries += 1;
                                    self.pending = Some(Instant::now() + Duration::from_secs(1));
                                }
                            }
                        }
                    }
                }
                Err(mpsc::TryRecvError::Disconnected) => {
                    self.flight = None;
                    self.error = Some("Preview capture stopped.".into());
                }
                Err(mpsc::TryRecvError::Empty) => {}
            }
        }
        let Some(pid) = self.target else {
            return;
        };
        let now = Instant::now();
        if self.expanded
            && self.pending.is_none()
            && self.flight.is_none()
            && self
                .updated
                .is_none_or(|updated| now.duration_since(updated) >= Duration::from_secs(2))
        {
            self.pending = Some(now);
        }
        if self.flight.is_some()
            || self.pending.is_none_or(|due| now < due)
            || self.next_allowed.is_some_and(|due| now < due)
        {
            return;
        }
        let (tx, rx) = mpsc::channel();
        let epoch = self.epoch;
        let expanded = self.expanded;
        match thread::Builder::new()
            .name("ghost-preview".into())
            .spawn(move || {
                let _ = tx.send((epoch, window_capture::capture_preview(pid, expanded)));
            }) {
            Ok(_) => self.flight = Some(rx),
            Err(_) => self.error = Some("Preview capture could not start.".into()),
        }
        self.pending = None;
        self.next_allowed = Some(now + Duration::from_secs(1));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_frame_from_an_old_client_is_not_displayed_after_restart() {
        let mut preview = Preview::default();
        preview.target(Some(10));
        let revision = preview.epoch;
        let (tx, rx) = mpsc::channel();
        preview.flight = Some(rx);
        preview.target(None);
        tx.send((
            revision,
            Ok(Frame {
                width: 1,
                height: 1,
                rgba: vec![255; 4],
            }),
        ))
        .unwrap();
        preview.poll();
        assert!(preview.image.is_none());
        assert!(preview.pending.is_none());
    }

    #[test]
    fn changes_during_capture_schedule_one_fresh_frame() {
        let mut preview = Preview::default();
        preview.target(Some(10));
        let revision = preview.epoch;
        let (tx, rx) = mpsc::channel();
        preview.flight = Some(rx);
        preview.request();
        preview.request();
        tx.send((
            revision,
            Ok(Frame {
                width: 1,
                height: 1,
                rgba: vec![255; 4],
            }),
        ))
        .unwrap();
        preview.poll();
        assert!(preview.image.is_some());
        assert!(preview.pending.is_some());
    }
}
