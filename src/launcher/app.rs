use super::{
    preview::Preview,
    process::{self, GameState},
    profile::{GameSpeed, Profile, Profiles},
    storage::Store,
};
use ghost_reforged::hosting::{Control, Snapshot, Worker};
use iced::{Subscription, Task};
use std::{
    collections::VecDeque,
    path::{Path, PathBuf},
    time::{Duration, Instant},
};

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Page {
    Hosting,
    Profiles,
    Activity,
    Options,
}

#[derive(Debug, Clone, Copy)]
pub enum Field {
    Title,
    Pattern,
    Number,
    Players,
    RehostDelay,
    EmptyRefresh,
    Welcome,
    LobbyPassword,
}

#[derive(Debug, Clone)]
pub enum Message {
    UpdateChecked(Result<Option<super::updates::Release>, String>),
    InstallUpdate,
    DismissUpdate,
    UpdateDownloaded(Result<super::updates::Download, String>),
    Tray(super::tray::Event),
    QuitRequested(iced::window::Id),
    Window(WindowAction),
    PollBackdrop,
    WindowFocus(bool),
    Animate(Instant),
    Menu(super::motion::Menu),
    DismissMenus,
    Page(Page),
    Edit(Field, String),
    Command(ghost_reforged::commands::ChatCommand, bool),
    Welcome(bool),
    Rehost(bool),
    Headless(bool),
    Private(bool),
    Speed(GameSpeed),
    RequireFull(bool),
    MapName,
    OpenPreview,
    ClosePreview,
    Host,
    Stop,
    Start,
    Slot(ghost_reforged::hosting::SlotRequest),
    Kick(ghost_reforged::hosting::KickRequest),
    PollHost,
    CloseRequested(iced::window::Id),
    Browse,
    Picked(Result<Option<PathBuf>, String>),
    Save,
    SelectProfile(usize),
    NewProfile,
    PollProcess,
    ProcessObserved(process::Observation),
    ClearLog,
}

#[derive(Debug, Clone, Copy)]
pub enum WindowAction {
    Drag,
    Minimize,
    Maximize,
    Close,
}

pub struct Form {
    pub profile: Profile,
    pub number: String,
    pub players: String,
    pub rehost_delay: String,
    pub empty_refresh: String,
}

impl Form {
    pub fn from(profile: Profile) -> Self {
        Self {
            number: profile.next_number.to_string(),
            players: profile.players.to_string(),
            rehost_delay: profile.rehost_delay_seconds.to_string(),
            empty_refresh: profile.empty_rehost_seconds.to_string(),
            profile,
        }
    }

    pub fn read(&self) -> Result<Profile, String> {
        let mut p = self.profile.clone();
        p.next_number = self
            .number
            .parse()
            .map_err(|_| "Next number must be a positive integer.")?;
        if !p.require_full {
            p.players = self
                .players
                .parse()
                .map_err(|_| "Minimum players must be a whole number.")?;
        }
        p.rehost_delay_seconds = self
            .rehost_delay
            .parse()
            .map_err(|_| "Rehost delay must be a whole number.")?;
        p.empty_rehost_seconds = self
            .empty_refresh
            .parse()
            .map_err(|_| "Empty lobby refresh must be a whole number.")?;
        p.auto_start = true;
        p.validate()?;
        Ok(p)
    }
}

pub struct LogEntry {
    pub time: String,
    pub message: String,
    pub highlight: bool,
}

pub struct App {
    pub updates: super::updates::State,
    pub(super) data_directory: PathBuf,
    pub(super) last_reserved: Option<u32>,
    pub(super) hosting_requested: bool,
    pub(super) audio: ghost_reforged::background::Silence,
    pub hidden: bool,
    pub tray_available: bool,
    pub backdrop: super::backdrop::Backdrop,
    pub focused: bool,
    pub motion: super::motion::Motion,
    pub page: Page,
    pub form: Form,
    pub profiles: Profiles,
    pub notice: String,
    pub notice_error: bool,
    pub logs: VecDeque<LogEntry>,
    pub preview: Preview,
    pub(super) client_pid: Option<u32>,
    pub picking: bool,
    pub store: Option<Store>,
    pub game: GameState,
    pub worker: Option<Worker>,
    pub lobby: Option<Snapshot>,
    pub hosting_status: String,
    pub(super) closing: Option<iced::window::Id>,
    probing: bool,
    pub(super) settings_due: Option<Instant>,
    pub(super) started: Instant,
}

