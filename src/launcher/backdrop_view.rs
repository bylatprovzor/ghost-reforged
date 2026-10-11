use super::app::Message;
use iced::{
    Element, Length, Rectangle, Renderer, Size, Theme,
    advanced::{
        Layout, Widget,
        image::{self, Renderer as _},
        layout, mouse, renderer,
        widget::{Tree, tree},
    },
};
use std::cell::RefCell;

pub const HEADER: f32 = 166.0;
pub const SIDEBAR: f32 = 208.0;
pub const SIDEBAR_GAP: f32 = 8.0;
pub const RIGHT: f32 = 20.0;
pub const BOTTOM: f32 = 24.0;
pub const COLUMNS_GAP: f32 = 12.0;
pub const PREVIEW_GAP: f32 = 14.0;
pub const PREVIEW_HEIGHT: f32 = 286.0;

#[derive(Clone, Copy, PartialEq)]
pub enum Coverage {
    None,
    Hosting,
    Page,
}

impl Coverage {
    pub fn for_page(page: super::app::Page, progress: f32) -> Self {
        if progress < 1.0 {
            Self::None
        } else if page == super::app::Page::Hosting {
            Self::Hosting
        } else {
            Self::Page
        }
    }
}

struct Backdrop {
    source: image::Handle,
    coverage: Coverage,
}

#[derive(PartialEq)]
struct Key {
    source: image::Id,
    bounds: Rectangle,
    coverage: Coverage,
}

struct Tile {
    pixels: image::Handle,
    bounds: Rectangle,
    clip: Rectangle,
}

#[derive(Default)]
struct Cache {
    key: Option<Key>,
    tiles: Vec<Tile>,
}

pub fn view(source: image::Handle, coverage: Coverage) -> Element<'static, Message> {
    Element::new(Backdrop { source, coverage })
}

// Upload only the exposed backdrop. Keeping each image's actual bounds small
// also keeps software-renderer damage out of the unchanged opaque panels.
impl Widget<Message, Theme, Renderer> for Backdrop {
    fn tag(&self) -> tree::Tag {
        tree::Tag::of::<RefCell<Cache>>()
    }
    fn state(&self) -> tree::State {
        tree::State::new(RefCell::new(Cache::default()))
    }
    fn size(&self) -> Size<Length> {
        Size::new(Length::Fill, Length::Fill)
    }
    fn layout(&mut self, _: &mut Tree, _: &Renderer, limits: &layout::Limits) -> layout::Node {
        layout::Node::new(limits.max())
    }
    fn draw(
        &self,
        tree: &Tree,
        renderer: &mut Renderer,
        _: &Theme,
        _: &renderer::Style,
        layout: Layout<'_>,
        _: mouse::Cursor,
        viewport: &Rectangle,
    ) {
        let key = Key {
            source: self.source.id(),
            bounds: layout.bounds(),
            coverage: self.coverage,
        };
        let mut cache = tree.state.downcast_ref::<RefCell<Cache>>().borrow_mut();
        if cache.key.as_ref() != Some(&key) {
            cache.tiles = visible_regions(key.bounds, key.coverage)
                .into_iter()
                .filter_map(|region| crop(&self.source, key.bounds, region))
                .collect();
            cache.key = Some(key);
        }
        for tile in &cache.tiles {
            if let Some(clip) = tile.clip.intersection(viewport) {
                renderer.draw_image(image::Image::new(tile.pixels.clone()), tile.bounds, clip);
            }
        }
    }
}

fn visible_regions(bounds: Rectangle, coverage: Coverage) -> Vec<Rectangle> {
    if coverage == Coverage::None || bounds.width < 600.0 || bounds.height < 500.0 {
        return vec![bounds];
    }
    let left = SIDEBAR + SIDEBAR_GAP;
    let body_width = bounds.width - left - RIGHT;
    let body_height = bounds.height - HEADER - BOTTOM;
    let rect = |x, y, width, height| Rectangle {
        x: bounds.x + x,
        y: bounds.y + y,
        width,
        height,
    };
    let mut regions = vec![
        rect(0.0, 0.0, bounds.width, HEADER),
        rect(0.0, HEADER, left, bounds.height - HEADER),
        rect(bounds.width - RIGHT, HEADER, RIGHT, bounds.height - HEADER),
        rect(left, bounds.height - BOTTOM, body_width, BOTTOM),
    ];
    if coverage == Coverage::Hosting {
        let form_width = (body_width - COLUMNS_GAP) * 0.6;
        regions.push(rect(left + form_width, HEADER, COLUMNS_GAP, body_height));
        regions.push(rect(
            left + form_width + COLUMNS_GAP,
            bounds.height - BOTTOM - PREVIEW_HEIGHT - PREVIEW_GAP,
            body_width - form_width - COLUMNS_GAP,
            PREVIEW_GAP,
        ));
    }
    regions
}

