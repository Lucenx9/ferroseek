// SPDX-License-Identifier: GPL-3.0-or-later
//! Secondary-view presentation, transitions and replies.

use super::{
    Launcher, Message, SearchStatusPanel, Shortcut, actions_id, hint, page_header, search_id, search_status_view,
    settings, status_button_style, update,
};
use crate::backend::{CommandRequest, SearchStatus};
use crate::commands::Builtin;
use crate::model::ClickTracker;
use crate::shortcuts::Routing;
use crate::{appearance, details, folders, open_with, preview, timing};
use iced::alignment::Vertical;
use iced::widget::{button, container, responsive, row, space, text};
use iced::{Element, Length, Task};

pub(crate) fn back_refresh_header<'a, M: Clone + 'a>(
    title: &'static str,
    back: M,
    refresh: Option<M>,
) -> Element<'a, M> {
    page_header(responsive(move |size| {
        let compact = size.width < 320.0;
        let back = button(text("Back").size(12)).on_press(back.clone()).padding([6, 8]).style(status_button_style);
        let label = if compact { text("Refresh").size(12).into() } else { hint("Ctrl+R", "Refresh") };
        let refresh = button(label).on_press_maybe(refresh.clone()).padding([6, 8]).style(status_button_style);
        let heading = row![back, text(title).size(20), space::horizontal(), refresh]
            .spacing(if compact { 6 } else { 12 })
            .align_y(Vertical::Center);
        container(heading).center_y(Length::Fill).into()
    }))
    .into()
}

impl Launcher {
    pub(super) fn input_focus(&self) -> Option<iced::widget::Id> {
        if let Some(panel) = &self.window_switcher.panel {
            return Some(panel.input());
        }
        if let Some(panel) = &self.clipboard_history.panel {
            return panel.focus();
        }
        if self.search_suspended() {
            return self.secondary_focus();
        }
        Some(if self.panel.is_some() { actions_id() } else { search_id() })
    }

    fn secondary_focus(&self) -> Option<iced::widget::Id> {
        if let Some(editor) = &self.alias_editor {
            editor.focus_target()
        } else if let Some(editor) = &self.saved_editor {
            editor.focus_target()
        } else if self.folder_settings.is_some() {
            Some(folders::input_id())
        } else if self.application_picker.is_some() {
            Some(open_with::input_id())
        } else {
            None
        }
    }

    pub(super) fn dialog_height(&self) -> Option<u32> {
        if self.window_switcher.panel.is_some() {
            Some(super::windows::HEIGHT)
        } else if let Some(panel) = &self.clipboard_history.panel {
            Some(panel.height())
        } else if self.alias_editor.is_some() {
            Some(super::alias_editor::HEIGHT)
        } else if self.shortcuts.panel.is_some() || self.global_shortcuts.panel.is_some() {
            Some(settings::SHORTCUTS_HEIGHT)
        } else if self.settings_menu_open() {
            Some(settings::HEIGHT)
        } else {
            self.saved_editor.as_ref().and_then(super::saved_editor::Editor::dialog_height)
        }
    }

    pub(super) fn shortcut_routing(&self) -> Routing {
        if self.clipboard_history.panel.is_some() {
            return Routing::Clipboard;
        }
        if let Some(panel) = &self.shortcuts.panel {
            return if panel.recording() { Routing::Recording } else { Routing::Disabled };
        }
        if self.overrides_launcher_shortcuts() {
            return Routing::Disabled;
        }
        let shortcut = if self.folder_settings.is_some() {
            Some(Shortcut::SearchFolders)
        } else if self.appearance.is_some() {
            Some(Shortcut::Appearance)
        } else if self.window_switcher.panel.is_some() {
            Some(Shortcut::Actions)
        } else {
            None
        };
        shortcut.map_or(Routing::Commands(self.shortcuts.bindings), |action| {
            Routing::Only(self.shortcuts.bindings.get(action), action)
        })
    }

    fn overrides_launcher_shortcuts(&self) -> bool {
        self.application_picker.is_some() || self.global_shortcuts.panel.is_some() || self.saved_editor_open()
    }

    pub(super) fn input_context(&self) -> Option<iced::widget::Id> {
        self.window_switcher
            .panel
            .as_ref()
            .map(super::windows::Panel::input)
            .or_else(|| self.clipboard_history.panel.as_ref().map(super::clipboard_view::Panel::context))
            .or_else(|| self.alias_editor.as_ref().map(super::alias_editor::Editor::context))
            .or_else(|| self.saved_editor.as_ref().map(super::saved_editor::Editor::context))
            .or_else(|| self.appearance.as_ref().map(appearance::Panel::content_id))
    }

