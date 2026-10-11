use super::{app::Message, motion::Tween, palette as p};
use iced::{
    Border, Color, Element, Event, Length, Rectangle, Renderer, Size, Theme, Vector,
    advanced::{
        Clipboard, Layout, Shell, Widget, layout, mouse, overlay, renderer,
        widget::{Operation, Tree, tree},
    },
};
use std::time::{Duration, Instant};

enum Effect {
    Frame(super::frame_art::Frame),
    Hover(bool),
    Press,
    Slide(f32),
}

struct Surface<'a> {
    content: Element<'a, Message>,
    effect: Effect,
}

struct State {
    hover: Tween,
    pressed: bool,
}

pub fn frame<'a>(content: impl Into<Element<'a, Message>>) -> Element<'a, Message> {
    skin(content, super::frame_art::Frame::Panel)
}

pub fn skin<'a>(
    content: impl Into<Element<'a, Message>>,
    frame: super::frame_art::Frame,
) -> Element<'a, Message> {
    Element::new(Surface {
        content: content.into(),
        effect: Effect::Frame(frame),
    })
}

pub fn hover<'a>(content: impl Into<Element<'a, Message>>, enabled: bool) -> Element<'a, Message> {
    Element::new(Surface {
        content: content.into(),
        effect: Effect::Hover(enabled),
    })
}

pub fn press<'a>(content: impl Into<Element<'a, Message>>) -> Element<'a, Message> {
    Element::new(Surface {
        content: content.into(),
        effect: Effect::Press,
    })
}

pub fn slide<'a>(content: impl Into<Element<'a, Message>>, progress: f32) -> Element<'a, Message> {
    Element::new(Surface {
        content: content.into(),
        effect: Effect::Slide(progress),
    })
}

