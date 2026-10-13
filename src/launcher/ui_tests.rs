use super::{
    app::{App, Message, Page},
    motion::Menu,
    palette, view,
};
use iced::{
    Event, Point, Renderer, Size,
    advanced::{clipboard, mouse, renderer},
    window,
};
use iced_runtime::{UserInterface, user_interface::Cache};
use std::{
    path::Path,
    time::{Duration, Instant},
};

struct Harness {
    renderer: Renderer,
    cache: Cache,
    size: Size,
}

impl Harness {
    fn new() -> Self {
        Self {
            renderer: Renderer::new(iced::Font::with_name("Segoe UI"), 16.into()),
            cache: Cache::new(),
            size: Size::new(1180.0, 830.0),
        }
    }
    fn frame(
        &mut self,
        app: &App,
        position: Option<Point>,
        click: bool,
        name: &str,
    ) -> Vec<Message> {
        use renderer::Headless;
        let cursor = position.map(mouse::Cursor::Available).unwrap_or_default();
        let mut events = vec![Event::Window(
            window::Event::RedrawRequested(Instant::now()),
        )];
        if let Some(position) = position {
            events.push(Event::Mouse(mouse::Event::CursorMoved { position }));
        }
        if click {
            events.push(Event::Mouse(mouse::Event::ButtonPressed(
                mouse::Button::Left,
            )));
            events.push(Event::Mouse(mouse::Event::ButtonReleased(
                mouse::Button::Left,
            )));
        }
        let mut messages = Vec::new();
        let mut ui = UserInterface::build(
            view::view(app),
            self.size,
            std::mem::take(&mut self.cache),
            &mut self.renderer,
        );
        ui.update(
            &events,
            cursor,
            &mut self.renderer,
            &mut clipboard::Null,
            &mut messages,
        );
        ui.draw(
            &mut self.renderer,
            &palette::theme(app),
            &renderer::Style {
                text_color: palette::TEXT,
            },
            cursor,
        );
        self.cache = ui.into_cache();
        let rgba = self.renderer.screenshot(
            Size::new(self.size.width as u32, self.size.height as u32),
            1.0,
            palette::BG,
        );
        if let Some(directory) = std::env::var_os("GHOST_UI_ARTIFACTS") {
            std::fs::create_dir_all(&directory).unwrap();
            ::image::save_buffer(
                Path::new(&directory).join(format!("{name}.png")),
                &rgba,
                self.size.width as u32,
                self.size.height as u32,
                ::image::ColorType::Rgba8,
            )
            .unwrap();
        }
        messages
    }
    fn settle(&mut self, app: &mut App) {
        let _ = app.update(Message::Animate(Instant::now() + Duration::from_secs(1)));
        let _ = app.update(Message::Animate(Instant::now() + Duration::from_secs(2)));
    }
}

#[test]
fn update_modal_is_compact_blocks_underlying_controls_and_can_be_deferred() {
    let directory = tempfile::tempdir().unwrap();
    let mut app = App::new(directory.path(), None);
    let release = serde_json::json!([{"tag_name":"v9.0.0","draft":false,"prerelease":true,"assets":[{
        "name":"GHost-Reforged-Setup-x64.exe","state":"uploaded","size":123,
        "browser_download_url":"https://github.com/shideneyu/ghost-reforged/releases/download/v9.0.0/GHost-Reforged-Setup-x64.exe",
        "digest":format!("sha256:{}", "a".repeat(64))
    }]}]);
    app.updates.available =
        super::updates::select(&serde_json::to_vec(&release).unwrap(), "0.2.3").unwrap();
    let mut harness = Harness::new();
    harness.settle(&mut app);
    harness.frame(&app, None, false, "update-modal");
    let covered = harness.frame(
        &app,
        Some(Point::new(100.0, 280.0)),
        true,
        "update-covered-controls",
    );
    assert!(!covered.iter().any(|m| matches!(m, Message::Page(_))));
    let dismissed = harness.frame(
        &app,
        Some(Point::new(640.0, 485.0)),
        true,
        "update-later-click",
    );
    assert!(
        dismissed
            .iter()
            .any(|m| matches!(m, Message::DismissUpdate))
    );
    for message in dismissed {
        let _ = app.update(message);
    }
    harness.frame(&app, None, false, "update-dismissed-version");
    assert!(app.updates.available.is_none());
}

