// SPDX-License-Identifier: GPL-3.0-or-later
//! Preserve input order across model changes and focus the current field in-tree.

use iced::advanced::layout::{self, Layout};
use iced::advanced::renderer;
use iced::advanced::widget::{Operation, Tree, tree};
use iced::advanced::{Clipboard, Shell, Widget, mouse, overlay};
use iced::{Element, Event, Length, Rectangle, Size, Theme, Vector, keyboard};

use std::collections::VecDeque;

use crate::shortcuts::Routing;
use crate::ui;
use crate::ui::Message;

pub fn capture<'a, R: iced::advanced::Renderer + 'a>(
    content: Element<'a, Message, Theme, R>,
    epoch: u64,
    focus: Option<iced::widget::Id>,
    context: Option<iced::widget::Id>,
    focus_cycle: Option<FocusCycle>,
    shortcuts: Routing,
) -> Element<'a, Message, Theme, R> {
    Element::new(Input { content, epoch, focus, context, focus_cycle, shortcuts })
}

struct Input<'a, R> {
    content: Element<'a, Message, Theme, R>,
    epoch: u64,
    focus: Option<iced::widget::Id>,
    context: Option<iced::widget::Id>,
    focus_cycle: Option<FocusCycle>,
    shortcuts: Routing,
}

struct State {
    epoch: u64,
    focus: Option<iced::widget::Id>,
    context: Option<iced::widget::Id>,
    needs_focus: bool,
    window_focused: bool,
    blocked: bool,
    pending: VecDeque<(Event, mouse::Cursor)>,
}

struct EnsureFocus {
    target: iced::widget::Id,
    found: bool,
    allowed: bool,
}

pub fn focus<T>(target: iced::widget::Id) -> iced::Task<T> {
    iced_runtime::task::effect(iced_runtime::Action::widget(EnsureFocus { target, found: false, allowed: true }))
}

impl<T> Operation<T> for EnsureFocus {
    fn focusable(
        &mut self,
        id: Option<&iced::widget::Id>,
        _: Rectangle,
        state: &mut dyn iced::advanced::widget::operation::focusable::Focusable,
    ) {
        if !self.allowed {
            return;
        }
        if id == Some(&self.target) {
            if !state.is_focused() {
                state.focus();
            }
            self.found = state.is_focused();
        } else {
            state.unfocus();
        }
    }
    fn custom(&mut self, _: Option<&iced::widget::Id>, _: Rectangle, state: &mut dyn std::any::Any) {
        if let Some(state) = state.downcast_mut::<State>() {
            self.allowed = state.focus.as_ref() == Some(&self.target);
        }
    }
    fn traverse(&mut self, operate: &mut dyn FnMut(&mut dyn Operation<T>)) {
        operate(self);
    }
}

#[derive(Default)]
struct CurrentFocus(Option<iced::widget::Id>);

impl Operation for CurrentFocus {
    fn focusable(
        &mut self,
        id: Option<&iced::widget::Id>,
        _: Rectangle,
        state: &mut dyn iced::advanced::widget::operation::focusable::Focusable,
    ) {
        if state.is_focused() {
            self.0 = id.cloned();
        }
    }

    fn traverse(&mut self, operate: &mut dyn FnMut(&mut dyn Operation)) {
        operate(self);
    }
}

pub struct FocusCycle(pub [(iced::widget::Id, Message); 3]);

impl FocusCycle {
    fn command<R: iced::advanced::Renderer>(
        &self,
        message: Option<Message>,
        content: &mut dyn Widget<Message, Theme, R>,
        child: &mut Tree,
        layout: Layout<'_>,
        renderer: &R,
    ) -> Option<Message> {
        use crate::model::Command;

        let step = match message {
            Some(Message::Cmd(Command::Up)) => self.0.len() - 1,
            Some(Message::Cmd(Command::Down)) => 1,
            _ => return message,
        };
        let mut current = CurrentFocus::default();
        content.operate(child, layout, renderer, &mut current);
        let index = self.0.iter().position(|(id, _)| Some(id) == current.0.as_ref());
        let next = index.map_or(0, |index| (index + step) % self.0.len());
        let (id, message) = &self.0[next];
        // Move the native caret before releasing subsequent buffered text.
        // The message records the field and keeps it visible on short outputs.
        let mut focus = EnsureFocus { target: id.clone(), found: false, allowed: true };
        content.operate(child, layout, renderer, &mut focus);
        Some(message.clone())
    }
}

impl<R: iced::advanced::Renderer> Input<'_, R> {
    fn restore_focus(&mut self, child: &mut Tree, layout: Layout<'_>, renderer: &R, shell: &mut Shell<'_, Message>) {
        let Some(target) = &self.focus else { return };
        if self.focus_cycle.is_some() {
            let mut current = CurrentFocus::default();
            self.content.as_widget_mut().operate(child, layout, renderer, &mut current);
            if current.0.is_some() {
                return;
            }
        }
        let mut operation = EnsureFocus { target: target.clone(), found: false, allowed: true };
        self.content.as_widget_mut().operate(child, layout, renderer, &mut operation);
        if operation.found && crate::timing::enabled() {
            shell.publish(Message::InputFocused {
                epoch: self.epoch,
                field: target.clone(),
                at: std::time::Instant::now(),
            });
        }
    }
}

