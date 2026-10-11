use super::{
    app::{App, Message},
    palette as p, updates,
};
use iced::{
    Color, Element, Fill, Task,
    widget::{column, container, opaque, row, space, stack, text},
};

impl App {
    pub(super) fn update_message(&mut self, message: &Message) -> Option<Task<Message>> {
        match message {
            Message::UpdateChecked(Ok(release)) => {
                self.updates.available = release.clone();
                Some(Task::none())
            }
            Message::UpdateChecked(Err(_)) => Some(Task::none()), // Offline starts remain silent.
            Message::DismissUpdate if !self.updates.downloading && self.updates.ready.is_none() => {
                self.updates.available = None;
                self.updates.error = None;
                Some(Task::none())
            }
            Message::InstallUpdate if !self.updates.downloading && self.updates.ready.is_none() => {
                let release = self.updates.available.clone()?;
                if let Err(error) = self.save() {
                    self.updates.error = Some(error);
                    return Some(Task::none());
                }
                self.updates.downloading = true;
                self.updates.error = None;
                Some(Task::perform(
                    updates::download(release),
                    Message::UpdateDownloaded,
                ))
            }
            Message::UpdateDownloaded(result) => {
                self.updates.downloading = false;
                match result {
                    Ok(download) => {
                        self.updates.ready = Some(download.clone());
                        self.updates.resume = self.hosting_requested;
                        if let Some(worker) = &self.worker {
                            worker.send(ghost_reforged::hosting::Control::Stop);
                            self.hosting_status = "Stopping for update…".into();
                            Some(Task::none())
                        } else {
                            Some(self.install_ready())
                        }
                    }
                    Err(error) => {
                        self.updates.error = Some(error.clone());
                        Some(Task::none())
                    }
                }
            }
            _ => None,
        }
    }

    pub(super) fn install_ready(&mut self) -> Task<Message> {
        let Some(download) = self.updates.ready.take() else {
            return Task::none();
        };
        match updates::install(&download, &self.data_directory, self.updates.resume) {
            Ok(()) => iced::window::latest().and_then(iced::window::close),
            Err(error) => {
                self.updates.error = Some(error);
                if self.updates.resume {
                    let result = self.start_hosting();
                    self.notify(result, "Hosting resumed.");
                }
                Task::none()
            }
        }
    }
}

pub fn modal<'a>(app: &'a App, base: Element<'a, Message>) -> Element<'a, Message> {
    let Some(release) = &app.updates.available else {
        return base;
    };
    let busy = app.updates.downloading || app.updates.ready.is_some();
    let mut content = column![
        text("Update available").size(23).color(p::GOLD),
        text(format!("GHost Reforged {}{}", release.version, if release.preview { " (preview)" } else { "" })).size(17),
        text("Install the new version from GitHub? Your profiles are kept. Hosting resumes after the restart.").size(15),
    ].spacing(16);
    if let Some(error) = &app.updates.error {
        content = content.push(text(error).color(p::ERROR).size(13));
    }
    content = content.push(
        row![
            space().width(Fill),
            super::widgets::action("Later", Message::DismissUpdate, !busy),
            super::widgets::action(
                if app.updates.downloading {
                    "Downloading…"
                } else if busy {
                    "Restarting…"
                } else {
                    "Update now"
                },
                Message::InstallUpdate,
                !busy
            ),
        ]
        .spacing(12),
    );
    stack![
        base,
        opaque(
            container(space())
                .width(Fill)
                .height(Fill)
                .style(|_| container::Style {
                    background: Some(Color::from_rgba(0.0, 0.0, 0.0, 0.75).into()),
                    ..Default::default()
                })
        ),
        container(opaque(
            container(content).padding(26).width(480).style(p::panel)
        ))
        .center_x(Fill)
        .center_y(Fill),
        container(row![space().width(Fill), super::chrome::controls()]).padding(iced::Padding {
            right: 20.0,
            ..Default::default()
        }),
    ]
    .into()
}