#[test]
fn rendered_menu_opens_selects_and_closes_without_clicking_underneath() {
    let directory = tempfile::tempdir().unwrap();
    let mut app = App::new(directory.path(), Some("SurvivalChaosReborn.w3x".into()));
    app.form.profile.require_full = false;
    let mut harness = Harness::new();
    harness.settle(&mut app);
    harness.frame(&app, None, false, "hosting");
    let messages = harness.frame(&app, Some(Point::new(620.0, 500.0)), true, "menu-pressed");
    assert!(
        messages
            .iter()
            .any(|m| matches!(m, Message::Menu(Menu::StartRule))),
        "{messages:?}"
    );
    for message in messages {
        let _ = app.update(message);
    }
    harness.settle(&mut app);
    harness.frame(&app, None, false, "menu-open");
    let messages = harness.frame(&app, Some(Point::new(620.0, 421.0)), true, "menu-selection");
    assert!(
        messages
            .iter()
            .any(|m| matches!(m, Message::RequireFull(true))),
        "{messages:?}"
    );
    for message in messages {
        let _ = app.update(message);
    }
    harness.settle(&mut app);
    harness.frame(&app, None, false, "menu-closed");
    assert!(app.form.profile.require_full);
    assert!(!app.motion.menus[Menu::StartRule.index()].is_open());
}

#[test]
fn all_pages_and_the_preview_render_at_the_minimum_window_size() {
    let directory = tempfile::tempdir().unwrap();
    let mut app = App::new(directory.path(), None);
    app.preview.image = Some(super::art::backdrop());
    let mut harness = Harness::new();
    harness.size = Size::new(1000.0, 720.0);
    for (page, name) in [
        (Page::Hosting, "small-hosting"),
        (Page::Options, "small-options"),
        (Page::Profiles, "small-profiles"),
        (Page::Activity, "small-activity"),
    ] {
        let _ = app.update(Message::Page(page));
        harness.settle(&mut app);
        harness.frame(&app, None, false, name);
    }
    app.preview.image = Some(super::art::backdrop());
    let _ = app.update(Message::OpenPreview);
    harness.settle(&mut app);
    harness.frame(&app, None, false, "small-preview");
    let _ = app.update(Message::ClosePreview);
    harness.settle(&mut app);
    assert!(!app.motion.modal_visible());
}

#[test]
fn an_opaque_button_cannot_cover_its_decorative_frame() {
    use iced::{
        Element,
        widget::{button, text},
    };
    use renderer::Headless;
    let render = |framed: bool| {
        let mut renderer = Renderer::new(iced::Font::default(), 16.into());
        let content = super::effects::hover(
            button(text("Host"))
                .width(100)
                .height(44)
                .style(palette::primary),
            true,
        );
        let content: Element<'_, Message> = if framed {
            super::effects::frame(content)
        } else {
            content
        };
        let mut ui =
            UserInterface::build(content, Size::new(100.0, 44.0), Cache::new(), &mut renderer);
        ui.draw(
            &mut renderer,
            &iced::Theme::Dark,
            &renderer::Style {
                text_color: palette::TEXT,
            },
            mouse::Cursor::Unavailable,
        );
        renderer.screenshot(Size::new(100, 44), 1.0, palette::BG)
    };
    let plain = render(false);
    let framed = render(true);
    let changed_border_pixels = (0..8)
        .flat_map(|y| (0..8).map(move |x| (y * 100 + x) * 4))
        .filter(|&offset| plain[offset..offset + 4] != framed[offset..offset + 4])
        .count();
    assert!(
        changed_border_pixels > 16,
        "Frame was covered by its child layer"
    );
}

#[test]
fn window_controls_remain_reachable_above_the_preview() {
    use super::app::WindowAction;
    let directory = tempfile::tempdir().unwrap();
    let mut app = App::new(directory.path(), None);
    let mut harness = Harness::new();
    harness.settle(&mut app);
    for expanded in [false, true] {
        if expanded {
            app.preview.image = Some(super::art::backdrop());
            let _ = app.update(Message::OpenPreview);
            harness.settle(&mut app);
        }
        for (x, action) in [
            (1000.0, WindowAction::Minimize),
            (1064.0, WindowAction::Maximize),
            (1128.0, WindowAction::Close),
        ] {
            let messages = harness.frame(&app, Some(Point::new(x, 27.0)), true, "window-controls");
            assert!(messages.iter().any(|m| matches!(m, Message::Window(actual) if std::mem::discriminant(actual) == std::mem::discriminant(&action))), "{messages:?}");
            assert!(app.update(Message::Window(action)).units() > 0);
        }
    }
}

