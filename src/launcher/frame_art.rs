use iced::{
    Rectangle, Renderer,
    advanced::image::{self, Renderer as _},
};
use std::sync::OnceLock;

#[derive(Clone, Copy)]
pub enum Frame {
    Panel,
    Button,
    Navigation,
    SelectedNavigation,
    Header,
    Input,
}

struct Slices {
    images: Vec<image::Handle>,
    edges: [f32; 4],
}

fn slices(kind: Frame) -> &'static Slices {
    static CACHE: OnceLock<Vec<Slices>> = OnceLock::new();
    &CACHE.get_or_init(|| {
        [
            ([274, 208, 681, 822], [25, 25, 25, 25]),
            ([803, 338, 112, 47], [12, 10, 12, 10]),
            ([26, 405, 228, 76], [16, 13, 16, 13]),
            ([26, 235, 228, 86], [16, 13, 16, 13]),
            ([24, 45, 487, 117], [37, 18, 37, 18]),
            ([306, 517, 610, 45], [7, 6, 7, 6]),
        ]
        .into_iter()
        .map(|([x, y, w, h], [l, t, r, b])| {
            let xs = [x, x + l, x + w - r, x + w];
            let ys = [y, y + t, y + h - b, y + h];
            let mut images = Vec::with_capacity(8);
            for row in 0..3 {
                for column in 0..3 {
                    if row == 1 && column == 1 {
                        continue;
                    }
                    images.push(super::art::crop(
                        xs[column],
                        ys[row],
                        xs[column + 1] - xs[column],
                        ys[row + 1] - ys[row],
                    ));
                }
            }
            Slices {
                images,
                edges: [
                    l as f32 * 0.8,
                    t as f32 * 0.8,
                    r as f32 * 0.8,
                    b as f32 * 0.8,
                ],
            }
        })
        .collect()
    })[kind as usize]
}

pub fn draw(renderer: &mut Renderer, bounds: Rectangle, kind: Frame) {
    let skin = slices(kind);
    let [l, t, r, b] = skin.edges;
    let scale = (bounds.width / (l + r))
        .min(bounds.height / (t + b))
        .min(1.0);
    let xs = [
        bounds.x,
        bounds.x + l * scale,
        bounds.x + bounds.width - r * scale,
        bounds.x + bounds.width,
    ];
    let ys = [
        bounds.y,
        bounds.y + t * scale,
        bounds.y + bounds.height - b * scale,
        bounds.y + bounds.height,
    ];
    let mut index = 0;
    for row in 0..3 {
        for column in 0..3 {
            if row == 1 && column == 1 {
                continue;
            }
            let rect = Rectangle {
                x: xs[column],
                y: ys[row],
                width: xs[column + 1] - xs[column],
                height: ys[row + 1] - ys[row],
            };
            if rect.width >= 1.0 && rect.height >= 1.0 {
                renderer.draw_image(
                    image::Image::new(skin.images[index].clone()).snap(true),
                    rect,
                    bounds,
                );
            }
            index += 1;
        }
    }
}