    fn editor_view(&self) -> Option<Element<'_, Message>> {
        if let Some(panel) = &self.window_switcher.panel {
            let key = self.shortcuts.bindings.get(Shortcut::Actions).to_string();
            return Some(panel.view(self.window_switcher.generation, key));
        }
        if let Some(panel) = &self.clipboard_history.panel {
            return Some(self.clipboard_view(panel));
        }
        if let Some(panel) = &self.shortcuts.panel {
            return Some(self.shortcut_settings_view(panel));
        }
        if let Some(panel) = &self.global_shortcuts.panel {
            return Some(self.global_shortcut_view(panel));
        }
        if let Some(editor) = &self.alias_editor {
            return Some(editor.view());
        }
        if let Some(editor) = &self.saved_editor {
            return Some(self.saved_editor_view(editor));
        }
        None
    }

    pub(super) fn secondary_view(&self) -> Option<Element<'_, Message>> {
        if let Some(editor) = self.editor_view() {
            return Some(editor);
        }
        if let Some(panel) = &self.appearance {
            let generation = self.appearance_generation;
            return Some(
                panel
                    .view(self.system_dark, self.opaque, self.glass)
                    .map(move |event| Message::Appearance { generation, event }),
            );
        }
        if let Some(panel) = &self.file_details {
            let generation = self.details_generation;
            return Some(panel.view().map(move |event| Message::Details { generation, event }));
        }
        if let Some(panel) = &self.application_picker {
            return Some(self.open_with_view(panel));
        }
        if let Some(panel) = &self.folder_settings {
            let generation = self.folder_generation;
            return Some(panel.view().map(move |event| Message::Folder { generation, event }));
        }
        if let Some(status) = &self.search_status {
            return Some(search_status_view(status));
        }
        self.settings.map(|entry| settings::view(entry, self.shortcuts.bindings))
    }

    pub(super) fn open_with_view<'a>(&self, panel: &'a open_with::Panel) -> Element<'a, Message> {
        let generation = self.application_generation;
        let view = panel.view(self.opening.is_some(), self.unconfirmed_open);
        crate::shortcuts::capture_numbered(view.map(move |event| Message::OpenWith { generation, event }))
    }

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
        if self.opening.is_some() && !matches!(event, open_with::Event::Back | open_with::Event::ReturnToSearch) {
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
        self.settings = self.settings.map(|_| settings::Entry::SearchStatus);
        let load = self.refresh_search_status();
        Task::batch([load, self.resize()])
    }

    pub(super) fn close_search_status(&mut self) -> Task<Message> {
        self.search_status = None;
        self.return_to_parent()
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

    pub(super) fn open_builtin(&mut self, command: Builtin) -> Task<Message> {
        if self.opening.is_some() || self.search_suspended() {
            return Task::none();
        }
        match command {
            Builtin::Settings => self.toggle_settings(),
            Builtin::Appearance => self.show_appearance(appearance::Tab::Colors),
            Builtin::Layout => self.show_appearance(appearance::Tab::Layout),
            Builtin::SearchFolders => self.show_folders(false),
            Builtin::SearchStatus => self.toggle_search_status(),
            Builtin::CreateLink => self.open_saved_editor(super::saved_editor::Kind::Link, None),
            Builtin::CreateSnippet => self.open_saved_editor(super::saved_editor::Kind::Snippet, None),
            Builtin::Shortcuts => self.show_shortcuts(),
            Builtin::GlobalShortcuts => self.show_global_shortcuts(),
            Builtin::ClipboardHistory => self.show_clipboard_history(),
            Builtin::SwitchWindows => self.show_windows(),
        }
    }

    pub(super) fn deliver_command(&mut self, request: CommandRequest) -> Task<Message> {
        let Some(reply) = request.take_reply() else { return Task::none() };
        if self.opening.is_some() || self.search_suspended() {
            let _ = reply.send(Err("Return to the launcher search before opening another command."));
            return Task::none();
        }
        let open = self.open_direct_command(request.command);
        let _ = reply.send(Ok(()));
        open
    }

    pub(super) fn open_direct_command(&mut self, command: Builtin) -> Task<Message> {
        timing::event("direct_command_opened");
        self.open_builtin(command)
    }

    pub(super) fn show_appearance(&mut self, tab: appearance::Tab) -> Task<Message> {
        self.settings = self.settings.map(|_| settings::Entry::Appearance);
        self.panel = None;
        self.search_status = None;
        self.clicks = ClickTracker::default();
        self.appearance_generation += 1;
        let generation = self.appearance_generation;
        let panel = appearance::Panel::new(self.settings_path.clone(), self.preferences, tab);
        let load = panel.load().map(move |event| Message::Appearance { generation, event });
        self.appearance = Some(panel);
        timing::event("appearance_opened");
        Task::batch([load, self.resize()])
    }
}