fn crop(source: &image::Handle, bounds: Rectangle, region: Rectangle) -> Option<Tile> {
    let image::Handle::Rgba {
        width,
        height,
        pixels,
        ..
    } = source
    else {
        return None;
    };
    let scale = (bounds.width / *width as f32).max(bounds.height / *height as f32);
    let origin_x = bounds.x + (bounds.width - *width as f32 * scale) / 2.0;
    let origin_y = bounds.y + (bounds.height - *height as f32 * scale) / 2.0;
    let x = ((region.x - origin_x) / scale).floor().max(0.0) as u32;
    let y = ((region.y - origin_y) / scale).floor().max(0.0) as u32;
    let right = ((region.x + region.width - origin_x) / scale)
        .ceil()
        .min(*width as f32) as u32;
    let bottom = ((region.y + region.height - origin_y) / scale)
        .ceil()
        .min(*height as f32) as u32;
    let w = right.checked_sub(x).filter(|w| *w > 0)?;
    let h = bottom.checked_sub(y).filter(|h| *h > 0)?;
    let mut rgba = Vec::with_capacity((w * h * 4) as usize);
    for row in y..bottom {
        let start = ((row * width + x) * 4) as usize;
        rgba.extend_from_slice(&pixels[start..start + (w * 4) as usize]);
    }
    Some(Tile {
        pixels: image::Handle::from_rgba(w, h, rgba),
        // Preserve fractional cover coordinates at adjoining strip edges.
        bounds: Rectangle {
            x: origin_x + x as f32 * scale,
            y: origin_y + y as f32 * scale,
            width: w as f32 * scale,
            height: h as f32 * scale,
        },
        clip: region,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_almost_finished_slide_still_needs_the_complete_backdrop() {
        use super::super::app::Page;
        for page in [Page::Hosting, Page::Profiles, Page::Options, Page::Activity] {
            for progress in [0.0, 0.5, 0.999, 0.999_999] {
                assert!(Coverage::for_page(page, progress) == Coverage::None);
            }
            assert!(Coverage::for_page(page, 1.0) != Coverage::None);
        }
    }

    #[test]
    fn exposed_strips_cover_the_background_without_damaging_panel_interiors() {
        for (width, height) in [(1000.0, 720.0), (1180.0, 856.0), (1936.0, 1176.0)] {
            let bounds = Rectangle::with_size(Size::new(width, height));
            for coverage in [Coverage::Hosting, Coverage::Page] {
                let regions = visible_regions(bounds, coverage);
                let area: f32 = regions.iter().map(Rectangle::area).sum();
                assert!(
                    area < bounds.area() * 0.47,
                    "Too much background redraw: {area}"
                );
                for (index, region) in regions.iter().enumerate() {
                    assert_eq!(bounds.intersection(region), Some(*region));
                    for neighbor in &regions[index + 1..] {
                        assert!(region.intersection(neighbor).is_none_or(|r| r.area() < 0.1));
                    }
                }
                assert!(
                    regions
                        .iter()
                        .any(|r| r.contains(iced::Point::new(600.0, 80.0)))
                );
                assert!(
                    regions
                        .iter()
                        .any(|r| r.contains(iced::Point::new(100.0, 600.0)))
                );
                assert!(
                    !regions
                        .iter()
                        .any(|r| r.contains(iced::Point::new(400.0, 400.0)))
                );
            }
            assert_eq!(visible_regions(bounds, Coverage::None), vec![bounds]);
        }
    }

    #[test]
    fn cropping_preserves_cover_coordinates_and_reuses_only_visible_pixels() {
        let source = image::Handle::from_rgba(8, 8, (0..256).map(|i| i as u8).collect::<Vec<_>>());
        let bounds = Rectangle {
            x: 10.0,
            y: 20.0,
            width: 16.0,
            height: 8.0,
        };
        let region = Rectangle {
            x: 12.0,
            y: 21.0,
            width: 3.0,
            height: 3.0,
        };
        let tile = crop(&source, bounds, region).unwrap();
        assert_eq!(tile.clip, region);
        assert_eq!(
            tile.bounds,
            Rectangle {
                x: 12.0,
                y: 20.0,
                width: 4.0,
                height: 4.0
            }
        );
        let image::Handle::Rgba {
            width,
            height,
            pixels,
            ..
        } = tile.pixels
        else {
            panic!()
        };
        assert_eq!((width, height), (2, 2));
        assert_eq!(
            pixels.as_ref(),
            &[
                68, 69, 70, 71, 72, 73, 74, 75, 100, 101, 102, 103, 104, 105, 106, 107
            ]
        );
    }
}
