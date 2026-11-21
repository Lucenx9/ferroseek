// SPDX-License-Identifier: GPL-3.0-or-later
//! Favorites actions in the search results and command catalog.

use std::{borrow::Cow, io};

use iced::Task;

use super::{Action, Launcher, Message, Status, Target, actions_id, search_id};
use crate::favorites::Kind;

impl Launcher {
    fn favorite_key<'a>(&'a self, target: &'a Target) -> Option<(Kind, Cow<'a, str>)> {
        self.scripts
            .history_key(target)
            .map(|key| (Kind::Command, key.into()))
            .or_else(|| target.favorite_key(&self.apps, &self.saved))
    }

    pub(super) fn is_favorite(&self, target: &Target) -> bool {
        self.favorite_key(target).is_some_and(|(kind, key)| self.favorites.contains(kind, &key))
    }

    pub(super) fn favorite_action(&mut self, target: &Target, action: Action) -> Task<Message> {
        match action {
            Action::AddFavorite => self.set_favorite(target, true),
            Action::RemoveFavorite => self.set_favorite(target, false),
            Action::MoveFavoriteUp => self.move_favorite(target, -1),
            Action::MoveFavoriteDown => self.move_favorite(target, 1),
            _ => Task::none(),
        }
    }

    fn set_favorite(&mut self, target: &Target, enabled: bool) -> Task<Message> {
        let Some((kind, key)) = self.favorite_key(target) else { return Task::none() };
        let key = key.into_owned();
        let message = match (enabled, self.favorites.is_persistent()) {
            (true, true) => "Added to favorites",
            (true, false) => "Added to favorites for this session",
            (false, _) => "Removed from favorites",
        };
        let result = self.favorites.set(kind, &key, enabled);
        self.finish_favorite_update(result, message)
    }

    pub(super) fn add_favorite_move_actions(&self, target: &Target, actions: &mut Vec<Action>) {
        for (offset, action) in [(-1, Action::MoveFavoriteUp), (1, Action::MoveFavoriteDown)] {
            if self.favorite_neighbor(target, offset).is_some() {
                actions.push(action);
            }
        }
    }

    fn move_favorite(&mut self, target: &Target, offset: isize) -> Task<Message> {
        let Some(neighbor) = self.favorite_neighbor(target, offset) else { return Task::none() };
        let (earlier, later, message) =
            if offset < 0 { (neighbor, target, "Moved up") } else { (target, neighbor, "Moved down") };
        let Some(((first_kind, first_key), (second_kind, second_key))) =
            self.favorite_key(earlier).zip(self.favorite_key(later))
        else {
            return Task::none();
        };
        // Target identities can borrow saved data. Copy keys before writing.
        let (first_key, second_key) = (first_key.into_owned(), second_key.into_owned());
        let result = self.favorites.swap_in_order((first_kind, &first_key), (second_kind, &second_key));
        self.finish_favorite_update(result, message)
    }

    fn favorite_neighbor(&self, target: &Target, offset: isize) -> Option<&Target> {
        if self.search_folder.is_some() || !self.query.trim().is_empty() {
            return None;
        }
        let index =
            self.items.iter().position(|item| item.target.same_identity(target) && self.is_favorite(&item.target))?;
        let neighbor = &self.items.get(index.checked_add_signed(offset)?)?.target;
        self.is_favorite(neighbor).then_some(neighbor)
    }

    fn finish_favorite_update(&mut self, result: io::Result<()>, message: &str) -> Task<Message> {
        self.status = match result {
            Ok(()) => {
                self.panel = None;
                self.rebuild(true);
                Status::Notice(message.into())
            }
            Err(e) if e.kind() == io::ErrorKind::WouldBlock => Status::Error("Favorites are busy. Try again.".into()),
            Err(e) => Status::Error(format!("Cannot update favorites: {e}")),
        };
        let focus = if self.panel.is_some() { actions_id() } else { search_id() };
        Task::batch([self.resize(), crate::input::focus(focus)])
    }
}