impl App {
    pub fn new(directory: &Path, map: Option<PathBuf>) -> Self {
        let (store, profiles, notice, notice_error) = match Store::open(directory) {
            Ok((store, profiles)) => (Some(store), profiles, String::new(), false),
            Err(error) => (None, Profiles::default(), error.to_string(), true),
        };
        let mut form = Form::from(profiles.items[profiles.selected].clone());
        if let Some(map) = map {
            if form.profile.automatic_name {
                form.profile.lobby_pattern = super::map_name::pattern(&map);
            }
            form.profile.map = Some(map);
        }
        let mut app = Self {
            updates: Default::default(),
            data_directory: directory.to_owned(),
            last_reserved: None,
            hosting_requested: false,
            audio: ghost_reforged::background::Silence::default(),
            hidden: false,
            tray_available: false,
            backdrop: super::backdrop::Backdrop::new(),
            focused: true,
            motion: super::motion::Motion::default(),
            page: Page::Hosting,
            form,
            profiles,
            notice,
            notice_error,
            logs: VecDeque::new(),
            preview: Preview::default(),
            client_pid: None,
            picking: false,
            store,
            game: GameState::Checking,
            worker: None,
            lobby: None,
            hosting_status: "Idle".into(),
            closing: None,
            probing: false,
            settings_due: None,
            started: Instant::now(),
        };
        app.log("Launcher opened.");
        app
    }

    pub(super) fn log(&mut self, line: &str) {
        self.log_line(line, false);
    }

    pub(super) fn log_line(&mut self, line: &str, highlight: bool) {
        let seconds = self.started.elapsed().as_secs();
        super::journal::append(&self.data_directory, seconds, line);
        self.logs.push_front(LogEntry {
            time: format!("{:02}:{:02}", seconds / 60, seconds % 60),
            message: line.into(),
            highlight,
        });
        self.logs.truncate(200);
    }

    pub(super) fn notify(&mut self, result: Result<(), String>, success: &str) {
        match result {
            Ok(()) => {
                self.notice = success.into();
                self.notice_error = false;
            }
            Err(error) => {
                self.notice = error;
                self.notice_error = true;
            }
        }
    }

    pub(super) fn save(&mut self) -> Result<(), String> {
        let profile = self.form.read()?;
        let mut updated = self.profiles.clone();
        updated.items[updated.selected] = profile;
        self.store
            .as_ref()
            .ok_or("Profile storage is unavailable.")?
            .save(&updated)
            .map_err(|e| e.to_string())?;
        self.profiles = updated;
        Ok(())
    }

