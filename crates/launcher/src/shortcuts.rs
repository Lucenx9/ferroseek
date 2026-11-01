// SPDX-License-Identifier: GPL-3.0-or-later
//! Capture numbered commands before iced's focused text input inserts their text.

use iced::advanced::layout::{self, Layout};
use iced::advanced::renderer;
use iced::advanced::widget::{Operation, Tree, tree};
use iced::advanced::{Clipboard, Shell, Widget, mouse, overlay};
use iced::{Element, Event, Length, Rectangle, Size, Theme, Vector, keyboard};

use crate::model;
use crate::ui::Message;

pub fn capture_numbered<'a, R: iced::advanced::Renderer + 'a>(
    content: Element<'a, Message, Theme, R>,
) -> Element<'a, Message, Theme, R> {
    Element::new(NumberedShortcuts(content))
}

struct NumberedShortcuts<'a, R>(Element<'a, Message, Theme, R>);

impl<R: iced::advanced::Renderer> Widget<Message, Theme, R> for NumberedShortcuts<'_, R> {
    fn tag(&self) -> tree::Tag {
        self.0.as_widget().tag()
    }

    fn state(&self) -> tree::State {
        self.0.as_widget().state()
    }

    fn children(&self) -> Vec<Tree> {
        self.0.as_widget().children()
    }

    fn diff(&self, tree: &mut Tree) {
        self.0.as_widget().diff(tree);
    }

    fn size(&self) -> Size<Length> {
        self.0.as_widget().size()
    }

    fn size_hint(&self) -> Size<Length> {
        self.0.as_widget().size_hint()
    }

    fn layout(&mut self, tree: &mut Tree, renderer: &R, limits: &layout::Limits) -> layout::Node {
        self.0.as_widget_mut().layout(tree, renderer, limits)
    }

    fn draw(
        &self,
        tree: &Tree,
        renderer: &mut R,
        theme: &Theme,
        style: &renderer::Style,
        layout: Layout<'_>,
        cursor: mouse::Cursor,
        viewport: &Rectangle,
    ) {
        self.0.as_widget().draw(tree, renderer, theme, style, layout, cursor, viewport);
    }

    fn operate(&mut self, tree: &mut Tree, layout: Layout<'_>, renderer: &R, operation: &mut dyn Operation) {
        self.0.as_widget_mut().operate(tree, layout, renderer, operation);
    }

    fn update(
        &mut self,
        tree: &mut Tree,
        event: &Event,
        layout: Layout<'_>,
        cursor: mouse::Cursor,
        renderer: &R,
        clipboard: &mut dyn Clipboard,
        shell: &mut Shell<'_, Message>,
        viewport: &Rectangle,
    ) {
        if let Event::Keyboard(keyboard::Event::KeyPressed { key, physical_key, modifiers, .. }) = event
            && let Some(command) = model::numbered_command(key, *physical_key, *modifiers)
        {
            shell.publish(Message::Cmd(command));
            shell.capture_event();
            return;
        }
        self.0.as_widget_mut().update(tree, event, layout, cursor, renderer, clipboard, shell, viewport);
    }

    fn mouse_interaction(
        &self,
        tree: &Tree,
        layout: Layout<'_>,
        cursor: mouse::Cursor,
        viewport: &Rectangle,
        renderer: &R,
    ) -> mouse::Interaction {
        self.0.as_widget().mouse_interaction(tree, layout, cursor, viewport, renderer)
    }

    fn overlay<'b>(
        &'b mut self,
        tree: &'b mut Tree,
        layout: Layout<'b>,
        renderer: &R,
        viewport: &Rectangle,
        translation: Vector,
    ) -> Option<overlay::Element<'b, Message, Theme, R>> {
        self.0.as_widget_mut().overlay(tree, layout, renderer, viewport, translation)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use iced::keyboard::key::{Code, Physical};
    use iced::keyboard::{Key, Location, Modifiers};
    use iced::widget::text_input;

    use crate::model::Command;

    #[test]
    fn focused_fields_receive_typing_but_never_numbered_shortcut_text() {
        for (value, on_input) in [("report", Message::Query as fn(String) -> Message), ("copy", Message::ActionQuery)] {
            for (symbol, code, mods, expected_slot) in [
                ("1", Code::Digit1, Modifiers::CTRL, Some(0)),
                ("&", Code::Digit1, Modifiers::CTRL, Some(0)),
                ("é", Code::Digit2, Modifiers::CTRL, Some(1)),
                ("8", Code::Digit8, Modifiers::CTRL, Some(7)),
                ("1", Code::Digit1, Modifiers::empty(), None),
                ("1", Code::Digit1, Modifiers::CTRL | Modifiers::ALT, None),
                ("1", Code::Digit1, Modifiers::CTRL | Modifiers::SHIFT, None),
                ("9", Code::Digit9, Modifiers::CTRL, None),
            ] {
                let input: Element<'_, Message, Theme, ()> = text_input("", value).on_input(on_input).into();
                let mut input = capture_numbered(input);
                let widget = input.as_widget_mut();
                let mut tree = Tree::new(&*widget);
                tree.state.downcast_mut::<text_input::State<()>>().focus();
                let node = widget.layout(&mut tree, &(), &layout::Limits::new(Size::ZERO, Size::new(400.0, 60.0)));
                let layout = Layout::new(&node);
                let event = Event::Keyboard(keyboard::Event::KeyPressed {
                    key: Key::Character(symbol.into()),
                    modified_key: Key::Character(symbol.into()),
                    physical_key: Physical::Code(code),
                    modifiers: mods,
                    text: Some(symbol.into()),
                    location: Location::Standard,
                    repeat: false,
                });
                let mut messages = Vec::new();
                let status = {
                    let mut shell = Shell::new(&mut messages);
                    widget.update(
                        &mut tree,
                        &event,
                        layout,
                        mouse::Cursor::Unavailable,
                        &(),
                        &mut iced::advanced::clipboard::Null,
                        &mut shell,
                        &Rectangle::with_size(Size::new(400.0, 60.0)),
                    );
                    shell.event_status()
                };
                assert_eq!(status, iced::event::Status::Captured);
                assert_eq!(messages.len(), 1);
                match expected_slot {
                    Some(slot) => {
                        assert!(matches!(messages[0], Message::Cmd(Command::ActivateVisible(i)) if i == slot))
                    }
                    None => match &messages[0] {
                        Message::Query(text) | Message::ActionQuery(text) => {
                            assert_eq!(text, &format!("{symbol}{value}"));
                        }
                        other => panic!("expected ordinary text input, got {other:?}"),
                    },
                }
            }
        }
    }
}
