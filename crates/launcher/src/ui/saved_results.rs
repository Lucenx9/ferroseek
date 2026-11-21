// SPDX-License-Identifier: GPL-3.0-or-later
//! Match saved launcher entries and dispatch links from results.

use iced::Task;

use super::{Action, Launcher, Message, Scope, Status, paste, saved_editor};
use crate::links::{self, Link};
use crate::{model, snippets};

impl Launcher {
    pub(super) fn snippet_action(&mut self, action: Action, snippet: &snippets::Snippet) -> Task<Message> {
        match action {
            Action::CopySnippet => self.copy(snippet.body.to_string(), "snippet"),
            Action::PasteSnippet => self.paste_text(snippet.body.to_string(), paste::Origin::Snippet),
            Action::EditSnippet => {
                self.open_saved_editor(saved_editor::Kind::Snippet, Some(saved_editor::Entry::Snippet(snippet.clone())))
            }
            _ => Task::none(),
        }
    }

    pub(super) fn append_search_fallbacks(&mut self) {
        let (Scope::All, Status::Idle, None, []) =
            (self.scope, &self.status, &self.search_folder, self.items.as_slice())
        else {
            return;
        };
        if self.scope.file_query(&self.query) {
            let terms = self.query.trim();
            self.items.extend(self.saved.links.iter().filter_map(|link| model::fallback_item(link, terms)));
        }
    }

    pub(super) fn merge_saved_entries(&mut self) {
        if self.search_folder.is_some() || crate::calc::is_explicit(&self.query) {
            return;
        }
        let all = match self.scope {
            Scope::All => true,
            Scope::Commands => false,
            _ => return,
        };
        let links = links::matching(&self.saved.links, &self.query, all)
            .into_iter()
            .map(|(link, terms, score)| (model::link_item(link, terms), score));
        let snippets = snippets::matching(&self.saved.snippets, &self.query, all)
            .into_iter()
            .map(|(snippet, score)| (model::snippet_item(snippet), score));
        let scripts = self
            .scripts
            .catalog
            .matching(&self.query, all)
            .into_iter()
            .map(|(script, score)| (model::script_item(script), score));
        let workflows = self
            .scripts
            .catalog
            .matching_workflows(&self.query, all)
            .into_iter()
            .map(|(workflow, score)| (model::workflow_item(workflow), score));
        let mut matches: Vec<_> = links.chain(snippets).chain(scripts).chain(workflows).collect();
        if self.query.trim().is_empty() {
            // Favorites and recent runs are already in the empty search.
            matches.retain(|(item, _)| !self.items.iter().any(|current| current.target == item.target));
        }
        matches.sort_by_key(|(_, score)| std::cmp::Reverse(*score));
        let index = if self.query.trim().is_empty() { self.items.len() } else { 0 };
        self.items.splice(index..index, matches.into_iter().map(|(item, _)| item));
    }

    pub(super) fn open_saved_link(&mut self, link: &Link, terms: &str) -> Task<Message> {
        if self.opening.is_some() {
            return Task::none();
        }
        match link.open(terms) {
            Ok(links::Open::Prompt(query)) => self.resume_query(query),
            Ok(request) => self.submit_link(link, request),
            Err(error) => {
                self.status = Status::Error(error);
                self.resize()
            }
        }
    }

    fn submit_link(&mut self, link: &Link, request: links::Open) -> Task<Message> {
        use super::RunError;

        if self.unconfirmed_open {
            self.status = Status::Error(super::UNCONFIRMED_OPEN.into());
            return self.resize();
        }
        let backend = self.backend.clone();
        let activation = self.activation.clone();
        self.opening = Some((link.name.clone(), String::new()));
        self.status = Status::Notice("Opening link…".into());
        let open = async move {
            match request {
                links::Open::Indexed(uri) => backend.run(uri, "").await,
                links::Open::Web(uri) => {
                    let token = activation.token(String::new()).await;
                    zruntime::unblock(move || crate::launch::open_web(&uri, token.as_deref()))
                        .await
                        .map_err(|error| RunError::Failed(error.to_string()))
                }
                links::Open::Prompt(_) => unreachable!("prompts are handled before dispatch"),
            }
        };
        let (task, handle) = Task::perform(open, Message::LinkOpened).abortable();
        self.launch_task = Some(handle.abort_on_drop());
        Task::batch([task, self.resize()])
    }
}