    pub fn update(&mut self, message: Message) -> Task<Message> {
        if let Some(task) = self.update_message(&message) {
            return task;
        }
        if let Message::Tray(event) = message {
            return self.tray_event(event);
        }
        if let Message::CloseRequested(id) = message {
            return self.hide_to_tray(id);
        }
        if (self.motion.modal_visible() || self.updates.available.is_some())
            && !matches!(
                message,
                Message::ClosePreview
                    | Message::Window(_)
                    | Message::WindowFocus(_)
                    | Message::PollBackdrop
                    | Message::Picked(_)
                    | Message::Animate(_)
                    | Message::PollHost
                    | Message::PollProcess
                    | Message::ProcessObserved(_)
                    | Message::CloseRequested(_)
                    | Message::QuitRequested(_)
                    | Message::Stop
            )
        {
            return Task::none();
        }
        if self.picking && matches!(message, Message::SelectProfile(_) | Message::NewProfile) {
            return Task::none();
        }
        if self.worker.is_some()
            && matches!(
                message,
                Message::Edit(Field::Pattern | Field::Number | Field::LobbyPassword, _)
                    | Message::Private(_)
                    | Message::Speed(_)
                    | Message::MapName
                    | Message::Browse
                    | Message::SelectProfile(_)
                    | Message::NewProfile
            )
        {
            return Task::none();
        }
        match message {
            Message::Window(action) => {
                return iced::window::latest().and_then(move |id| match action {
                    WindowAction::Drag => iced::window::drag(id),
                    WindowAction::Minimize => iced::window::minimize(id, true),
                    WindowAction::Maximize => iced::window::toggle_maximize(id),
                    WindowAction::Close => Task::done(Message::CloseRequested(id)),
                });
            }
            Message::PollBackdrop => self.backdrop.poll(),
            Message::WindowFocus(focused) => self.focused = focused && !self.hidden,
            Message::Animate(now) => {
                if let Some(page) = self.motion.tick(now) {
                    self.page = page;
                }
            }
            Message::Menu(menu) => self.motion.toggle_menu(menu),
            Message::DismissMenus => self.motion.close_menus(),
            Message::OpenPreview if !self.form.profile.headless => {
                self.preview.expand();
                if self.preview.expanded {
                    self.motion.modal.target(1.0, Instant::now());
                }
            }
            Message::ClosePreview => {
                self.preview.expanded = false;
                self.motion.modal.target(0.0, Instant::now());
            }
            Message::QuitRequested(id) => {
                self.hosting_requested = false;
                self.updates.resume = false;
                if let Some(worker) = &self.worker {
                    self.closing = Some(id);
                    worker.send(Control::Stop);
                    self.hosting_status = "Stopping…".into();
                } else {
                    return iced::window::close(id);
                }
            }
            Message::Host if self.worker.is_none() && !self.picking && self.closing.is_none() => {
                let result = self.start_hosting();
                self.notify(result, "");
            }
            Message::Stop => {
                self.hosting_requested = false;
                self.updates.resume = false;
                if let Some(worker) = &self.worker {
                    worker.send(Control::Stop);
                }
                self.hosting_status = "Stopping…".into();
            }
            Message::Start => {
                if let Some(worker) = &self.worker {
                    worker.send(Control::Start);
                }
            }
            Message::Slot(request) => {
                self.motion.close_menus();
                if let Some(worker) = &self.worker {
                    worker.send(Control::EditSlot(request));
                }
            }
            Message::Kick(target) => {
                if let Some(worker) = &self.worker {
                    worker.send(Control::Kick(target));
                }
            }
            Message::PollHost => {
                self.poll_host();
                if !self.hidden && !self.form.profile.headless {
                    self.preview.poll();
                }
                if self.worker.is_none() && self.updates.ready.is_some() {
                    return self.install_ready();
                }
                if self.worker.is_none()
                    && let Some(id) = self.closing.take()
                {
                    return iced::window::close(id);
                }
            }
            Message::Page(page) => self.motion.navigate(self.page, page),
            Message::Edit(field, value) => {
                match field {
                    Field::Title => self.form.profile.title = value,
                    Field::Pattern => {
                        self.form.profile.lobby_pattern = value;
                        self.form.profile.automatic_name = false;
                    }
                    Field::Number => self.form.number = value,
                    Field::Players => self.form.players = value,
                    Field::RehostDelay => self.form.rehost_delay = value,
                    Field::EmptyRefresh => self.form.empty_refresh = value,
                    Field::Welcome => self.form.profile.welcome.message = value,
                    Field::LobbyPassword => self.form.profile.lobby_password = value,
                }
                self.queue_settings();
            }
            Message::Command(command, enabled) => {
                self.form.profile.commands.set(command, enabled);
                self.apply_settings();
            }
            Message::Welcome(enabled) => {
                self.form.profile.welcome.enabled = enabled;
                self.apply_settings();
            }
            Message::Rehost(value) => {
                self.form.profile.rehost = value;
                self.apply_settings();
            }
            Message::Private(value) => self.form.profile.private_game = value,
            Message::Headless(value) => {
                self.form.profile.headless = value;
                self.preview = Preview::default();
                if !value {
                    self.preview.target(self.client_pid);
                }
                self.motion.modal.target(0.0, Instant::now());
                self.apply_settings();
            }
            Message::Speed(value) => {
                self.form.profile.speed = value;
                self.motion.close_menus();
            }
            Message::RequireFull(value) => {
                self.motion.close_menus();
                self.form.profile.require_full = value;
                self.apply_settings();
            }
            Message::MapName => {
                if let Some(map) = &self.form.profile.map {
                    self.form.profile.lobby_pattern = super::map_name::pattern(map);
                    self.form.profile.automatic_name = true;
                }
            }
            Message::Browse if !self.picking => {
                self.picking = true;
                return Task::perform(super::maps::pick(), Message::Picked);
            }
            Message::Picked(result) => {
                self.picking = false;
                match result {
                    Ok(Some(path)) => {
                        if self.form.profile.automatic_name {
                            self.form.profile.lobby_pattern = super::map_name::pattern(&path);
                        }
                        self.form.profile.map = Some(path);
                        self.notify(Ok(()), "Map selected.");
                    }
                    Err(error) => self.notify(Err(error), ""),
                    Ok(None) => {}
                }
            }
            Message::Save => {
                let result = self.save();
                if result.is_ok() {
                    self.log("Profile saved.");
                }
                self.notify(result, "Saved.");
            }
            Message::SelectProfile(index) if index < self.profiles.items.len() => {
                self.motion.close_menus();
                let result = self.save().and_then(|_| {
                    let mut updated = self.profiles.clone();
                    updated.selected = index;
                    self.store
                        .as_ref()
                        .ok_or("Profile storage is unavailable.")?
                        .save(&updated)
                        .map_err(|e| e.to_string())?;
                    self.profiles = updated;
                    self.form = Form::from(self.profiles.items[index].clone());
                    Ok(())
                });
                self.notify(result, "Profile loaded.");
            }
            Message::NewProfile if self.profiles.items.len() < 30 => {
                let result = self.save().and_then(|_| {
                    let mut updated = self.profiles.clone();
                    updated.items.push(Profile {
                        title: format!("Profile {}", updated.items.len() + 1),
                        ..Profile::default()
                    });
                    updated.selected = updated.items.len() - 1;
                    self.store
                        .as_ref()
                        .ok_or("Profile storage is unavailable.")?
                        .save(&updated)
                        .map_err(|e| e.to_string())?;
                    self.profiles = updated;
                    self.form = Form::from(self.profiles.items[self.profiles.selected].clone());
                    Ok(())
                });
                self.notify(result, "Profile created.");
            }
            Message::PollProcess if !self.probing => {
                self.probing = true;
                return Task::perform(async { process::observe() }, Message::ProcessObserved);
            }
            Message::ProcessObserved(game) => {
                if game.pid.is_some() || game.state == GameState::Stopped {
                    self.audio.target(game.pid);
                }
                if let Some(error) = self.audio.error() {
                    self.log(&error);
                    self.notify(Err(error), "");
                }
                self.probing = false;
                if self.worker.is_none() {
                    self.preview
                        .target(game.pid.filter(|_| !self.form.profile.headless));
                    if self.game != game.state {
                        self.preview.request();
                    }
                }
                self.game = game.state;
            }
            Message::ClearLog => self.logs.clear(),
            _ => {}
        }
        Task::none()
    }

