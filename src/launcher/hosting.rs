use super::app::App;
use ghost_reforged::hosting::{Activity, Control, Event, Worker};
use std::time::{Duration, Instant};

impl App {
    pub(super) fn start_hosting(&mut self) -> Result<(), String> {
        self.save()?;
        let p = self.form.read()?;
        let config = p.config(&self.data_directory)?;
        self.worker = Some(Worker::start(config)?);
        self.hosting_requested = true;
        self.last_reserved = None;
        self.lobby = None;
        self.hosting_status = "Connecting…".into();
        self.log("Auto Host started.");
        Ok(())
    }

    pub(super) fn reserve_number(&mut self, number: u32) -> Result<(), String> {
        let next = number.checked_add(1).ok_or("Lobby number exhausted.")?;
        let mut updated = self.profiles.clone();
        // An acknowledgement can be lost after the atomic save succeeded.
        if self.last_reserved == Some(number) && updated.items[updated.selected].next_number == next
        {
            return Ok(());
        }
        if updated.items[updated.selected].next_number != number {
            return Err("The lobby counter changed unexpectedly.".into());
        }
        updated.items[updated.selected].next_number = next;
        self.store
            .as_ref()
            .ok_or("Profile storage is unavailable.")?
            .save(&updated)
            .map_err(|e| e.to_string())?;
        self.profiles = updated;
        self.form.number = next.to_string();
        self.form.profile.next_number = next;
        self.last_reserved = Some(number);
        Ok(())
    }

    pub(super) fn poll_host(&mut self) {
        if self.settings_due.is_some_and(|due| Instant::now() >= due) {
            self.settings_due = None;
            self.apply_settings();
        }
        for _ in 0..64 {
            let Some(event) = self.worker.as_ref().and_then(Worker::receive) else {
                break;
            };
            match event {
                Event::ClientChanged(pid) => {
                    self.client_pid = pid;
                    self.preview
                        .target(pid.filter(|_| !self.form.profile.headless));
                }
                Event::VisualChanged => self.preview.request(),
                Event::Activity(event) => {
                    self.preview.request();
                    self.record_activity(event);
                }
                Event::Status(text) => {
                    self.preview.request();
                    self.hosting_status = text.clone();
                    self.log(&text);
                }
                Event::Lobby(lobby) => self.lobby = Some(lobby),
                Event::LobbyCleared => self.lobby = None,
                Event::Reserve(number) => {
                    let result = self.reserve_number(number);
                    if let Some(worker) = &self.worker {
                        worker.send(Control::Reserved {
                            number,
                            saved: result.is_ok(),
                        });
                    }
                    if result.is_err() {
                        self.notify(result, "");
                    }
                }
                Event::Finished(result) => {
                    self.client_pid = None;
                    self.hosting_requested = false;
                    self.preview.target(None);
                    self.worker = None;
                    self.lobby = None;
                    self.hosting_status = if result.is_ok() {
                        "Stopped"
                    } else {
                        "Stopped with an error"
                    }
                    .into();
                    if let Err(error) = &result {
                        self.log(error);
                    }
                    self.notify(result, "Auto Host stopped.");
                    break;
                }
            }
        }
    }

    pub(super) fn queue_settings(&mut self) {
        self.notice.clear();
        self.notice_error = false;
        if self.worker.is_some() {
            self.settings_due = Some(Instant::now() + Duration::from_millis(500));
        }
    }

    pub(super) fn record_activity(&mut self, event: Activity) {
        match event {
            Activity::LobbyCreated(name) => self.log_line(&format!("Lobby created · {name}"), true),
            Activity::GameStarted(name) => self.log_line(&format!("Game started · {name}"), true),
            Activity::Joined(name) => self.log(&format!("{name} joined")),
            Activity::Left(name) => self.log(&format!("{name} left")),
            Activity::Command(line) => self.log(&line),
        }
    }

    pub(super) fn apply_settings(&mut self) {
        if self.worker.is_none() {
            return;
        }
        let result = self.form.read().and_then(|profile| {
            let settings = profile.live_settings();
            settings.validate()?;
            self.save()?;
            if let Some(worker) = &self.worker {
                worker.send(Control::Settings(settings));
            }
            Ok(())
        });
        self.notify(result, "");
    }
}