impl<R: iced::advanced::Renderer> Widget<Message, Theme, R> for Input<'_, R> {
    fn tag(&self) -> tree::Tag {
        tree::Tag::of::<State>()
    }

    fn state(&self) -> tree::State {
        tree::State::new(State {
            epoch: self.epoch,
            focus: self.focus.clone(),
            context: self.context.clone(),
            needs_focus: true,
            window_focused: true,
            blocked: false,
            pending: VecDeque::new(),
        })
    }

    fn children(&self) -> Vec<Tree> {
        vec![Tree::new(&self.content)]
    }

    fn diff(&self, tree: &mut Tree) {
        let state = tree.state.downcast_mut::<State>();
        if state.context != self.context {
            // A held button must not transfer its press to a different view.
            state.context = self.context.clone();
            tree.children = vec![Tree::new(&self.content)];
        }
        tree.diff_children(&[&self.content]);
        let state = tree.state.downcast_mut::<State>();
        if state.epoch != self.epoch {
            state.epoch = self.epoch;
            state.blocked = false;
            state.needs_focus = true;
        }
        if state.focus != self.focus {
            state.focus = self.focus.clone();
            state.needs_focus = true;
        }
    }

    fn size(&self) -> Size<Length> {
        self.content.as_widget().size()
    }

    fn size_hint(&self) -> Size<Length> {
        self.content.as_widget().size_hint()
    }

    fn layout(&mut self, tree: &mut Tree, renderer: &R, limits: &layout::Limits) -> layout::Node {
        self.content.as_widget_mut().layout(&mut tree.children[0], renderer, limits)
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
        self.content.as_widget().draw(&tree.children[0], renderer, theme, style, layout, cursor, viewport);
    }

    fn operate(&mut self, tree: &mut Tree, layout: Layout<'_>, renderer: &R, operation: &mut dyn Operation) {
        operation.custom(None, layout.bounds(), tree.state.downcast_mut::<State>());
        self.content.as_widget_mut().operate(&mut tree.children[0], layout, renderer, operation);
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
        let state = tree.state.downcast_mut::<State>();
        let child = &mut tree.children[0];
        match event {
            Event::Window(iced::window::Event::Unfocused) => {
                state.pending.clear();
                state.window_focused = false;
            }
            Event::Window(iced::window::Event::Focused) => {
                state.window_focused = true;
                state.needs_focus = true;
            }
            _ => {}
        }
        if !state.blocked && state.needs_focus && state.window_focused {
            self.restore_focus(child, layout, renderer, shell);
            state.needs_focus = false;
        }
        let mut dispatch = |event: &Event, child: &mut Tree, shell: &mut Shell<'_, Message>, cursor: mouse::Cursor| {
            let mut messages = Vec::new();
            let mut inner = Shell::new(&mut messages);
            // A printable global shortcut must not edit the focused text field.
            let shortcut = self.shortcuts.message(event);
            if shortcut.is_none() {
                self.content
                    .as_widget_mut()
                    .update(child, event, layout, cursor, renderer, clipboard, &mut inner, viewport);
            }
            let mut command = shortcut.or_else(|| ui::input_event(event, inner.event_status()));
            if let Some(cycle) = &self.focus_cycle {
                command = cycle.command(command, self.content.as_widget_mut(), child, layout, renderer);
            }
            let numbered = matches!(event, Event::Keyboard(keyboard::Event::KeyPressed { key, physical_key, modifiers, .. })
                if crate::model::numbered_command(key, *physical_key, *modifiers).is_some() && inner.is_event_captured());
            let pause = command.is_some()
                || numbered
                || (!inner.is_empty() && matches!(event, Event::Mouse(_) | Event::Touch(_)));
            shell.merge(inner, |message| message);
            if let Some(command) = command {
                shell.publish(command);
                shell.capture_event();
            }
            if pause {
                // The next view acknowledges all messages from this event.
                // Keep later input on this tree until its context is rebuilt.
                shell.publish(Message::InputApplied);
                shell.request_redraw();
            }
            pause
        };
        if !state.blocked && state.window_focused {
            while let Some((pending, observed_cursor)) = state.pending.pop_front() {
                if dispatch(&pending, child, shell, observed_cursor) {
                    state.blocked = true;
                    break;
                }
            }
        }
        if state.blocked && !matches!(event, Event::Window(_)) {
            state.pending.push_back((event.clone(), cursor));
            shell.capture_event();
        } else {
            state.blocked |= dispatch(event, child, shell, cursor);
        }
    }

    fn mouse_interaction(
        &self,
        tree: &Tree,
        layout: Layout<'_>,
        cursor: mouse::Cursor,
        viewport: &Rectangle,
        renderer: &R,
    ) -> mouse::Interaction {
        self.content.as_widget().mouse_interaction(&tree.children[0], layout, cursor, viewport, renderer)
    }

    fn overlay<'b>(
        &'b mut self,
        tree: &'b mut Tree,
        layout: Layout<'b>,
        renderer: &R,
        viewport: &Rectangle,
        translation: Vector,
    ) -> Option<overlay::Element<'b, Message, Theme, R>> {
        self.content.as_widget_mut().overlay(&mut tree.children[0], layout, renderer, viewport, translation)
    }
}