    pub fn subscription(&self) -> Subscription<Message> {
        Subscription::batch([
            super::tray::subscription().map(Message::Tray),
            if let Some(interval) = self
                .backdrop
                .interval(self.focused && !self.hidden && !self.motion.modal_visible())
            {
                iced::time::every(interval).map(|_| Message::PollBackdrop)
            } else {
                Subscription::none()
            },
            iced::event::listen_with(|event, _, _| match event {
                iced::Event::Window(iced::window::Event::Focused) => {
                    Some(Message::WindowFocus(true))
                }
                iced::Event::Window(iced::window::Event::Unfocused) => {
                    Some(Message::WindowFocus(false))
                }
                _ => None,
            }),
            if !self.hidden && self.motion.active() {
                iced::time::every(Duration::from_millis(16)).map(Message::Animate)
            } else {
                Subscription::none()
            },
            iced::time::every(Duration::from_secs(3)).map(|_| Message::PollProcess),
            if self.worker.is_some()
                || (!self.hidden && self.preview.needs_poll())
                || self.settings_due.is_some()
            {
                iced::time::every(Duration::from_millis(250)).map(|_| Message::PollHost)
            } else {
                Subscription::none()
            },
            if self.preview.expanded {
                iced::event::listen_with(|event, _, _| match event {
                    iced::Event::Keyboard(iced::keyboard::Event::KeyPressed {
                        key: iced::keyboard::Key::Named(iced::keyboard::key::Named::Escape),
                        ..
                    }) => Some(Message::ClosePreview),
                    _ => None,
                })
            } else {
                Subscription::none()
            },
            iced::window::close_requests().map(Message::CloseRequested),
        ])
    }
}
