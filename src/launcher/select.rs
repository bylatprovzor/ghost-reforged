use super::{
    app::Message,
    art::{self, Icon},
    effects,
    motion::Menu,
    palette as p,
};
use iced::{
    Element, Event, Fill, Length, Point, Rectangle, Renderer, Size, Theme, Vector,
    advanced::{
        Clipboard, Layout, Shell, Widget, layout, mouse, overlay, renderer,
        widget::{Operation, Tree, tree},
    },
    widget::{button, overlay::menu, row, space, text},
};

#[derive(Clone)]
struct Choice {
    label: String,
    message: Message,
}
impl std::fmt::Display for Choice {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.label)
    }
}

pub fn select<'a>(
    app: &super::app::App,
    id: Menu,
    selected: String,
    choices: impl IntoIterator<Item = (String, Message)>,
) -> Element<'a, Message> {
    let tween = &app.motion.menus[id.index()];
    let content = effects::skin(
        effects::hover(
            button(
                row![
                    text(selected).size(15),
                    space().width(Fill),
                    art::icon(Icon::Chevron, 16.0)
                ]
                .align_y(iced::Alignment::Center),
            )
            .padding([8, 14])
            .width(Fill)
            .style(p::inset)
            .on_press(Message::Menu(id)),
            true,
        ),
        super::frame_art::Frame::Input,
    );
    Element::new(Select {
        content,
        choices: choices
            .into_iter()
            .map(|(label, message)| Choice { label, message })
            .collect(),
        progress: tween.value(app.motion.now),
        open: tween.is_open(),
        class: Box::new(menu_style),
    })
}

fn menu_style(_: &Theme) -> menu::Style {
    menu::Style {
        background: p::BG.into(),
        text_color: p::TEXT,
        border: iced::Border {
            color: p::GOLD.scale_alpha(0.65),
            width: 1.0,
            radius: 3.0.into(),
        },
        selected_text_color: p::GOLD,
        selected_background: iced::Color::from_rgb8(22, 51, 60).into(),
        shadow: iced::Shadow::default(),
    }
}

struct Select<'a> {
    content: Element<'a, Message>,
    choices: Vec<Choice>,
    progress: f32,
    open: bool,
    class: <Theme as menu::Catalog>::Class<'a>,
}

#[derive(Default)]
struct State {
    menu: menu::State,
    hovered: Option<usize>,
}

impl Widget<Message, Theme, Renderer> for Select<'_> {
    fn tag(&self) -> tree::Tag {
        tree::Tag::of::<State>()
    }
    fn state(&self) -> tree::State {
        tree::State::new(State::default())
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
        self.content
            .as_widget_mut()
            .layout(&mut tree.children[0], renderer, limits)
    }
    fn operate(
        &mut self,
        tree: &mut Tree,
        layout: Layout<'_>,
        renderer: &Renderer,
        operation: &mut dyn Operation,
    ) {
        self.content
            .as_widget_mut()
            .operate(&mut tree.children[0], layout, renderer, operation);
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
        self.content.as_widget_mut().update(
            &mut tree.children[0],
            event,
            layout,
            cursor,
            renderer,
            clipboard,
            shell,
            viewport,
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
        self.content.as_widget().mouse_interaction(
            &tree.children[0],
            layout,
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
        self.content.as_widget().draw(
            &tree.children[0],
            renderer,
            theme,
            style,
            layout,
            cursor,
            viewport,
        );
    }
    fn overlay<'b>(
        &'b mut self,
        tree: &'b mut Tree,
        layout: Layout<'b>,
        _: &Renderer,
        viewport: &Rectangle,
        translation: Vector,
    ) -> Option<overlay::Element<'b, Message, Theme, Renderer>> {
        if !self.open && self.progress <= 0.0 {
            return None;
        }
        let state = tree.state.downcast_mut::<State>();
        let bounds = layout.bounds();
        let width = bounds.width.max(180.0).min(viewport.width);
        let x = (bounds.x + translation.x).clamp(viewport.x, viewport.x + viewport.width - width);
        let inner = menu::Menu::new(
            &mut state.menu,
            &self.choices,
            &mut state.hovered,
            |choice| choice.message,
            None,
            &self.class,
        )
        .width(width)
        .padding(11)
        .text_size(15)
        .overlay(
            Point::new(x, bounds.y + translation.y),
            *viewport,
            bounds.height,
            Length::Shrink,
        );
        Some(overlay::Element::new(Box::new(AnimatedMenu {
            inner,
            progress: self.progress,
            open: self.open,
        })))
    }
}

struct AnimatedMenu<'a> {
    inner: overlay::Element<'a, Message, Theme, Renderer>,
    progress: f32,
    open: bool,
}

impl overlay::Overlay<Message, Theme, Renderer> for AnimatedMenu<'_> {
    fn layout(&mut self, renderer: &Renderer, bounds: Size) -> layout::Node {
        self.inner.as_overlay_mut().layout(renderer, bounds)
    }
    fn draw(
        &self,
        renderer: &mut Renderer,
        theme: &Theme,
        style: &renderer::Style,
        layout: Layout<'_>,
        cursor: mouse::Cursor,
    ) {
        use iced::advanced::Renderer as _;
        let bounds = layout.bounds();
        let clip = Rectangle {
            height: bounds.height * self.progress,
            ..bounds
        };
        if clip.height > 0.0 {
            renderer.with_layer(clip, |renderer| {
                self.inner
                    .as_overlay()
                    .draw(renderer, theme, style, layout, cursor)
            });
        }
    }
    fn update(
        &mut self,
        event: &Event,
        layout: Layout<'_>,
        cursor: mouse::Cursor,
        renderer: &Renderer,
        clipboard: &mut dyn Clipboard,
        shell: &mut Shell<'_, Message>,
    ) {
        let dismiss = matches!(
            event,
            Event::Keyboard(iced::keyboard::Event::KeyPressed {
                key: iced::keyboard::Key::Named(iced::keyboard::key::Named::Escape),
                ..
            })
        ) || (matches!(
            event,
            Event::Mouse(mouse::Event::ButtonPressed(mouse::Button::Left))
        ) && !cursor.is_over(layout.bounds()));
        if dismiss {
            shell.publish(Message::DismissMenus);
            shell.capture_event();
        } else if self.open && self.progress >= 0.999 {
            self.inner
                .as_overlay_mut()
                .update(event, layout, cursor, renderer, clipboard, shell);
        } else if matches!(event, Event::Mouse(_) | Event::Keyboard(_)) {
            shell.capture_event();
        }
    }
    fn mouse_interaction(
        &self,
        layout: Layout<'_>,
        cursor: mouse::Cursor,
        renderer: &Renderer,
    ) -> mouse::Interaction {
        self.inner
            .as_overlay()
            .mouse_interaction(layout, cursor, renderer)
    }
}
