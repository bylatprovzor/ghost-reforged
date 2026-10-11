use super::{
    app::{Message, WindowAction},
    effects, palette,
};
use iced::{
    Element,
    widget::{button, container, row, svg, text, tooltip},
};
use std::sync::OnceLock;

pub fn highlight(renderer: &mut iced::Renderer, bounds: iced::Rectangle, opacity: f32) {
    use iced::advanced::svg::Renderer as _;
    static OUTLINE: OnceLock<svg::Handle> = OnceLock::new();
    let handle = OUTLINE.get_or_init(|| svg::Handle::from_memory(
        br#"<svg xmlns="http://www.w3.org/2000/svg" width="64" height="64"><path d="M16 10H47L54 17V46L47 53H16L9 46V17Z" fill="none" stroke="white" stroke-width="1.3"/></svg>"#
    ));
    renderer.draw_svg(
        iced::advanced::svg::Svg::new(handle.clone()).color(palette::GOLD.scale_alpha(opacity)),
        bounds,
        bounds,
    );
}

pub fn controls() -> Element<'static, Message> {
    static ICONS: OnceLock<Vec<svg::Handle>> = OnceLock::new();
    let icons = ICONS.get_or_init(|| {
        [
            "M23 32H40V36H23Z",
            "M23 23H40V39H23ZM26 26V36H37V26Z",
            "M24 23 31.5 30.5 39 23 42 26 34.5 33.5 42 41 39 44 31.5 36.5 24 44 21 41 28.5 33.5 21 26Z",
        ].map(|path| {
            let glyph = format!(r##"<path d="{path}" fill="url(#gold)" fill-rule="evenodd" stroke="#100c04" stroke-width="1.1"/>"##);
            svg::Handle::from_memory(include_str!("../../design/window-control.svg").replace("<!-- glyph -->", &glyph).into_bytes())
        }).to_vec()
    });
    let mut controls = row![].spacing(0);
    for ((label, action), icon) in [
        ("Minimize", WindowAction::Minimize),
        ("Maximize / restore", WindowAction::Maximize),
        ("Close", WindowAction::Close),
    ]
    .into_iter()
    .zip(icons)
    {
        controls = controls.push(tooltip(
            effects::press(
                button(svg(icon.clone()).width(64).height(64))
                    .padding(0)
                    .style(palette::window_button)
                    .on_press(Message::Window(action)),
            ),
            container(text(label).size(12))
                .padding(7)
                .style(palette::panel),
            tooltip::Position::Bottom,
        ));
    }
    controls.into()
}
