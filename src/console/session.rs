use super::{Action, Args, profile_index};
use crate::{
    hosting::{Activity, Control, Event, Snapshot, Worker},
    profile::{Profile, Profiles},
    storage::Store,
};
use serde_json::{Value, json};
use std::{
    collections::VecDeque,
    path::{Path, PathBuf},
    time::Instant,
};

pub(super) struct Session {
    directory: PathBuf,
    store: Store,
    pub profiles: Profiles,
    worker: Option<Worker>,
    lobby: Option<Snapshot>,
    pid: Option<u32>,
    status: String,
    events: VecDeque<Value>,
    began: Instant,
    reserved: Option<u32>,
    pub quitting: bool,
    pub last_result: Result<(), String>,
}

impl Session {
    pub fn open(directory: &Path) -> Result<Self, String> {
        let (store, profiles) = Store::open(directory).map_err(|e| e.to_string())?;
        Ok(Self {
            directory: directory.to_owned(),
            store,
            profiles,
            worker: None,
            lobby: None,
            pid: None,
            status: "Idle".into(),
            events: VecDeque::new(),
            began: Instant::now(),
            reserved: None,
            quitting: false,
            last_result: Ok(()),
        })
    }
    pub fn selected(&self) -> &Profile {
        &self.profiles.items[self.profiles.selected]
    }
    pub fn running(&self) -> bool {
        self.worker.is_some()
    }
    fn commit(&mut self, profiles: Profiles) -> Result<(), String> {
        self.store.save(&profiles).map_err(|e| e.to_string())?;
        self.profiles = profiles;
        Ok(())
    }
    pub fn configure(&mut self, args: &Args) -> Result<(), String> {
        let mut updated = self.profiles.clone();
        if let Some(profile) = &args.profile {
            updated.selected = profile_index(&updated, profile)?;
        }
        let profile = &mut updated.items[updated.selected];
        if let Some(map) = &args.map {
            profile.map = Some(crate::maps::validate(map)?);
            if profile.automatic_name {
                profile.lobby_pattern = crate::map_name::pattern(map);
            }
        }
        if args.headless || args.normal {
            profile.headless = args.headless;
        }
        self.commit(updated)
    }
    fn stopped(&self) -> Result<(), String> {
        if self.running() {
            Err("Stop hosting before changing this setting or selecting another profile.".into())
        } else {
            Ok(())
        }
    }
    fn record(&mut self, kind: &str, text: &str) {
        crate::journal::append(&self.directory, self.began.elapsed().as_secs(), text);
        self.events.push_back(
            json!({"elapsed_seconds":self.began.elapsed().as_secs(),"kind":kind,"message":text}),
        );
        while self.events.len() > 200 {
            self.events.pop_front();
        }
    }
    pub fn poll(&mut self) {
        for _ in 0..64 {
            let Some(event) = self.worker.as_ref().and_then(Worker::receive) else {
                break;
            };
            match event {
                Event::ClientChanged(pid) => self.pid = pid,
                Event::VisualChanged => {} // No preview request, window capture, or renderer.
                Event::Status(status) => {
                    self.record("status", &status);
                    self.status = status;
                }
                Event::Lobby(lobby) => self.lobby = Some(lobby),
                Event::LobbyCleared => self.lobby = None,
                Event::Reserve(number) => {
                    let result = self.reserve(number);
                    if let Some(worker) = &self.worker {
                        worker.send(Control::Reserved {
                            number,
                            saved: result.is_ok(),
                        });
                    }
                    if let Err(error) = result {
                        self.record("error", &error);
                    }
                }
                Event::Finished(result) => {
                    self.status = if result.is_ok() {
                        "Stopped"
                    } else {
                        "Stopped with an error"
                    }
                    .into();
                    if let Err(error) = &result {
                        self.record("error", error);
                    }
                    self.last_result = result;
                    self.worker = None;
                    self.lobby = None;
                    self.pid = None;
                }
                Event::Activity(activity) => {
                    let (kind, text) = match activity {
                        Activity::LobbyCreated(name) => ("lobby_created", name),
                        Activity::GameStarted(name) => ("game_started", name),
                        Activity::Joined(name) => ("joined", name),
                        Activity::Left(name) => ("left", name),
                        Activity::Command(text) => ("command", text),
                    };
                    self.record(kind, &text);
                }
            }
        }
    }
    fn reserve(&mut self, number: u32) -> Result<(), String> {
        let next = number.checked_add(1).ok_or("Lobby number exhausted")?;
        if self.reserved == Some(number) && self.selected().next_number == next {
            return Ok(());
        }
        if self.selected().next_number != number {
            return Err("The saved lobby counter changed unexpectedly".into());
        }
        let mut updated = self.profiles.clone();
        updated.items[updated.selected].next_number = next;
        self.commit(updated)?;
        self.reserved = Some(number);
        Ok(())
    }
    pub fn apply(&mut self, action: Action) -> Result<Value, String> {
        self.poll();
        if self.quitting && !matches!(action, Action::Status | Action::Events | Action::Quit) {
            return Err("Controller is shutting down".into());
        }
        match action {
            Action::Status => Ok(self.snapshot()),
            Action::Host => {
                self.stopped()?;
                let config = self.selected().config(&self.directory)?;
                self.worker = Some(Worker::start(config)?);
                self.reserved = None;
                self.last_result = Ok(());
                self.status = "Connecting…".into();
                Ok(json!({"accepted":"host","confirmed":false}))
            }
            Action::Stop | Action::Quit => {
                if matches!(action, Action::Quit) {
                    self.quitting = true;
                }
                if let Some(worker) = &self.worker {
                    worker.send(Control::Stop);
                    self.status = "Stopping…".into();
                } else {
                    self.last_result.clone()?;
                }
                Ok(json!({"accepted":"stop","confirmed":!self.running(),"quitting":self.quitting}))
            }
            Action::Start => {
                if !self.lobby.as_ref().is_some_and(|l| l.can_start) {
                    return Err("The current lobby is not ready to start".into());
                }
                self.worker
                    .as_ref()
                    .ok_or("Not hosting")?
                    .send(Control::Start);
                Ok(json!({"accepted":"start","confirmed":false}))
            }
            Action::Profiles => {
                Ok(json!({"selected":self.profiles.selected+1,"profiles":self.profiles.items}))
            }
            Action::ProfileGet { profile } => {
                let index = self.index(profile.as_deref())?;
                Ok(json!(self.profiles.items[index]))
            }
            Action::ProfileSelect { profile } => {
                self.stopped()?;
                let mut updated = self.profiles.clone();
                updated.selected = profile_index(&updated, &profile)?;
                self.commit(updated)?;
                Ok(json!(self.selected()))
            }
            Action::ProfileNew { title } => {
                self.stopped()?;
                let mut updated = self.profiles.clone();
                updated.items.push(Profile {
                    title,
                    ..Profile::default()
                });
                updated.selected = updated.items.len() - 1;
                self.commit(updated)?;
                Ok(json!(self.selected()))
            }
            Action::ProfileSet { profile, settings } => self.edit(profile.as_deref(), settings),
            Action::Slot {
                lobby,
                number,
                action,
            } => {
                let roster = self.owned(&lobby)?;
                let slot = number.checked_sub(1).ok_or("Slot numbers start at 1")?;
                let request = roster
                    .slot_actions(slot)
                    .into_iter()
                    .find(|(kind, _)| {
                        use crate::hosting::SlotAction::*;
                        matches!(
                            (kind, action.as_str()),
                            (Open, "open")
                                | (Close, "close")
                                | (Observer, "observer")
                                | (Computer, "computer")
                        )
                    })
                    .map(|(_, request)| request)
                    .ok_or("That slot action is not currently permitted")?;
                self.worker
                    .as_ref()
                    .ok_or("Not hosting")?
                    .send(Control::EditSlot(request));
                Ok(json!({"accepted":"slot","confirmed":false}))
            }
            Action::Kick { lobby, player } => {
                let roster = self.owned(&lobby)?;
                let matching: Vec<_> = roster
                    .slots
                    .iter()
                    .filter(|slot| {
                        slot.name == player && !slot.own && !slot.computer && slot.status == 2
                    })
                    .collect();
                let [target] = matching.as_slice() else {
                    return Err(
                        "Player name missing or ambiguous; use the full name from status".into(),
                    );
                };
                let request = roster
                    .kick_target(target.number)
                    .ok_or("Cannot kick that player")?;
                self.worker
                    .as_ref()
                    .ok_or("Not hosting")?
                    .send(Control::Kick(request));
                Ok(json!({"accepted":"kick","confirmed":false}))
            }
            Action::Events => Ok(json!(self.events)),
            Action::ClearEvents => {
                self.events.clear();
                Ok(json!({"cleared":true}))
            }
            Action::Commands => {
                let commands: Vec<_> = crate::commands::ChatCommand::ALL.into_iter()
                    .map(|c| json!({"name":c.name(),"description":c.description(),"enabled":self.selected().commands.enabled(c)}))
                    .collect();
                Ok(json!(commands))
            }
            Action::Maps => maps(),
        }
    }
    fn index(&self, title: Option<&str>) -> Result<usize, String> {
        title.map_or(Ok(self.profiles.selected), |p| {
            profile_index(&self.profiles, p)
        })
    }
    fn owned(&self, expected: &str) -> Result<&Snapshot, String> {
        self.lobby
            .as_ref()
            .filter(|l| l.name == expected)
            .ok_or_else(|| "The expected lobby is no longer current; refresh status".into())
    }
    fn edit(&mut self, title: Option<&str>, settings: Value) -> Result<Value, String> {
        let index = self.index(title)?;
        let object = settings
            .as_object()
            .ok_or("Settings must be a JSON object")?;
        let live = index == self.profiles.selected && self.running();
        const LIVE: &[&str] = &[
            "headless",
            "title",
            "players",
            "require_full",
            "rehost",
            "rehost_delay_seconds",
            "empty_rehost_seconds",
            "commands",
            "welcome",
        ];
        if live && object.keys().any(|k| !LIVE.contains(&k.as_str())) {
            return Err("These settings require Stop first; live fields: headless, title, players, require_full, rehost, rehost_delay_seconds, empty_rehost_seconds, commands, welcome".into());
        }
        if object.contains_key("auto_start")
            || object.contains_key("bot_name")
            || object.contains_key("countdown")
        {
            return Err("Legacy auto_start/bot_name/countdown fields are not configurable; native countdown is authoritative.".into());
        }
        let mut value = json!(self.profiles.items[index]);
        for (key, new) in object {
            if key == "welcome" {
                let fields = new.as_object().ok_or("welcome must be an object")?;
                for (field, v) in fields {
                    value[key][field] = v.clone();
                }
            } else {
                value[key] = new.clone();
            }
        }
        let mut profile: Profile =
            serde_json::from_value(value).map_err(|e| format!("Invalid profile: {e}"))?;
        profile.validate()?;
        if object.contains_key("map")
            && let Some(map) = &profile.map
        {
            profile.map = Some(crate::maps::validate(map)?);
            if profile.automatic_name && !object.contains_key("lobby_pattern") {
                profile.lobby_pattern = crate::map_name::pattern(profile.map.as_ref().unwrap());
            }
        }
        let settings = profile.live_settings();
        settings.validate()?;
        let mut updated = self.profiles.clone();
        updated.items[index] = profile;
        self.commit(updated)?;
        if live {
            self.worker
                .as_ref()
                .unwrap()
                .send(Control::Settings(settings));
        }
        Ok(json!({"profile":self.profiles.items[index],"live_update_queued":live}))
    }
    fn snapshot(&self) -> Value {
        let lobby=self.lobby.as_ref().map(|l|json!({"name":l.name,"humans":l.humans,"can_start":l.can_start,
            "slots":l.slots.iter().map(|s|json!({"number":s.number+1,"name":s.name,"computer":s.computer,"observer":s.observer,"own":s.own,"status":s.status,
                "actions":l.slot_actions(s.number).into_iter().map(|(a,_)|a.label()).collect::<Vec<_>>()})).collect::<Vec<_>>()}));
        json!({"running":self.running(),"status":self.status,"profile":self.selected().title,"headless":self.selected().headless,
            "client_pid":self.pid,"lobby":lobby,"last_error":self.last_result.as_ref().err(),"quitting":self.quitting})
    }
}

fn maps() -> Result<Value, String> {
    let root = crate::maps::documents_directory()
        .ok_or("Cannot locate the Warcraft III map directory")?
        .join("Warcraft III/Maps");
    let mut pending = vec![root];
    let mut paths = Vec::new();
    let mut examined = 0usize;
    while let Some(directory) = pending.pop() {
        for entry in std::fs::read_dir(directory).map_err(|_| "Cannot read map directory")? {
            examined += 1;
            if examined > 10000 {
                return Err("Map listing exceeds 10000 entries".into());
            }
            let entry = entry.map_err(|_| "Cannot read map entry")?;
            let kind = entry.file_type().map_err(|_| "Cannot inspect map entry")?;
            if kind.is_symlink() {
                continue;
            }
            let path = entry.path();
            if kind.is_dir() {
                pending.push(path);
            } else if kind.is_file()
                && path
                    .extension()
                    .is_some_and(|e| e.eq_ignore_ascii_case("w3x") || e.eq_ignore_ascii_case("w3m"))
            {
                paths.push(path);
            }
        }
    }
    paths.sort();
    Ok(json!(paths))
}