impl Widget<Message, Theme, Renderer> for Surface<'_> {
    fn tag(&self) -> tree::Tag {
        tree::Tag::of::<State>()
    }
    fn state(&self) -> tree::State {
        tree::State::new(State {
            hover: Tween::settled(0.0),
            pressed: false,
        })
    }
    fn children(&self) -> Vec<Tree> {
        vec![Tree::new(&self.content)]
    }
    fn diff(&self, tree: &mut Tree) {
        tree.diff_children(std::slice::from_ref(&self.content));
    }
    fn size(&self) -> Size<Length> {
        self.content.as_widget().size()
    }

    fn layout(
        &mut self,
        tree: &mut Tree,
        renderer: &Renderer,
        limits: &layout::Limits,
    ) -> layout::Node {
        let child = self
            .content
            .as_widget_mut()
            .layout(&mut tree.children[0], renderer, limits);
        let size = child.size();
        layout::Node::with_children(size, vec![child])
    }

    fn operate(
        &mut self,
        tree: &mut Tree,
        layout: Layout<'_>,
        renderer: &Renderer,
        operation: &mut dyn Operation,
    ) {
        if matches!(self.effect, Effect::Slide(p) if p < 0.999) {
            return;
        }
        self.content.as_widget_mut().operate(
            &mut tree.children[0],
            layout.children().next().unwrap(),
            renderer,
            operation,
        );
    }

    fn update(
        &mut self,
        tree: &mut Tree,
        event: &Event,
        layout: Layout<'_>,
        cursor: mouse::Cursor,
        renderer: &Renderer,
        clipboard: &mut dyn Clipboard,
        shell: &mut Shell<'_, Message>,
        viewport: &Rectangle,
    ) {
        if matches!(self.effect, Effect::Hover(_) | Effect::Press) {
            let enabled = !matches!(self.effect, Effect::Hover(false));
            let state = tree.state.downcast_mut::<State>();
            let now = Instant::now();
            let inside = enabled && cursor.is_over(layout.bounds()) && cursor.is_over(*viewport);
            state.hover.target(f32::from(inside), now);
            match event {
                Event::Mouse(mouse::Event::ButtonPressed(mouse::Button::Left)) if inside => {
                    state.pressed = true
                }
                Event::Mouse(mouse::Event::ButtonReleased(mouse::Button::Left)) => {
                    state.pressed = false
                }
                _ if !inside => state.pressed = false,
                _ => {}
            }
            if state.hover.active(now) {
                shell.request_redraw_at(now + Duration::from_millis(16));
            }
        }
        if matches!(self.effect, Effect::Slide(p) if p < 0.999) {
            return;
        }
        let Some(viewport) = layout.bounds().intersection(viewport) else {
            return;
        };
        self.content.as_widget_mut().update(
            &mut tree.children[0],
            event,
            layout.children().next().unwrap(),
            cursor,
            renderer,
            clipboard,
            shell,
            &viewport,
        );
    }

    fn mouse_interaction(
        &self,
        tree: &Tree,
        layout: Layout<'_>,
        cursor: mouse::Cursor,
        viewport: &Rectangle,
        renderer: &Renderer,
    ) -> mouse::Interaction {
        if matches!(self.effect, Effect::Slide(p) if p < 0.999) {
            return mouse::Interaction::None;
        }
        self.content.as_widget().mouse_interaction(
            &tree.children[0],
            layout.children().next().unwrap(),
            cursor,
            viewport,
            renderer,
        )
    }

    fn draw(
        &self,
        tree: &Tree,
        renderer: &mut Renderer,
        theme: &Theme,
        style: &renderer::Style,
        layout: Layout<'_>,
        cursor: mouse::Cursor,
        viewport: &Rectangle,
    ) {
        use iced::advanced::Renderer as _;
        let bounds = layout.bounds();
        let shown = if let Effect::Slide(progress) = self.effect {
            Rectangle {
                height: bounds.height * progress,
                ..bounds
            }
        } else {
            bounds
        };
        let Some(clip) = shown.intersection(viewport) else {
            return;
        };
        let shift = match self.effect {
            Effect::Slide(progress) => (1.0 - progress) * 18.0,
            Effect::Hover(_) | Effect::Press if tree.state.downcast_ref::<State>().pressed => 1.0,
            _ => 0.0,
        };
        renderer.with_layer(clip, |renderer| {
            renderer.with_translation(Vector::new(0.0, shift), |renderer| {
                self.content.as_widget().draw(
                    &tree.children[0],
                    renderer,
                    theme,
                    style,
                    layout.children().next().unwrap(),
                    cursor,
                    &clip,
                );
            });
        });
        // A child can allocate its own layers. Draw decoration in a later layer
        // so opaque button backgrounds cannot cover the frame or hover glow.
        renderer.with_layer(clip, |renderer| match self.effect {
            Effect::Frame(kind) => super::frame_art::draw(renderer, bounds, kind),
            Effect::Press => super::chrome::highlight(
                renderer,
                bounds,
                tree.state
                    .downcast_ref::<State>()
                    .hover
                    .value(Instant::now()),
            ),
            Effect::Hover(_) => {
                let amount = tree
                    .state
                    .downcast_ref::<State>()
                    .hover
                    .value(Instant::now());
                if amount > 0.0 {
                    renderer.fill_quad(
                        renderer::Quad {
                            bounds,
                            border: Border {
                                color: Color {
                                    a: amount * 0.8,
                                    ..p::GOLD
                                },
                                width: 1.0,
                                radius: 3.0.into(),
                            },
                            ..Default::default()
                        },
                        Color {
                            a: amount * 0.07,
                            ..p::GOLD
                        },
                    );
                }
            }
            _ => {}
        });
    }

    fn overlay<'b>(
        &'b mut self,
        tree: &'b mut Tree,
        layout: Layout<'b>,
        renderer: &Renderer,
        viewport: &Rectangle,
        translation: Vector,
    ) -> Option<overlay::Element<'b, Message, Theme, Renderer>> {
        if matches!(self.effect, Effect::Slide(p) if p < 0.999) {
            return None;
        }
        self.content.as_widget_mut().overlay(
            &mut tree.children[0],
            layout.children().next().unwrap(),
            renderer,
            viewport,
            translation,
        )
    }
}
