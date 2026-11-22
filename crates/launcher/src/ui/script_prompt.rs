// SPDX-License-Identifier: GPL-3.0-or-later
//! Collect literal argument values for one invocation.

use iced::advanced::widget::operation::{Operation, Outcome, scrollable};
use iced::widget::{Id, column, container, text, text_input};
use iced::{Element, Rectangle, Task, Theme};

use super::{Message, field_style, palette, scripts::Event};
use crate::input::FocusCycle;

pub(super) struct Prompt {
    inputs: Vec<Id>,
    rows: Vec<Id>,
    values: Vec<String>,
    focus: usize,
    missing: Option<usize>,
}

impl Prompt {
    pub fn new(count: usize) -> Self {
        Self {
            inputs: (0..count).map(|_| Id::unique()).collect(),
            rows: (0..count).map(|_| Id::unique()).collect(),
            values: vec![String::new(); count],
            focus: 0,
            missing: None,
        }
    }

    pub fn context(&self) -> Option<Id> {
        self.inputs.first().cloned()
    }

    pub fn focus_target(&self) -> Option<Id> {
        self.inputs.get(self.focus).cloned()
    }

    pub fn focus_cycle(&self, generation: u64) -> Option<FocusCycle> {
        (!self.inputs.is_empty()).then(|| FocusCycle {
            fields: self
                .inputs
                .iter()
                .enumerate()
                .map(|(index, id)| (id.clone(), Message::Script { generation, event: Event::Focus(index) }))
                .collect(),
            tab_only: false,
        })
    }

    pub fn change(&mut self, index: usize, value: String) {
        if let Some(input) = self.values.get_mut(index) {
            *input = value;
            self.focus = index;
            self.missing = None;
        }
    }

    pub fn take_arguments(&mut self) -> Result<Vec<String>, usize> {
        self.missing = self.values.iter().position(|value| value.trim().is_empty());
        match self.missing {
            Some(index) => Err(index),
            None => Ok(std::mem::take(&mut self.values)),
        }
    }

    pub fn focus(&mut self, index: usize, scroll: &Id, viewport: &Id) -> Task<Message> {
        let Some(id) = self.inputs.get(index).cloned() else { return Task::none() };
        self.focus = index;
        let reveal = RevealField {
            scroll: scroll.clone(),
            viewport: viewport.clone(),
            row: self.rows[index].clone(),
            viewport_bounds: None,
            row_bounds: None,
        };
        Task::batch([crate::input::focus(id), iced_runtime::task::effect(iced_runtime::Action::widget(reveal))])
    }

    pub fn view<'a>(&'a self, labels: &'a [String], generation: u64) -> Element<'a, Message> {
        let mut fields = column![].spacing(12);
        for (index, (label, id)) in labels.iter().zip(&self.inputs).enumerate() {
            let input = text_input("", &self.values[index])
                .id(id.clone())
                .padding([7, 8])
                .on_input(move |value| Message::Script { generation, event: Event::Input(index, value) })
                .style(field_style);
            let mut field = column![text(label.trim()).size(13), input].spacing(4);
            if self.missing == Some(index) {
                field = field.push(
                    text("Enter a value.")
                        .size(12)
                        .style(|theme: &Theme| text::Style { color: Some(palette::ERROR.resolve(theme)) }),
                );
            }
            fields = fields.push(container(field).id(self.rows[index].clone()));
        }
        fields.into()
    }
}

struct RevealField {
    scroll: Id,
    viewport: Id,
    row: Id,
    viewport_bounds: Option<Rectangle>,
    row_bounds: Option<Rectangle>,
}

impl<T: 'static> Operation<T> for RevealField {
    fn traverse(&mut self, operate: &mut dyn FnMut(&mut dyn Operation<T>)) {
        operate(self);
    }

    fn container(&mut self, id: Option<&Id>, bounds: Rectangle) {
        if id == Some(&self.row) {
            self.row_bounds = Some(bounds);
        } else if id == Some(&self.viewport) {
            self.viewport_bounds = Some(bounds);
        }
    }

    fn finish(&self) -> Outcome<T> {
        let (Some(viewport), Some(row)) = (self.viewport_bounds, self.row_bounds) else {
            return Outcome::None;
        };
        // Layout bounds are unscrolled. Bring the whole row, including its
        // validation message, above the bottom edge of the viewport.
        let desired = if row.height > viewport.height {
            row.y - viewport.y
        } else {
            row.y + row.height - viewport.y - viewport.height + 4.0
        };
        Outcome::Chain(Box::new(scrollable::scroll_to(
            self.scroll.clone(),
            scrollable::AbsoluteOffset { x: None, y: Some(desired.max(0.0)) },
        )))
    }
}
