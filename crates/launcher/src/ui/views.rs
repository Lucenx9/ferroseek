// SPDX-License-Identifier: GPL-3.0-or-later
//! Secondary-view transitions and replies; input routing stays in the parent UI.

use super::{Launcher, Message, SearchStatusPanel, search_id, update};
use crate::backend::SearchStatus;
use crate::model::ClickTracker;
use crate::{appearance, details, folders, open_with, preview, timing};
use iced::Task;

impl Launcher {
    pub(super) fn preview_loaded(
        &mut self,
        generation: u64,
        result: Result<crate::backend::PreviewData, String>,
    ) -> Task<Message> {
        if generation == self.preview_generation
            && let Some(panel) = &mut self.text_preview
        {
            panel.loaded(result);
            self.preview_request = None;
            timing::event("preview_loaded");
        }
        Task::none()
    }

    pub(super) fn preview_event(&mut self, generation: u64, event: preview::Event) -> Task<Message> {
        if generation != self.preview_generation {
            return Task::none();
        }
        if !self.preview_visible() {
            return Task::none();
        }
        if matches!(event, preview::Event::Back) {
            return self.preview_effect(preview::Effect::Close);
        }
        let Some(panel) = &mut self.text_preview else { return Task::none() };
        let effect = panel.event(event);
        self.preview_effect(effect)
    }

    pub(super) fn details_loaded(
        &mut self,
        generation: u64,
        result: Result<crate::backend::FileDetails, String>,
    ) -> Task<Message> {
        if generation == self.details_generation
            && let Some(panel) = &mut self.file_details
        {
            panel.loaded(result);
        }
        Task::none()
    }

    pub(super) fn details_event(&mut self, generation: u64, event: details::Event) -> Task<Message> {
        if generation != self.details_generation {
            return Task::none();
        }
        let Some(panel) = &mut self.file_details else { return Task::none() };
        let effect = panel.event(event);
        self.details_effect(effect)
    }

    pub(super) fn applications_loaded(
        &mut self,
        generation: u64,
        result: Result<Vec<crate::backend::ApplicationChoice>, String>,
    ) -> Task<Message> {
        if generation == self.application_generation
            && let Some(panel) = &mut self.application_picker
        {
            panel.loaded(result, &mut self.icons);
            timing::event("applications_loaded");
            return Task::batch([self.resize(), crate::input::focus(open_with::input_id())]);
        }
        Task::none()
    }

    pub(super) fn open_with_event(&mut self, generation: u64, event: open_with::Event) -> Task<Message> {
        if generation != self.application_generation {
            return Task::none();
        }
        if self.opening.is_some() && !matches!(event, open_with::Event::Back) {
            return Task::none();
        }
        let Some(panel) = &mut self.application_picker else { return Task::none() };
        if matches!(event, open_with::Event::Query(_)) {
            timing::event("application_input");
        }
        let effect = panel.event(event);
        self.application_effect(effect)
    }

    pub(super) fn toggle_folders(&mut self) -> Task<Message> {
        if let Some(panel) = &mut self.folder_settings {
            let (task, effect) = panel.update(folders::Event::Back, self.backend.clone());
            self.finish_folders(task, effect)
        } else if self.opening.is_some() {
            Task::none()
        } else {
            self.show_folders(false)
        }
    }

    pub(super) fn folder_shortcut(&mut self, shortcut: folders::Shortcut) -> Task<Message> {
        let Some(panel) = &mut self.folder_settings else { return Task::none() };
        let (task, effect) = panel.shortcut(shortcut, self.backend.clone());
        self.finish_folders(task, effect)
    }

    pub(super) fn folder_event(&mut self, generation: u64, event: folders::Event) -> Task<Message> {
        if generation != self.folder_generation {
            return Task::none();
        }
        let Some(panel) = &mut self.folder_settings else { return Task::none() };
        let (task, effect) = panel.update(event, self.backend.clone());
        self.finish_folders(task, effect)
    }

    pub(super) fn toggle_search_status(&mut self) -> Task<Message> {
        if self.search_status.is_some() {
            return update(self, Message::CloseSearchStatus);
        }
        if self.opening.is_some() {
            return Task::none();
        }
        self.clicks = ClickTracker::default();
        self.panel = None;
        let load = self.refresh_search_status();
        Task::batch([load, self.resize()])
    }

    pub(super) fn close_search_status(&mut self) -> Task<Message> {
        self.search_status = None;
        Task::batch([self.resize(), crate::input::focus(search_id())])
    }

    pub(super) fn search_status_loaded(
        &mut self,
        generation: u64,
        result: Result<SearchStatus, String>,
    ) -> Task<Message> {
        if generation == self.status_generation && self.search_status.is_some() {
            self.search_status = Some(match result {
                Ok(status) => SearchStatusPanel::Ready(status),
                Err(error) => SearchStatusPanel::Error(error),
            });
        }
        Task::none()
    }

    pub(super) fn show_appearance(&mut self) -> Task<Message> {
        self.panel = None;
        self.search_status = None;
        self.clicks = ClickTracker::default();
        self.appearance_generation += 1;
        let generation = self.appearance_generation;
        let panel = appearance::Panel::new(self.settings_path.clone(), self.preferences);
        let load = panel.load().map(move |event| Message::Appearance { generation, event });
        self.appearance = Some(panel);
        timing::event("appearance_opened");
        Task::batch([load, self.resize()])
    }
}