#[test]
fn host_remains_clickable_at_the_minimum_window_size() {
    let directory = tempfile::tempdir().unwrap();
    let mut app = App::new(directory.path(), None);
    let mut harness = Harness::new();
    harness.size = Size::new(1000.0, 720.0);
    harness.settle(&mut app);
    let messages = harness.frame(
        &app,
        Some(Point::new(300.0, 595.0)),
        true,
        "small-host-button",
    );
    assert!(
        messages
            .iter()
            .any(|message| matches!(message, Message::Host)),
        "{messages:?}"
    );
}

#[test]
fn headless_hides_the_preview_and_can_return_to_normal_without_restarting() {
    let directory = tempfile::tempdir().unwrap();
    let mut app = App::new(directory.path(), None);
    let mut harness = Harness::new();
    app.preview.image = Some(super::art::backdrop());
    harness.settle(&mut app);
    harness.frame(&app, None, false, "mode-normal");
    let _ = app.update(Message::Headless(true));
    harness.settle(&mut app);
    assert!(app.form.profile.headless);
    assert!(app.preview.image.is_none());
    assert!(!app.preview.needs_poll());
    // Even a late/stale image may not open a headless preview.
    app.preview.image = Some(super::art::backdrop());
    let _ = app.update(Message::OpenPreview);
    assert!(!app.preview.expanded);
    harness.frame(&app, None, false, "mode-headless");
    harness.size = Size::new(1000.0, 720.0);
    let messages = harness.frame(
        &app,
        Some(Point::new(300.0, 595.0)),
        true,
        "mode-headless-small",
    );
    assert!(
        messages
            .iter()
            .any(|message| matches!(message, Message::Host))
    );
    let _ = app.update(Message::Headless(false));
    app.preview.image = Some(super::art::backdrop());
    let _ = app.update(Message::OpenPreview);
    assert!(app.preview.expanded);
}

#[test]
fn occupied_player_rows_send_the_displayed_identity_when_kicked() {
    use ghost_reforged::hosting::{Slot, Snapshot};
    let directory = tempfile::tempdir().unwrap();
    let mut app = App::new(directory.path(), None);
    app.lobby = Some(Snapshot {
        name: "Test #7".into(),
        humans: 1,
        can_start: true,
        can_edit_slots: true,
        slots: vec![Slot {
            id: Some(42),
            name: "Player".into(),
            number: 1,
            status: 2,
            computer: false,
            observer: false,
            own: false,
            change_type: true,
            change_team: true,
        }],
    });
    let expected = app.lobby.as_ref().unwrap().kick_target(1).unwrap();
    let mut harness = Harness::new();
    harness.settle(&mut app);
    let messages = harness.frame(&app, Some(Point::new(1090.0, 317.0)), true, "kick-button");
    assert!(
        messages
            .iter()
            .any(|m| matches!(m, Message::Kick(target) if target == &expected)),
        "{messages:?}"
    );
    app.lobby.as_mut().unwrap().can_edit_slots = false;
    assert!(
        !harness
            .frame(&app, Some(Point::new(1090.0, 317.0)), true, "kick-disabled")
            .iter()
            .any(|m| matches!(m, Message::Kick(_)))
    );
}

#[test]
fn open_slot_menu_offers_computer_and_keeps_its_target_identity() {
    use ghost_reforged::hosting::{Slot, SlotAction, Snapshot};
    let directory = tempfile::tempdir().unwrap();
    let mut app = App::new(directory.path(), None);
    app.lobby = Some(Snapshot {
        name: "Test #7".into(),
        humans: 0,
        can_start: false,
        can_edit_slots: true,
        slots: vec![Slot {
            id: None,
            name: String::new(),
            number: 1,
            status: 0,
            computer: false,
            observer: false,
            own: false,
            change_type: true,
            change_team: false,
        }],
    });
    let expected = app
        .lobby
        .as_ref()
        .unwrap()
        .slot_actions(1)
        .into_iter()
        .find(|(action, _)| *action == SlotAction::Computer)
        .unwrap()
        .1;
    let mut harness = Harness::new();
    harness.settle(&mut app);
    let messages = harness.frame(
        &app,
        Some(Point::new(1090.0, 317.0)),
        true,
        "slot-menu-button",
    );
    assert!(
        messages
            .iter()
            .any(|m| matches!(m, Message::Menu(Menu::Slot(1))))
    );
    for message in messages {
        let _ = app.update(message);
    }
    harness.settle(&mut app);
    harness.frame(&app, None, false, "slot-menu-open");
    let messages = harness.frame(
        &app,
        Some(Point::new(1060.0, 397.0)),
        true,
        "slot-computer-selected",
    );
    assert!(
        messages
            .iter()
            .any(|m| matches!(m, Message::Slot(request) if request == &expected)),
        "{messages:?}"
    );
}
