use super::{
    app::{App, Form},
    map_name,
};
use ghost_reforged::console::{Args, profile_index};
impl App {
    pub(super) fn apply_launch_args(&mut self, args: &Args) -> Result<(), String> {
        args.validate()?;
        let mut updated = self.profiles.clone();
        if let Some(title) = &args.profile {
            updated.selected = profile_index(&updated, title)?;
        }
        let profile = &mut updated.items[updated.selected];
        if let Some(map) = &args.map {
            let path = super::maps::validate(map)?;
            if profile.automatic_name {
                profile.lobby_pattern = map_name::pattern(&path);
            }
            profile.map = Some(path);
        }
        if args.headless || args.normal {
            profile.headless = args.headless;
        }
        if args.profile.is_some() || args.map.is_some() || args.headless || args.normal {
            self.store
                .as_ref()
                .ok_or("Profile storage is unavailable.")?
                .save(&updated)
                .map_err(|e| e.to_string())?;
            self.form = Form::from(updated.items[updated.selected].clone());
            self.profiles = updated;
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use clap::Parser;

    #[test]
    fn launch_selection_is_atomic_and_persists_the_requested_mode() {
        let directory = tempfile::tempdir().unwrap();
        let mut app = App::new(directory.path(), None);
        app.profiles.items.push(super::super::profile::Profile {
            title: "VPS".into(),
            ..Default::default()
        });
        app.save().unwrap();
        let before = std::fs::read(directory.path().join("profiles.json")).unwrap();
        let invalid = Args::try_parse_from([
            "ghost",
            "--profile",
            "VPS",
            "--map",
            "missing.w3x",
            "--headless",
        ])
        .unwrap();
        assert!(app.apply_launch_args(&invalid).is_err());
        assert_eq!(app.profiles.selected, 0);
        assert!(!app.form.profile.headless);
        assert_eq!(
            std::fs::read(directory.path().join("profiles.json")).unwrap(),
            before
        );
        let valid = Args::try_parse_from(["ghost", "--profile", "VPS", "--headless"]).unwrap();
        app.apply_launch_args(&valid).unwrap();
        assert_eq!(app.profiles.selected, 1);
        assert!(app.form.profile.headless);
        drop(app);
        let app = App::new(directory.path(), None);
        assert_eq!(app.form.profile.title, "VPS");
        assert!(app.form.profile.headless);
    }
}
