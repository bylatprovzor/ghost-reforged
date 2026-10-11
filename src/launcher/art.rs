use iced::{
    Font,
    widget::{image, svg},
};
use std::sync::OnceLock;

pub const DISPLAY: Font = Font {
    weight: iced::font::Weight::Bold,
    ..Font::with_name("Georgia")
};
pub const LABEL: Font = Font::with_name("Georgia");

pub fn backdrop() -> image::Handle {
    static HANDLE: OnceLock<image::Handle> = OnceLock::new();
    HANDLE
        .get_or_init(|| {
            let pixels = ::image::load_from_memory(include_bytes!("../../design/citadel.jpg"))
                .expect("embedded citadel image")
                .to_rgba8();
            image::Handle::from_rgba(pixels.width(), pixels.height(), pixels.into_raw())
        })
        .clone()
}

pub fn crop(x: u32, y: u32, width: u32, height: u32) -> image::Handle {
    static SPRITES: OnceLock<std::collections::HashMap<[u32; 4], image::Handle>> = OnceLock::new();
    SPRITES
        .get_or_init(|| {
            let atlas = ::image::load_from_memory(include_bytes!("../../design/citadel-skin.png"))
                .expect("embedded interface atlas");
            let regions: Vec<[u32; 6]> =
                serde_json::from_slice(include_bytes!("../../design/citadel-skin.json"))
                    .expect("embedded sprite coordinates");
            regions
                .into_iter()
                .map(|[x, y, w, h, px, py]| {
                    let pixels = atlas.crop_imm(px, py, w, h).to_rgba8();
                    (
                        [x, y, w, h],
                        image::Handle::from_rgba(w, h, pixels.into_raw()),
                    )
                })
                .collect()
        })
        .get(&[x, y, width, height])
        .expect("known skin region")
        .clone()
}

#[derive(Clone, Copy)]
pub enum Icon {
    Tower,
    People,
    Gear,
    Scroll,
    Chevron,
    User,
    Map,
    Game,
    Number,
    Power,
    Players,
    Play,
    Swords,
    Eye,
    Save,
}

pub fn icon(kind: Icon, size: f32) -> image::Image {
    static HANDLES: OnceLock<Vec<image::Handle>> = OnceLock::new();
    let handles = HANDLES.get_or_init(|| {
        [
            [52, 246, 43, 59],
            [51, 339, 48, 47],
            [50, 421, 47, 47],
            [50, 502, 47, 49],
            [878, 696, 34, 44],
            [51, 339, 48, 47],
            [52, 502, 46, 47],
            [50, 502, 47, 49],
            [52, 502, 46, 47],
            [52, 421, 46, 47],
            [51, 339, 48, 47],
            [324, 864, 39, 41],
            [996, 240, 48, 48],
            [1001, 683, 48, 34],
            [555, 865, 40, 40],
        ]
        .into_iter()
        .map(|[x, y, w, h]| crop(x, y, w, h))
        .collect()
    });
    image(handles[kind as usize].clone())
        .width(size)
        .height(size)
        .content_fit(iced::ContentFit::Contain)
}

pub fn ghost(size: f32) -> svg::Svg<'static> {
    static HANDLE: OnceLock<svg::Handle> = OnceLock::new();
    svg(HANDLE
        .get_or_init(|| svg::Handle::from_memory(include_bytes!("../../design/ghost.svg")))
        .clone())
    .width(size)
    .height(size * 1.16)
}

pub fn separator() -> image::Image {
    static HANDLE: OnceLock<image::Handle> = OnceLock::new();
    image(HANDLE.get_or_init(|| crop(318, 289, 576, 19)).clone())
        .width(iced::Fill)
        .height(10)
        .content_fit(iced::ContentFit::Fill)
}
