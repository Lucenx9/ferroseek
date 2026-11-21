// SPDX-License-Identifier: GPL-3.0-or-later
//! Launcher window tests: input routing, view state and rendering helpers.

use super::*;

fn typed(value: &str, modifiers: Modifiers) -> Event {
    Event::Keyboard(keyboard::Event::KeyPressed {
        key: Key::Character(value.into()),
        modified_key: Key::Character(value.into()),
        physical_key: keyboard::key::Physical::Unidentified(keyboard::key::NativeCode::Unidentified),
        location: keyboard::Location::Standard,
        modifiers,
        text: (!modifiers.command()).then(|| value.into()),
        repeat: false,
    })
}

fn widget_batch(s: &mut Launcher, cache: &mut iced_runtime::user_interface::Cache, events: &[Event], initial: bool) {
    widget_batch_at(s, cache, events, initial, mouse::Cursor::Unavailable);
}

fn widget_batch_at(
    s: &mut Launcher,
    cache: &mut iced_runtime::user_interface::Cache,
    events: &[Event],
    initial: bool,
    cursor: mouse::Cursor,
) {
    let mut renderer =
        iced::Renderer::Secondary(iced_tiny_skia::Renderer::new(iced::Font::DEFAULT, iced::Pixels(16.0)));
    let (messages, next_cache) = {
        let mut ui = iced_runtime::UserInterface::build(
            view(s),
            iced::Size::new(s.window_width as f32, full_height() as f32),
            std::mem::take(cache),
            &mut renderer,
        );
        if initial {
            ui.operate(&renderer, &mut iced::advanced::widget::operation::focusable::focus::<()>(search_id()));
        }
        let mut messages = Vec::new();
        let (_, statuses) =
            ui.update(events, cursor, &mut renderer, &mut iced::advanced::clipboard::Null, &mut messages);
        for (event, status) in events.iter().cloned().zip(statuses) {
            if let Some(message) = on_event(event, status, iced::window::Id::unique()) {
                messages.push(message);
            }
        }
        (messages, ui.into_cache())
    };
    *cache = next_cache;
    // Deliberately do not run asynchronous widget-focus tasks. The actual
    // tree must keep later keys away from a field whose context just ended.
    for message in messages {
        let _ = update(s, message);
    }
}

fn settle_widgets(s: &mut Launcher, cache: &mut iced_runtime::user_interface::Cache) {
    for _ in 0..16 {
        widget_batch(
            s,
            cache,
            &[Event::Window(iced::window::Event::RedrawRequested(std::time::Instant::now()))],
            false,
        );
    }
}

fn named(key: keyboard::key::Named) -> Event {
    Event::Keyboard(keyboard::Event::KeyPressed {
        key: Key::Named(key),
        modified_key: Key::Named(key),
        physical_key: keyboard::key::Physical::Unidentified(keyboard::key::NativeCode::Unidentified),
        location: keyboard::Location::Standard,
        modifiers: Modifiers::empty(),
        text: None,
        repeat: false,
    })
}

#[test]
fn immediate_link_editor_input_follows_each_tab_without_focus_tasks() {
    use iced::keyboard::key::Named;
    let mut s = search_state();
    let _ = update(&mut s, Message::Query("create link".into()));
    let mut cache = iced_runtime::user_interface::Cache::default();
    let mut events = vec![named(Named::Enter)];
    for (index, value) in ["Atlas", "https://example.net/?q={query}", "atlas"].into_iter().enumerate() {
        if index != 0 {
            events.push(named(Named::Tab));
        }
        events.extend(value.chars().map(|c| typed(&c.to_string(), Modifiers::empty())));
    }
    widget_batch(&mut s, &mut cache, &events, true);
    settle_widgets(&mut s, &mut cache);
    let saved_editor::Entry::Link(draft) = s.saved_editor.as_ref().unwrap().draft().unwrap() else {
        panic!("expected a link");
    };
    assert_eq!(
        (draft.name.as_str(), draft.address.as_str(), draft.keyword.as_str()),
        ("Atlas", "https://example.net/?q={query}", "atlas")
    );
    assert_eq!(s.query, "create link");
}

#[test]
fn snippet_editor_keeps_enter_and_arrow_keys_in_the_multiline_field() {
    use iced::keyboard::key::Named;

    let mut state = search_state();
    let _ = update(&mut state, Message::Query("create snippet".into()));
    let mut cache = iced_runtime::user_interface::Cache::default();
    let events = [
        named(Named::Enter),
        typed("N", Modifiers::empty()),
        named(Named::Tab),
        typed("a", Modifiers::empty()),
        named(Named::Enter),
        typed("b", Modifiers::empty()),
        named(Named::ArrowUp),
        named(Named::End),
        typed("c", Modifiers::empty()),
        named(Named::Tab),
        typed("q", Modifiers::empty()),
    ];
    widget_batch(&mut state, &mut cache, &events, true);
    settle_widgets(&mut state, &mut cache);
    let saved_editor::Entry::Snippet(snippet) = state.saved_editor.as_ref().unwrap().draft().unwrap() else {
        panic!("expected pending snippet");
    };
    assert_eq!((snippet.name.as_str(), &*snippet.body, snippet.keyword.as_str()), ("N", "ac\nb", "q"));
    assert!(state.saved.snippets.is_empty());
}

#[test]
fn exact_snippet_keyword_wins_over_a_saved_link_name() {
    let mut state = search_state();
    state.saved.links.push(
        crate::links::Link::new(crate::links::LinkFields {
            name: "Brief",
            address: "https://example.net/",
            keyword: "",
        })
        .unwrap(),
    );
    state.saved.snippets.push(
        crate::snippets::Snippet::new(crate::snippets::Fields {
            name: "Sprint reply",
            keyword: "brief",
            body: "Thanks\nSee you tomorrow",
        })
        .unwrap(),
    );
    let _ = update(&mut state, Message::Query("BRIEF".into()));
    assert!(matches!(state.selected().map(|item| &item.target), Some(Target::Snippet(_))));
    assert!(state.items.iter().any(|item| matches!(item.target, Target::Link(..))));
}

#[test]
fn action_shortcut_and_typing_in_one_event_batch_keep_the_main_query() {
    check_action_shortcut_typing("k", Modifiers::CTRL);
    check_action_shortcut_typing("u", Modifiers::CTRL | Modifiers::SHIFT);
}

fn check_action_shortcut_typing(key: &str, modifiers: Modifiers) {
    let mut s = search_state();
    s.shortcuts
        .bindings
        .set(Shortcut::Actions, Chord::new(key.chars().next().unwrap(), modifiers.shift()).unwrap())
        .unwrap();
    let _ = update(&mut s, Message::Query("report".into()));
    let mut cache = iced_runtime::user_interface::Cache::default();
    widget_batch(&mut s, &mut cache, &[], true);
    widget_batch(
        &mut s,
        &mut cache,
        &[
            Event::Keyboard(keyboard::Event::ModifiersChanged(modifiers)),
            typed(key, modifiers),
            Event::Keyboard(keyboard::Event::ModifiersChanged(Modifiers::empty())),
            typed("p", Modifiers::empty()),
            typed("i", Modifiers::empty()),
            typed("n", Modifiers::empty()),
        ],
        false,
    );
    settle_widgets(&mut s, &mut cache);
    assert_eq!(s.query, "report");
    assert_eq!(s.panel.as_ref().unwrap().query, "pin");
}

#[test]
fn repeated_menu_transitions_preserve_order_and_native_text_editing() {
    let mut s = search_state();
    let _ = update(&mut s, Message::Query("report".into()));
    let mut cache = iced_runtime::user_interface::Cache::default();
    widget_batch(&mut s, &mut cache, &[], true);
    let control = |value| {
        [
            Event::Keyboard(keyboard::Event::ModifiersChanged(Modifiers::CTRL)),
            typed(value, Modifiers::CTRL),
            Event::Keyboard(keyboard::Event::ModifiersChanged(Modifiers::empty())),
        ]
    };
    let mut events = Vec::new();
    events.extend(control("k"));
    events.push(typed("p", Modifiers::empty()));
    events.extend(control("k"));
    events.push(typed("z", Modifiers::empty()));
    events.push(Event::Keyboard(keyboard::Event::KeyPressed {
        key: Key::Named(keyboard::key::Named::Backspace),
        modified_key: Key::Named(keyboard::key::Named::Backspace),
        physical_key: keyboard::key::Physical::Code(keyboard::key::Code::Backspace),
        location: keyboard::Location::Standard,
        modifiers: Modifiers::empty(),
        text: None,
        repeat: false,
    })); // Restore a matching result before opening its actions again.
    events.extend(control("k"));
    events.push(typed("x", Modifiers::empty()));
    events.extend(control("a")); // Native select-all in the new action field.
    events.push(typed("é", Modifiers::empty()));
    widget_batch(&mut s, &mut cache, &events, false);
    settle_widgets(&mut s, &mut cache);
    assert_eq!(s.query, "report");
    assert_eq!(s.generation, 3, "the temporary edit and its deletion both reached the main field");
    assert_eq!(s.panel.as_ref().unwrap().query, "é");
}

#[test]
fn query_prefill_is_applied_before_the_first_input_event() {
    let temp = tempfile::tempdir().unwrap();
    let config = temp.path().join("config.toml");
    std::fs::write(&config, "roots = []\n").unwrap();
    let (mut s, _pending_tasks) = boot_from(
        Boot {
            config: ferroseek_config::Config::default(),
            opaque: true,
            activation: Activation::default(),
            backend: Backend::disconnected(),
            apps: search_state().apps,
            usage: Usage::disabled(),
            favorites: Favorites::disabled(),
            initial_query: "report".into(),
            settings_path: config,
        },
        Material::default(),
        output::Observer::default(),
        Icons::disabled(),
    );
    assert_eq!(s.query, "report");
    let mut cache = iced_runtime::user_interface::Cache::default();
    widget_batch(&mut s, &mut cache, &[typed("x", Modifiers::empty())], false);
    assert_eq!(s.query, "reportx");
}

#[test]
fn context_click_and_immediate_typing_share_one_ordered_widget_batch() {
    let mut s = search_state();
    let _ = update(&mut s, Message::Query("report".into()));
    let mut cache = iced_runtime::user_interface::Cache::default();
    widget_batch_at(
        &mut s,
        &mut cache,
        &[
            Event::Mouse(mouse::Event::ButtonPressed(mouse::Button::Right)),
            Event::Mouse(mouse::Event::ButtonReleased(mouse::Button::Right)),
            typed("p", Modifiers::empty()),
        ],
        true,
        mouse::Cursor::Available(iced::Point::new(200.0, 90.0)),
    );
    settle_widgets(&mut s, &mut cache);
    assert_eq!(s.query, "report");
    assert_eq!(s.panel.as_ref().unwrap().query, "p");
}

#[test]
fn focus_loss_discards_deferred_keys_before_the_next_activation() {
    let mut s = search_state();
    let _ = update(&mut s, Message::Query("report".into()));
    s.clipboard_owner = true; // A clipboard-serving instance may remain open.
    let mut cache = iced_runtime::user_interface::Cache::default();
    widget_batch(
        &mut s,
        &mut cache,
        &[typed("k", Modifiers::CTRL), typed("p", Modifiers::empty()), Event::Window(iced::window::Event::Unfocused)],
        true,
    );
    settle_widgets(&mut s, &mut cache);
    assert!(s.panel.as_ref().unwrap().query.is_empty());
    widget_batch(
        &mut s,
        &mut cache,
        &[
            Event::Window(iced::window::Event::Focused),
            Event::Keyboard(keyboard::Event::ModifiersChanged(Modifiers::empty())),
            typed("x", Modifiers::empty()),
        ],
        false,
    );
    assert_eq!(s.query, "report");
    assert_eq!(s.panel.as_ref().unwrap().query, "x");
}

#[test]
fn delayed_focus_tasks_cannot_steal_action_input_or_reset_its_selection() {
    let mut s = search_state();
    let _ = update(&mut s, Message::Query("report".into()));
    let temp = tempfile::tempdir().unwrap();
    s.settings_path = temp.path().join("config.toml");
    std::fs::write(&s.settings_path, "roots = []\n").unwrap();
    let _ = update(&mut s, Message::ShowFolders);
    let generation = s.folder_generation;
    let folder_focus = update(
        &mut s,
        Message::Folder {
            generation,
            event: folders::Event::Loaded {
                revision: 1,
                result: ferroseek_config::folders::Document::load(temp.path().join("config.toml")),
            },
        },
    );
    let _ = command(&mut s, Command::Back);
    assert!(s.folder_settings.is_none());
    let mut cache = iced_runtime::user_interface::Cache::default();
    widget_batch(&mut s, &mut cache, &[typed("k", Modifiers::CTRL)], true);
    settle_widgets(&mut s, &mut cache);
    widget_batch(
        &mut s,
        &mut cache,
        &[
            typed("o", Modifiers::empty()),
            typed("p", Modifiers::empty()),
            Event::Keyboard(keyboard::Event::ModifiersChanged(Modifiers::CTRL)),
            typed("a", Modifiers::CTRL),
            Event::Keyboard(keyboard::Event::ModifiersChanged(Modifiers::empty())),
        ],
        false,
    );
    let mut renderer =
        iced::Renderer::Secondary(iced_tiny_skia::Renderer::new(iced::Font::DEFAULT, iced::Pixels(16.0)));
    {
        let mut ui = iced_runtime::UserInterface::build(
            view(&s),
            iced::Size::new(WIDTH as f32, full_height() as f32),
            std::mem::take(&mut cache),
            &mut renderer,
        );
        for target in [search_id(), actions_id()] {
            let mut stream = iced_runtime::task::into_stream(crate::input::focus::<Message>(target)).unwrap();
            let action = iced::futures::executor::block_on(stream.next()).unwrap();
            let iced_runtime::Action::Widget(mut operation) = action else {
                panic!("focus must use a widget operation")
            };
            ui.operate(&renderer, operation.as_mut());
        }
        let mut stream = iced_runtime::task::into_stream(folder_focus).unwrap();
        let action = iced::futures::executor::block_on(stream.next()).unwrap();
        let iced_runtime::Action::Widget(mut operation) = action else {
            panic!("loaded folder settings must request focus")
        };
        ui.operate(&renderer, operation.as_mut());
        cache = ui.into_cache();
    }
    widget_batch(&mut s, &mut cache, &[typed("é", Modifiers::empty())], false);
    assert_eq!(s.query, "report");
    assert_eq!(s.panel.as_ref().unwrap().query, "é");
}

#[test]
fn appearance_shortcut_does_not_insert_its_printable_key() {
    let mut state = search_state();
    let _ = update(&mut state, Message::Query("report".into()));
    let mut shortcut = typed("<", Modifiers::CTRL | Modifiers::SHIFT);
    if let Event::Keyboard(keyboard::Event::KeyPressed { physical_key, text, .. }) = &mut shortcut {
        *physical_key = keyboard::key::Physical::Code(keyboard::key::Code::Comma);
        *text = Some("<".into());
    }
    let mut cache = iced_runtime::user_interface::Cache::default();
    widget_batch(&mut state, &mut cache, &[shortcut], false);
    assert_eq!(state.query, "report");
    assert!(state.appearance.is_some());
}

#[test]
fn pending_search_completes_while_appearance_is_open() {
    let mut state = search_state();
    let _ = update(&mut state, Message::Query("report".into()));
    let generation = state.generation;
    let _ = update(&mut state, Message::ShowAppearance);
    let _ = update(&mut state, Message::Files { generation, result: Ok(Some(file_hits())) });
    assert_eq!(state.status, Status::Idle);
    assert_eq!(state.files.len(), file_hits().len());
    assert!(state.appearance.is_some());
    assert_eq!(state.query, "report");
}

#[test]
fn appearance_preview_cancel_and_system_changes_preserve_search() {
    let temp = tempfile::tempdir().unwrap();
    let path = temp.path().join("config.toml");
    std::fs::write(&path, "roots = []").unwrap();
    let mut state = search_state();
    state.settings_path = path.clone();
    state.query = "report".into();
    let _ = update(&mut state, Message::ShowAppearance);
    let generation = state.appearance_generation;
    let _ = update(
        &mut state,
        Message::Appearance {
            generation,
            event: appearance::Event::Loaded(Ok(ferroseek_config::folders::Document::load(path).unwrap())),
        },
    );
    let _ = update(&mut state, Message::Appearance { generation, event: appearance::Event::Choose(1) });
    assert_eq!(state.preferences.mode, ferroseek_config::appearance::Mode::Light);
    assert!(state.appearance_theme(false).palette().background.r > 0.9);
    let _ = update(&mut state, Message::Query("stale".into()));
    let _ = update(&mut state, Message::Ui(UiEvent::DesktopAppearance(Some(true))));
    assert_eq!(state.query, "report");
    assert!(state.appearance_theme(false).palette().background.r > 0.9, "explicit Light ignores desktop changes");
    let _ = update(&mut state, Message::Cmd(Command::Back));
    assert!(state.appearance.is_some(), "unsaved changes need explicit discard");
    let _ = update(&mut state, Message::Appearance { generation, event: appearance::Event::Discard });
    assert!(state.appearance.is_none());
    assert_eq!(state.preferences, Preferences::default());
    assert_eq!(state.query, "report");
    let _ = update(&mut state, Message::Appearance { generation, event: appearance::Event::Choose(1) });
    assert_eq!(state.preferences, Preferences::default(), "late page events are ignored");
    let _ = update(&mut state, Message::Ui(UiEvent::DesktopAppearance(Some(false))));
    assert!(state.appearance_theme(false).palette().background.r > 0.9, "System follows the desktop");
}

#[test]
fn appearance_tab_switch_cancels_pressed_choices() {
    let temp = tempfile::tempdir().unwrap();
    let path = temp.path().join("config.toml");
    std::fs::write(&path, "roots = []\n[appearance]\nmode = \"dark\"\n").unwrap();
    for (tab, switch) in
        [(appearance::Tab::Colors, Command::NextScope), (appearance::Tab::Layout, Command::PreviousScope)]
    {
        let mut state = search_state();
        state.settings_path = path.clone();
        let _ = update(&mut state, Message::ShowAppearance);
        let generation = state.appearance_generation;
        let _ = update(
            &mut state,
            Message::Appearance {
                generation,
                event: appearance::Event::Loaded(Ok(ferroseek_config::folders::Document::load(path.clone()).unwrap())),
            },
        );
        let _ = update(&mut state, Message::Appearance { generation, event: appearance::Event::Tab(tab) });
        let mut cache = iced_runtime::user_interface::Cache::default();
        let labels = rendered_labels(view_content(&state), iced::Size::new(WIDTH as f32, full_height() as f32));
        let first = if tab == appearance::Tab::Colors { "System" } else { "Compact · 640" };
        let (_, bounds) = labels.text.iter().find(|(value, _)| value == first).unwrap();
        let cursor = mouse::Cursor::Available(bounds.center());
        let press = Event::Mouse(mouse::Event::ButtonPressed(mouse::Button::Left));
        let release = Event::Mouse(mouse::Event::ButtonReleased(mouse::Button::Left));
        widget_batch_at(&mut state, &mut cache, std::slice::from_ref(&press), false, cursor);
        let original = state.preferences;
        let _ = update(&mut state, Message::Cmd(switch));
        widget_batch_at(&mut state, &mut cache, std::slice::from_ref(&release), false, cursor);
        assert_eq!(state.preferences, original, "release after leaving {tab:?}");

        widget_batch_at(&mut state, &mut cache, &[press, release], false, cursor);
        if tab == appearance::Tab::Colors {
            assert_eq!(state.preferences.width, ferroseek_config::appearance::Width::Compact);
        } else {
            assert_eq!(state.preferences.mode, ferroseek_config::appearance::Mode::System);
        }
        let _ = update(&mut state, Message::Cmd(Command::Back));
        assert!(state.appearance.is_some(), "a new click still edits the draft");
        let _ = update(&mut state, Message::Appearance { generation, event: appearance::Event::Discard });
        assert!(state.appearance.is_none());
        assert_eq!(state.preferences, original);
    }
}

#[test]
fn material_changes_preserve_the_current_search_and_actions() {
    let mut state = application_actions(&["New window"]);
    state.query = "report".into();
    state.panel.as_mut().unwrap().query = "window".into();
    for enabled in [true, false, true] {
        let _ = update(&mut state, Message::MaterialChanged(enabled));
        assert_eq!(state.query, "report");
        assert_eq!(state.panel.as_ref().unwrap().query, "window");
        assert_eq!(state.cursor.selected, 0);
        assert_eq!(material_theme(state.glass).palette().background.a < 1.0, enabled);
    }
}

#[test]
fn material_keeps_small_text_readable_over_light_and_dark_backdrops() {
    fn over(top: Color, bottom: Color) -> Color {
        Color::from_rgb(
            top.r * top.a + bottom.r * (1.0 - top.a),
            top.g * top.a + bottom.g * (1.0 - top.a),
            top.b * top.a + bottom.b * (1.0 - top.a),
        )
    }
    fn luminance(color: Color) -> f32 {
        let linear = |v: f32| if v <= 0.04045 { v / 12.92 } else { ((v + 0.055) / 1.055).powf(2.4) };
        linear(color.r) * 0.2126 + linear(color.g) * 0.7152 + linear(color.b) * 0.0722
    }
    for choice in [
        ferroseek_config::appearance::Palette::Iron,
        ferroseek_config::appearance::Palette::Slate,
        ferroseek_config::appearance::Palette::Forest,
    ] {
        for mode in [ferroseek_config::appearance::Mode::Dark, ferroseek_config::appearance::Mode::Light] {
            for glass in [false, true] {
                let theme = &appearance::theme(
                    Preferences { mode, light_theme: choice, dark_theme: choice, ..Default::default() },
                    None,
                    glass,
                );
                for backdrop in [Color::WHITE, Color::BLACK] {
                    let surface = over(theme.palette().background, backdrop);
                    for (background_index, background) in [
                        surface,
                        over(palette::FAINT.resolve(theme), surface),
                        over(palette::SELECTED.resolve(theme), surface),
                        palette::RAISED.resolve(theme),
                    ]
                    .into_iter()
                    .enumerate()
                    {
                        for foreground in [palette::TEXT.resolve(theme), palette::DIM.resolve(theme)].into_iter().chain(
                            (background_index < 2)
                                .then(|| recovery_button_style(theme, button::Status::Active).text_color),
                        ) {
                            let a = luminance(foreground);
                            let b = luminance(background);
                            let ratio = (a.max(b) + 0.05) / (a.min(b) + 0.05);
                            assert!(ratio >= 4.5, "small text contrast {ratio} on {background:?}");
                        }
                    }
                }
            }
        }
    }
}

fn report_app() -> App {
    crate::apps::parse(
        "report.desktop",
        std::path::Path::new("/synthetic"),
        "[Desktop Entry]\nType=Application\nName=Report Viewer\nExec=viewer\n",
        &[],
        &[],
    )
    .unwrap()
}

fn search_state() -> Launcher {
    Launcher {
        output: output::Observer::default(),
        output_bounds: None,
        placement: None,
        window_height: full_height(),
        preferences: Preferences::default(),
        system_dark: None,
        opaque: false,
        settings: None,
        shortcuts: shortcut_settings::State::new(Default::default()),
        appearance: None,
        appearance_generation: 0,
        input_epoch: 0,
        material: Material::default(),
        glass: false,
        activation: Activation::default(),
        backend: Backend::disconnected(),
        apps: vec![report_app()],
        usage: Usage::disabled(),
        favorites: Favorites::disabled(),
        saved: saved::Store::default(),
        alias_editor: None,
        saved_editor: None,
        edit_generation: 0,
        opening: None,
        launch_task: None,
        unconfirmed_open: false,
        clipboard_owner: false,
        icons: Icons::disabled(),
        query: String::new(),
        scope: Scope::All,
        search_folder: None,
        folder_more: false,
        folder_search_history: Vec::new(),
        scope_generation: 0,
        app_hits: Vec::new(),
        files: Vec::new(),
        items: Vec::new(),
        item_icons: Vec::new(),
        cursor: Cursor::new(VISIBLE_ROWS),
        clicks: ClickTracker::default(),
        panel: None,
        search_status: None,
        application_picker: None,
        application_generation: 0,
        file_details: None,
        details_generation: 0,
        preview_enabled: false,
        window_width: WIDTH,
        text_preview: None,
        preview_generation: 0,
        preview_request: None,
        folder_settings: None,
        folder_generation: 0,
        settings_path: "/synthetic/config.toml".into(),
        deferred_query: None,
        status_generation: 0,
        generation: 0,
        status: Status::Idle,
        compact: true,
    }
}

fn file_hits() -> Vec<FileHit> {
    ["report.txt", "report.pdf"]
        .into_iter()
        .map(|name| FileHit {
            id: format!("file:///synthetic/{name}"),
            name: name.into(),
            path: Some(std::path::Path::new("/synthetic").join(name)),
            icon: String::new(),
            subtext: "/synthetic".into(),
            is_dir: false,
        })
        .collect()
}

#[test]
fn scope_changes_replace_results_and_reject_late_replies() {
    let mut s = search_state();
    let _ = update(&mut s, Message::Query("report".into()));
    let old_generation = s.generation;
    let _ = update(&mut s, Message::Files { generation: old_generation, result: Ok(Some(file_hits())) });
    assert_eq!(s.items.len(), 3);
    s.cursor.select(2, s.items.len());
    let _ = command(&mut s, Command::OpenActions);
    assert!(s.panel.is_some());

    let _ = command(&mut s, Command::NextScope);
    assert_eq!(s.scope, Scope::Applications);
    assert_eq!(s.query, "report");
    assert_eq!(s.generation, old_generation + 1);
    assert_eq!(s.status, Status::Idle);
    assert!(s.panel.is_none());
    assert!(s.files.is_empty());
    assert_eq!(s.cursor.selected, 0);
    assert_eq!(s.items.len(), 1);
    assert!(matches!(s.items[0].target, Target::App(_)));
    let _ = update(&mut s, Message::Files { generation: old_generation, result: Ok(Some(file_hits())) });
    let _ = update(&mut s, Message::Files { generation: old_generation, result: Err("old error".into()) });
    assert_eq!(s.items.len(), 1);
    assert_eq!(s.status, Status::Idle);

    let _ = command(&mut s, Command::NextScope);
    assert_eq!(s.scope, Scope::Files);
    assert_eq!(s.query, "report");
    assert_eq!(s.status, Status::Searching);
    assert!(s.app_hits.is_empty());
    assert!(s.items.is_empty());
    let generation = s.generation;
    let _ = update(&mut s, Message::Files { generation, result: Ok(Some(file_hits())) });
    assert_eq!(s.items.len(), 2);
    assert!(s.items.iter().all(|i| matches!(i.target, Target::File(_))));
    let target = s.items[0].target.clone();
    let _ = s.reset_ranking(&target);
    assert!(s.app_hits.is_empty());
    assert_eq!(s.items.len(), 2);
    let _ = command(&mut s, Command::ActivateVisible(1));
    assert_eq!(s.opening.as_ref().unwrap().0, "file:///synthetic/report.pdf");
}

#[test]
fn exact_alias_promotes_the_original_target_once_and_obeys_scope() {
    let mut s = search_state();
    let target = saved::AliasTarget::Application(s.apps[0].id.clone());
    s.saved.aliases.push(saved::Alias::new(target, "report").unwrap());
    let _ = update(&mut s, Message::Query(" REPORT ".into()));
    let generation = s.generation;
    let _ = update(&mut s, Message::Files { generation, result: Ok(Some(file_hits())) });
    let selected = s.selected().unwrap();
    assert_eq!(selected.target, Target::App(0));
    assert_eq!(selected.subtitle, "Alias · report");
    assert_eq!(s.items.iter().filter(|item| item.target == Target::App(0)).count(), 1);
    let _ = update(&mut s, Message::Scope(Scope::Files));
    assert!(s.items.iter().all(|item| matches!(item.target, Target::File(_))));
}

#[test]
fn unavailable_alias_can_be_edited_without_dispatching_an_application() {
    let mut s = search_state();
    let alias = saved::Alias::new(saved::AliasTarget::Application("missing.desktop".into()), "missing").unwrap();
    s.saved.aliases.push(alias.clone());
    let _ = update(&mut s, Message::Query("missing".into()));
    assert_eq!(s.selected().unwrap().target, Target::UnavailableAlias(alias));
    let _ = update(&mut s, Message::Cmd(Command::Activate));
    assert!(s.alias_editor.is_some() && s.opening.is_none());
    let _ = update(&mut s, Message::Cmd(Command::Back));
    assert!(s.alias_editor.is_none());
    assert_eq!(s.query, "missing");
}

#[test]
fn command_action_uses_the_selected_destination_after_file_results_change() {
    use crate::commands::Builtin;

    let mut s = search_state();
    let _ = update(&mut s, Message::Query("window width".into()));
    let generation = s.generation;
    let target = Target::Command(Builtin::Layout);
    assert_eq!(s.selected().unwrap().target, target);
    let _ = command(&mut s, Command::ToggleActions);
    let _ = update(&mut s, Message::Files { generation, result: Ok(Some(file_hits())) });
    assert_eq!(s.selected().unwrap().target, target);
    assert_eq!(s.panel.as_ref().unwrap().actions, [Action::OpenCommand, Action::EditAlias]);
    let _ = update(&mut s, Message::PanelClicked { target: target.clone(), action: Action::OpenCommand });
    assert_eq!(s.appearance.as_ref().unwrap().content_id(), Id::new("appearance-layout"));
    assert!(s.panel.is_none() && s.opening.is_none());
    let appearance_generation = s.appearance_generation;
    let event = appearance::Event::Loaded(Err("Can’t read appearance settings".into()));
    let _ = update(&mut s, Message::Appearance { generation: appearance_generation, event });
    let _ = update(&mut s, Message::Cmd(Command::Back));
    assert!(s.appearance.is_none());
    assert_eq!((s.query.as_str(), s.generation, &s.selected().unwrap().target), ("window width", generation, &target));
}

#[test]
fn commands_tab_lists_local_destinations_and_drops_stale_file_replies() {
    let mut s = search_state();
    let _ = update(&mut s, Message::Query("report".into()));
    let generation = s.generation;
    let rendered = rendered_labels(view_content(&s), iced::Size::new(WIDTH as f32, full_height() as f32));
    let (_, label) = rendered.text.iter().find(|(title, _)| title == "Commands").unwrap();
    for message in pointer_messages(
        &s,
        iced::Point::new(label.center_x(), label.center_y()),
        vec![mouse::Event::ButtonPressed(mouse::Button::Left), mouse::Event::ButtonReleased(mouse::Button::Left)],
    ) {
        let _ = update(&mut s, message);
    }
    assert_eq!((s.scope, &s.status), (Scope::Commands, &Status::Idle));
    let _ = update(&mut s, Message::Files { generation, result: Ok(Some(file_hits())) });
    assert!(s.items.is_empty());
    let _ = command(&mut s, Command::Back);
    assert!(s.query.is_empty() && s.items.len() == 8);
    assert!(s.items.iter().all(|item| matches!(item.target, Target::Command(_))));
}

#[test]
fn explicit_calculator_overrides_scope_but_implicit_calculations_obey_it() {
    let mut s = search_state();
    for scope in Scope::ALL {
        let _ = update(&mut s, Message::Scope(scope));
        let _ = update(&mut s, Message::Query("= 40 + 2".into()));
        assert_eq!(s.items.len(), 1);
        assert_eq!(s.items[0].target, Target::Calc("42".into()));
        assert_eq!(s.status, Status::Idle);
        assert!(s.app_hits.is_empty());
        let _ = update(&mut s, Message::Query("= (".into()));
        assert!(s.items.is_empty());
        assert_eq!(s.status, Status::Idle);
    }
    for scope in [Scope::Applications, Scope::Files] {
        let _ = update(&mut s, Message::Scope(scope));
        let _ = update(&mut s, Message::Query("40 + 2".into()));
        assert!(s.items.is_empty());
        assert_eq!(s.status, if scope == Scope::Files { Status::Searching } else { Status::Idle });
    }
}

#[test]
fn editing_main_query_leaves_actions_while_action_filter_keeps_the_query() {
    let mut s = search_state();
    let _ = update(&mut s, Message::Query("report".into()));
    let _ = command(&mut s, Command::OpenActions);
    let generation = s.generation;
    let _ = update(&mut s, Message::ActionQuery("copy".into()));
    assert_eq!(s.query, "report");
    assert_eq!(s.generation, generation);
    assert_eq!(s.panel.as_ref().unwrap().query, "copy");
    let _ = update(&mut s, Message::Query("report2".into()));
    assert_eq!(s.query, "report2");
    assert_eq!(s.generation, generation + 1);
    assert!(s.panel.is_none());
}

#[test]
fn session_favorites_toggle_from_actions_without_changing_ranking() {
    let mut s = search_state();
    let _ = update(&mut s, Message::Query("report".into()));
    let target = s.selected().unwrap().target.clone();
    let _ = command(&mut s, Command::OpenActions);
    let _ = update(&mut s, Message::ActionQuery("pin".into()));
    assert_eq!(s.panel.as_ref().unwrap().filtered_actions(), [Action::AddFavorite]);
    let _ = command(&mut s, Command::ActivateVisible(0));
    assert!(s.is_favorite(&target));
    assert_eq!(s.status, Status::Notice("Added to favorites for this session".into()));
    assert_eq!(s.query, "report");
    assert!(s.panel.is_none());
    assert!(s.usage.recent(usage::now(), 8).is_empty());
    assert_eq!(s.usage.boost(Kind::App, "report.desktop", "report", usage::now()), 0);
    let _ = s.reset_ranking(&target);
    assert!(s.is_favorite(&target), "reset ranking must not remove an explicit favorite");
    let _ = update(&mut s, Message::Query(String::new()));
    assert_eq!(s.items.len(), 1, "no duplicate app fallback");
    assert_eq!(s.items[0].target, target);
    let _ = command(&mut s, Command::OpenActions);
    let _ = update(&mut s, Message::ActionQuery("unpin".into()));
    assert_eq!(s.panel.as_ref().unwrap().filtered_actions(), [Action::RemoveFavorite]);
    let _ = command(&mut s, Command::Activate);
    assert!(!s.is_favorite(&target));
    assert_eq!(s.items.len(), 1, "unpinning retains the available application suggestion");
}

#[test]
fn favorite_files_lead_home_suggestions_and_respect_scope_and_removal() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("report.txt");
    std::fs::write(&path, "synthetic").unwrap();
    let id = format!("file://{}", path.display());
    let mut s = search_state();
    let _ = update(&mut s, Message::Query("report".into()));
    let generation = s.generation;
    let hit = FileHit {
        id: id.clone(),
        path: Some(path),
        name: "report.txt".into(),
        icon: String::new(),
        subtext: String::new(),
        is_dir: false,
    };
    let _ = update(&mut s, Message::Files { generation, result: Ok(Some(vec![hit])) });
    s.cursor.select(1, s.items.len());
    let target = s.selected().unwrap().target.clone();
    let _ = s.perform(Action::AddFavorite);
    assert!(s.is_favorite(&target));
    let _ = update(&mut s, Message::Query(String::new()));
    assert!(matches!(&s.items[0].target, Target::File(file) if file.id == id));
    assert_eq!(s.items.len(), 2, "favorite before the app fallback");
    let _ = update(&mut s, Message::Scope(Scope::Applications));
    assert!(s.items.iter().all(|item| matches!(item.target, Target::App(_))));
    assert!(s.is_favorite(&target));
    let _ = update(&mut s, Message::Scope(Scope::Files));
    assert_eq!(s.items.len(), 1);
    assert!(matches!(&s.items[0].target, Target::File(file) if file.id == id));
    let _ = s.perform(Action::RemoveFavorite);
    assert!(s.items.is_empty(), "no history or app fallback in Files mode");
    assert!(s.panel.is_none());
}

#[test]
fn busy_favorite_store_preserves_the_menu_and_allows_retry() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("launcher-favorites");
    let mut s = search_state();
    s.favorites = Favorites::load(Some(path.clone()));
    let _ = update(&mut s, Message::Query("report".into()));
    let target = s.selected().unwrap().target.clone();
    let _ = s.perform(Action::AddFavorite);
    assert_eq!(s.status, Status::Notice("Added to favorites".into()));
    let lock = std::fs::OpenOptions::new().read(true).write(true).open(path.with_extension("lock")).unwrap();
    lock.lock().unwrap();
    let _ = command(&mut s, Command::OpenActions);
    let _ = update(&mut s, Message::ActionQuery("unpin".into()));
    let _ = command(&mut s, Command::Activate);
    assert!(s.is_favorite(&target));
    assert!(s.panel.is_some());
    assert_eq!(s.status, Status::Error("Favorites are busy. Try again.".into()));
    drop(lock);
    let _ = command(&mut s, Command::Activate);
    assert!(!s.is_favorite(&target));
    assert!(s.panel.is_none());
}

#[test]
fn numbered_shortcuts_are_not_dispatched_again_by_the_subscription() {
    let event = Event::Keyboard(keyboard::Event::KeyPressed {
        key: Key::Character("1".into()),
        modified_key: Key::Character("1".into()),
        physical_key: keyboard::key::Physical::Code(keyboard::key::Code::Digit1),
        location: keyboard::Location::Standard,
        modifiers: Modifiers::CTRL,
        text: Some("1".into()),
        repeat: false,
    });
    assert!(on_event(event, event::Status::Captured, iced::window::Id::unique()).is_none());
}

#[test]
fn displayed_shortcuts_work_inside_the_action_panel() {
    let panel = Panel {
        title: "notes.txt".into(),
        actions: vec![Action::Open, Action::Reveal, Action::CopyPath, Action::CopyName],
        query: String::new(),
        desktop_labels: Vec::new(),
        cursor: Cursor { selected: 3, ..Cursor::new(VISIBLE_ROWS) },
    };
    let key = |key, mods| model::command_for(&key, mods).unwrap();
    assert_eq!(
        panel.action_for_command(key(Key::Named(keyboard::key::Named::Enter), Modifiers::CTRL)),
        Some(Action::Reveal)
    );
    assert_eq!(
        panel.action_for_command(key(Key::Character("C".into()), Modifiers::CTRL | Modifiers::SHIFT)),
        Some(Action::CopyPath)
    );
    assert_eq!(
        panel.action_for_command(key(Key::Named(keyboard::key::Named::Enter), Modifiers::empty())),
        Some(Action::CopyName)
    );
    let app = Panel {
        title: "Files".into(),
        actions: vec![Action::Launch, Action::CopyName],
        query: String::new(),
        desktop_labels: Vec::new(),
        cursor: Cursor { selected: 0, ..Cursor::new(VISIBLE_ROWS) },
    };
    assert_eq!(app.action_for_command(Command::Secondary), None);
    assert_eq!(app.action_for_command(Command::CopyPath), None);
}

#[test]
fn filtering_actions_keeps_the_target_and_selects_a_matching_action() {
    let mut panel = Panel::new(
        &Item {
            target: Target::File(FileHit {
                id: "file:///tmp/notes.txt".into(),
                name: "notes.txt".into(),
                path: Some("/tmp/notes.txt".into()),
                icon: String::new(),
                subtext: "/tmp".into(),
                is_dir: false,
            }),
            section: model::Section::Files,
            title: "notes.txt".into(),
            subtitle: "/tmp".into(),
            icon: String::new(),
            tag: "File",
        },
        false,
    );
    panel.cursor.selected = 3;
    panel.filter("  PATH  copy ".into());
    assert_eq!(panel.title, "notes.txt");
    assert_eq!(panel.cursor.selected, 0);
    assert_eq!(panel.filtered_actions(), [Action::CopyPath]);
    assert_eq!(panel.action_for_command(Command::Activate), Some(Action::CopyPath));
    // Direct shortcuts retain their meaning even when filtered out.
    assert_eq!(panel.action_for_command(Command::Secondary), Some(Action::Reveal));

    panel.filter("RESET rank".into());
    assert_eq!(panel.filtered_actions(), [Action::ResetRanking]);
    assert_eq!(panel.action_for_command(Command::Activate), Some(Action::ResetRanking));

    panel.filter("does not match".into());
    assert!(panel.filtered_actions().is_empty());
    assert_eq!(panel.action_for_command(Command::Activate), None);
    panel.filter(String::new());
    assert_eq!(panel.action_for_command(Command::Activate), Some(Action::Open));
    assert_eq!(panel.actions.len(), 9);
}

#[test]
fn numbered_actions_follow_the_filter_without_falling_back_to_the_selection() {
    let mut panel = Panel {
        title: "notes.txt".into(),
        actions: vec![Action::Open, Action::Reveal, Action::CopyPath, Action::CopyName, Action::ResetRanking],
        query: String::new(),
        desktop_labels: Vec::new(),
        cursor: Cursor { selected: 4, ..Cursor::new(VISIBLE_ROWS) },
    };
    assert_eq!(panel.action_for_command(Command::ActivateVisible(0)), Some(Action::Open));
    panel.filter("copy".into());
    panel.cursor.selected = 1;
    assert_eq!(panel.action_for_command(Command::ActivateVisible(0)), Some(Action::CopyPath));
    assert_eq!(panel.action_for_command(Command::ActivateVisible(1)), Some(Action::CopyName));
    assert_eq!(panel.action_for_command(Command::ActivateVisible(2)), None);
    assert_eq!(panel.action_for_command(Command::ActivateVisible(8)), None);
    assert_eq!(panel.action_for_command(Command::ActivateVisible(usize::MAX)), None);
    assert_eq!(panel.cursor.selected, 1);
    assert_eq!(panel.query, "copy");
    panel.filter("missing".into());
    assert_eq!(panel.action_for_command(Command::ActivateVisible(0)), None);
}

#[test]
fn action_search_ranks_precise_matches_and_keeps_activation_consistent() {
    let mut panel = Panel {
        title: "notes.txt".into(),
        actions: vec![Action::Open, Action::Reveal, Action::CopyPath, Action::CopyName, Action::ResetRanking],
        query: String::new(),
        desktop_labels: Vec::new(),
        cursor: Cursor { selected: 0, ..Cursor::new(VISIBLE_ROWS) },
    };
    let original = panel.actions.clone();
    for query in ["", "  "] {
        panel.filter(query.into());
        assert_eq!(panel.filtered_actions(), original);
    }
    for (query, action) in [
        ("copnm", Action::CopyName),
        ("COPY filename", Action::CopyName),
        ("reveal", Action::Reveal),
        ("directory", Action::Reveal),
        ("copy location", Action::CopyPath),
        ("rank reset", Action::ResetRanking),
    ] {
        panel.filter(query.into());
        assert_eq!(panel.filtered_actions(), [action], "{query}");
        assert_eq!(panel.action_for_command(Command::Activate), Some(action));
        assert_eq!(panel.action_for_command(Command::ActivateVisible(0)), Some(action));
        assert_eq!(panel.action_for_command(Command::ActivateVisible(1)), None);
    }
    panel.filter("name".into());
    assert_eq!(panel.action_for_command(Command::Secondary), Some(Action::Reveal));
    assert_eq!(panel.action_for_command(Command::CopyPath), Some(Action::CopyPath));
    panel.filter("open".into());
    assert_eq!(panel.filtered_actions().first(), Some(&Action::Open));
    panel.filter("r".into());
    assert_eq!(panel.filtered_actions(), [Action::ResetRanking, Action::Reveal]);
    assert_eq!(panel.action_for_command(Command::Activate), Some(Action::ResetRanking));
    assert_eq!(panel.action_for_command(Command::ActivateVisible(0)), Some(Action::ResetRanking));
    assert_eq!(panel.action_for_command(Command::ActivateVisible(1)), Some(Action::Reveal));
    panel.filter("copy".into());
    assert_eq!(panel.filtered_actions(), [Action::CopyPath, Action::CopyName], "ties keep their order");
    panel.filter("copy xyz".into());
    assert!(panel.filtered_actions().is_empty());
    assert_eq!(panel.action_for_command(Command::Activate), None);
}

#[test]
fn file_metadata_updates_keep_selection_and_queued_actions() {
    let mut s = search_state();
    let _ = update(&mut s, Message::Scope(Scope::Files));
    let _ = update(&mut s, Message::Query("report".into()));
    let generation = s.generation;
    let mut files = file_hits();
    let _ = update(&mut s, Message::Files { generation, result: Ok(Some(files.clone())) });
    s.cursor.select(1, s.items.len());
    let drawn = s.selected().unwrap().target.clone();
    let _ = command(&mut s, Command::OpenActions);
    let _ = update(&mut s, Message::ActionQuery("copy name".into()));

    files[1].icon = "application-pdf".into();
    let expected = Target::File(files[1].clone());
    let _ = update(&mut s, Message::Files { generation, result: Ok(Some(files)) });
    assert_eq!(s.selected().unwrap().target, expected, "an icon update must not select another file");
    assert_eq!(s.panel.as_ref().unwrap().query, "copy name");
    let _ = update(&mut s, Message::PanelClicked { target: drawn.clone(), action: Action::CopyName });
    assert_eq!(s.status, Status::Notice("Copied name".into()));
    assert!(s.panel.is_none());

    s.cursor.select(0, s.items.len());
    let _ = update(&mut s, Message::Selected(drawn));
    assert_eq!(s.selected().unwrap().target, expected, "queued result clicks retain their target");
    let _pending = command(&mut s, Command::Activate);
    assert_eq!(s.opening.as_ref().unwrap().0, "file:///synthetic/report.pdf");
    assert!(s.usage.recent(usage::now(), 8).is_empty(), "submission alone does not record history");
}

#[test]
fn action_clicks_keep_their_identity_after_filtering_and_ignore_changed_targets() {
    let mut s = search_state();
    let _ = update(&mut s, Message::Query("report".into()));
    let generation = s.generation;
    let _ = update(&mut s, Message::Files { generation, result: Ok(Some(file_hits())) });
    s.cursor.select(1, s.items.len());
    let target = s.selected().unwrap().target.clone();
    let _ = command(&mut s, Command::OpenActions);
    let _ = update(&mut s, Message::ActionQuery("copy".into()));
    let status = s.status.clone();
    let _ = update(&mut s, Message::PanelClicked { target: target.clone(), action: Action::Open });
    assert!(s.opening.is_none(), "a removed action must not run the row that replaced it");
    assert_eq!(s.status, status);
    assert_eq!(s.panel.as_ref().unwrap().query, "copy");

    let _ = update(&mut s, Message::ActionQuery("name".into()));
    let _ = update(&mut s, Message::PanelClicked { target: target.clone(), action: Action::CopyName });
    assert_eq!(s.status, Status::Notice("Copied name".into()));
    assert!(s.panel.is_none());
    assert_eq!(s.query, "report");

    let _ = update(&mut s, Message::Query("report".into()));
    let _ = command(&mut s, Command::OpenActions);
    let status = s.status.clone();
    let _ = update(&mut s, Message::PanelClicked { target, action: Action::CopyName });
    assert_eq!(s.status, status, "an old file click must not copy the newly selected app name");
    assert!(s.panel.is_some());
}

#[test]
fn calculator_secondary_action_is_available_while_filtering_for_copy() {
    let mut panel = Panel::new(&model::calc_item("= 1/3").unwrap(), false);
    panel.filter("copy".into());
    assert_eq!(panel.filtered_actions(), [Action::CopyResult]);
    assert_eq!(panel.action_for_command(Command::Activate), Some(Action::CopyResult));
    assert_eq!(panel.action_for_command(Command::Secondary), Some(Action::UseResult));
    panel.filter("use".into());
    assert_eq!(panel.action_for_command(Command::Activate), Some(Action::UseResult));
    assert_eq!(Action::UseResult.shortcut(), "Ctrl+Enter");
}

fn search_info(state: &str) -> SearchStatus {
    SearchStatus {
        state: state.into(),
        files: 42,
        dirs: 7,
        queued_dirs: 3,
        watch_limit_hit: false,
        errors: 0,
        roots: vec!["/synthetic/Projects:2026".into()],
    }
}

#[test]
fn search_status_refresh_ignores_old_replies_and_preserves_the_search() {
    let mut s = search_state();
    let _ = update(&mut s, Message::Query("report".into()));
    let query_generation = s.generation;
    let selected = s.selected().unwrap().target.clone();
    let _ = command(&mut s, Command::OpenActions);
    let _ = update(&mut s, Message::ShowSearchStatus);
    assert!(s.panel.is_none());
    assert_eq!(s.search_status, Some(SearchStatusPanel::Loading));
    let first = s.status_generation;
    let _ = update(&mut s, Message::RefreshSearchStatus);
    assert_eq!(s.status_generation, first, "only one status request may be in flight");
    let _ = update(&mut s, Message::SearchStatusLoaded { generation: first, result: Ok(search_info("scanning")) });
    assert_eq!(s.search_status, Some(SearchStatusPanel::Ready(search_info("scanning"))));
    let _ = update(&mut s, Message::RefreshSearchStatus);
    let second = s.status_generation;
    assert!(second > first);
    let _ = update(&mut s, Message::SearchStatusLoaded { generation: first, result: Err("old failure".into()) });
    assert_eq!(s.search_status, Some(SearchStatusPanel::Loading));
    let _ = command(&mut s, Command::Back);
    assert!(s.search_status.is_none());
    let _ = update(&mut s, Message::SearchStatusLoaded { generation: second, result: Ok(search_info("idle")) });
    assert!(s.search_status.is_none());
    let _ = update(&mut s, Message::ShowSearchStatus);
    let third = s.status_generation;
    let _ = update(&mut s, Message::SearchStatusLoaded { generation: second, result: Ok(search_info("paused")) });
    assert_eq!(s.search_status, Some(SearchStatusPanel::Loading));
    let _ = update(&mut s, Message::SearchStatusLoaded { generation: third, result: Ok(search_info("idle")) });
    let _ = update(&mut s, Message::CloseSearchStatus);
    assert_eq!(s.query, "report");
    assert_eq!(s.scope, Scope::All);
    assert_eq!(s.generation, query_generation);
    assert_eq!(s.selected().unwrap().target, selected);
}

#[test]
fn search_status_blocks_queued_result_actions_and_scope_navigation() {
    let mut s = search_state();
    // A misrouted activation must never spawn an external program in this test.
    s.apps[0].exec.clear();
    let _ = update(&mut s, Message::Query("report".into()));
    let target = s.selected().unwrap().target.clone();
    let generation = s.generation;
    let search_status = s.status.clone();
    let _ = update(&mut s, Message::ShowSearchStatus);
    for message in [
        Message::Query("changed".into()),
        Message::Scope(Scope::Files),
        Message::Selected(target.clone()),
        Message::Clicked(target.clone()),
        Message::ContextMenu(target.clone()),
        Message::PanelClicked { target, action: Action::Launch },
        Message::Wheel(-1.0),
    ] {
        let _ = update(&mut s, message);
    }
    for cmd in [
        Command::Activate,
        Command::ActivateVisible(0),
        Command::Secondary,
        Command::NextScope,
        Command::PreviousScope,
        Command::CopyPath,
        Command::OpenActions,
    ] {
        let _ = command(&mut s, cmd);
    }
    assert_eq!(s.query, "report");
    assert_eq!(s.scope, Scope::All);
    assert_eq!(s.cursor.selected, 0);
    assert!(s.panel.is_none());
    assert!(s.opening.is_none());
    assert_eq!(s.status, search_status);
    // A current file reply can finish without replacing the status view.
    let _ = update(&mut s, Message::Files { generation, result: Ok(Some(file_hits())) });
    assert_eq!(s.search_status, Some(SearchStatusPanel::Loading));
    assert_eq!(s.files.len(), 2);
}

#[test]
fn open_with_filters_choices_and_dispatches_the_captured_file() {
    let mut s = search_state();
    let _ = update(&mut s, Message::Scope(Scope::Files));
    let _ = update(&mut s, Message::Query("report".into()));
    let search_generation = s.generation;
    let files = file_hits();
    let captured = files[0].clone();
    let _ = update(&mut s, Message::Files { generation: search_generation, result: Ok(Some(files.clone())) });
    let _ = update(&mut s, Message::Selected(Target::File(captured.clone())));
    let _ = command(&mut s, Command::OpenActions);
    let _ = update(&mut s, Message::ActionQuery("open with".into()));
    let _ = command(&mut s, Command::Activate);
    assert!(s.application_picker.is_some());
    assert!(s.opening.is_none());
    let generation = s.application_generation;
    let _ = update(
        &mut s,
        Message::ApplicationsLoaded {
            generation,
            result: Ok(vec![
                crate::backend::ApplicationChoice {
                    id: "one.desktop".into(),
                    name: "Editor One".into(),
                    icon: String::new(),
                },
                crate::backend::ApplicationChoice {
                    id: "two.desktop".into(),
                    name: "Editor Two".into(),
                    icon: String::new(),
                },
            ]),
        },
    );
    let _ = update(&mut s, Message::Selected(Target::File(files[1].clone())));
    let _ = update(&mut s, Message::Query("offscreen input".into()));
    assert_eq!(s.query, "report");
    assert_eq!(s.scope, Scope::Files);
    let _ = update(&mut s, Message::OpenWith { generation, event: crate::open_with::Event::Query("two".into()) });
    let _ = command(&mut s, Command::Activate);
    assert_eq!(s.opening.as_ref(), Some(&(captured.id.clone(), "report".into())));
    assert_eq!(s.usage.boost(Kind::File, &captured.id, "report", usage::now()), 0);
    let _ = update(&mut s, Message::Opened(Ok(())));
    assert!(s.usage.boost(Kind::File, &captured.id, "report", usage::now()) > 0);
}

#[test]
fn file_details_action_is_discoverable_and_preserves_search_on_return() {
    let mut s = search_state();
    let _ = update(&mut s, Message::Scope(Scope::Files));
    let _ = update(&mut s, Message::Query("report".into()));
    let generation = s.generation;
    let _ = update(&mut s, Message::Files { generation, result: Ok(Some(file_hits())) });
    let target = s.selected().unwrap().target.clone();
    let _ = command(&mut s, Command::OpenActions);
    let _ = update(&mut s, Message::ActionQuery("properties".into()));
    let actions = s.panel.as_ref().unwrap().filtered_actions();
    assert_eq!(actions.len(), 1);
    assert_eq!(actions[0].label(), "File details");
    let _ = command(&mut s, Command::Activate);
    let _ = update(&mut s, Message::Query("queued input".into()));
    let _ = update(&mut s, Message::Scope(Scope::All));
    assert_eq!(s.query, "report");
    assert_eq!(s.scope, Scope::Files);
    let _ = command(&mut s, Command::Back);
    assert_eq!(s.selected().unwrap().target, target);
    assert_eq!(s.query, "report");
    assert!(s.usage.recent(usage::now(), 8).is_empty());
}

#[test]
fn details_ignore_stale_replies_and_restore_the_captured_file() {
    let mut s = search_state();
    let _ = update(&mut s, Message::Scope(Scope::Files));
    let _ = update(&mut s, Message::Query("report".into()));
    let search_generation = s.generation;
    let files = file_hits();
    let _ = update(&mut s, Message::Files { generation: search_generation, result: Ok(Some(files.clone())) });
    let captured = s.selected().unwrap().target.clone();
    let _ = s.perform(Action::Details);
    let old = s.details_generation;
    let _ = update(&mut s, Message::RefreshSearchStatus);
    assert_eq!(s.details_generation, old, "one read at a time");
    let _ = update(&mut s, Message::Selected(Target::File(files[1].clone())));
    let _ = update(&mut s, Message::ShowFolders);
    let _ = update(&mut s, Message::ShowSearchStatus);
    let _ = command(&mut s, Command::ActivateVisible(0));
    assert!(s.opening.is_none());
    assert!(s.folder_settings.is_none() && s.search_status.is_none());
    let _ = update(&mut s, Message::DetailsLoaded { generation: old, result: Err("unavailable".into()) });
    let _ = update(&mut s, Message::RefreshSearchStatus);
    let refreshing = s.details_generation;
    assert!(refreshing > old);
    let _ = update(&mut s, Message::DetailsLoaded { generation: old, result: Err("late error".into()) });
    let _ = update(&mut s, Message::RefreshSearchStatus);
    assert_eq!(s.details_generation, refreshing, "late replies must leave the active read loading");
    let _ = command(&mut s, Command::Back);
    let _ = s.perform(Action::Details);
    let current = s.details_generation;
    let _ = update(&mut s, Message::Details { generation: old, event: details::Event::Back });
    let _ = update(&mut s, Message::DetailsLoaded { generation: refreshing, result: Err("closed".into()) });
    assert!(s.file_details.is_some());
    let _ = update(&mut s, Message::RefreshSearchStatus);
    assert_eq!(s.details_generation, current);
    let _ = update(
        &mut s,
        Message::DetailsLoaded {
            generation: current,
            result: Ok(crate::backend::FileDetails { mime: String::new(), size: 0, modified: 0 }),
        },
    );
    let _ = update(
        &mut s,
        Message::Files { generation: search_generation, result: Ok(Some(vec![files[1].clone(), files[0].clone()])) },
    );
    assert!(matches!(s.file_details.as_mut().unwrap().event(details::Event::CopyLocation),
        details::Effect::Copy(path) if path == "/synthetic/report.txt"));
    let _ = command(&mut s, Command::Back);
    assert!(s.file_details.is_none());
    assert_eq!(s.selected().unwrap().target, captured);
    assert_eq!(s.scope, Scope::Files);
    assert_eq!(s.query, "report");
    assert!(s.usage.recent(usage::now(), 8).is_empty());
}

#[test]
fn details_widget_and_subscription_route_keys_to_the_modal_view() {
    use iced::advanced::{Layout, Shell, layout, widget::Tree};
    use iced::keyboard::key::{Code, Physical};
    use iced::{Rectangle, Size};

    let mut s = search_state();
    let _ = update(&mut s, Message::Scope(Scope::Files));
    let _ = update(&mut s, Message::Query("report".into()));
    let generation = s.generation;
    let _ = update(&mut s, Message::Files { generation, result: Ok(Some(file_hits())) });
    let captured = s.selected().unwrap().target.clone();
    let _ = s.perform(Action::Details);
    let initial = s.details_generation;
    let loaded = |generation| Message::DetailsLoaded {
        generation,
        result: Ok(crate::backend::FileDetails { mime: "application/pdf".into(), size: 4096, modified: -1 }),
    };
    let _ = update(&mut s, loaded(initial));
    for (key, code, modifiers) in [
        (Key::Named(keyboard::key::Named::ArrowDown), Code::ArrowDown, Modifiers::empty()),
        (Key::Character("r".into()), Code::KeyR, Modifiers::CTRL),
        (Key::Character("C".into()), Code::KeyC, Modifiers::CTRL | Modifiers::SHIFT),
        (Key::Named(keyboard::key::Named::Escape), Code::Escape, Modifiers::empty()),
    ] {
        let event = Event::Keyboard(keyboard::Event::KeyPressed {
            modified_key: key.clone(),
            key,
            physical_key: Physical::Code(code),
            location: keyboard::Location::Standard,
            modifiers,
            text: None,
            repeat: false,
        });
        let mut messages = {
            let renderer =
                iced::Renderer::Secondary(iced_tiny_skia::Renderer::new(iced::Font::DEFAULT, iced::Pixels(16.0)));
            let mut element = view(&s);
            let widget = element.as_widget_mut();
            let mut tree = Tree::new(&*widget);
            let bounds = Size::new(720.0, 500.0);
            let node = widget.layout(&mut tree, &renderer, &layout::Limits::new(Size::ZERO, bounds));
            let mut messages = Vec::new();
            let mut shell = Shell::new(&mut messages);
            widget.update(
                &mut tree,
                &event,
                Layout::new(&node),
                mouse::Cursor::Unavailable,
                &renderer,
                &mut iced::advanced::clipboard::Null,
                &mut shell,
                &Rectangle::with_size(bounds),
            );
            if let Some(message) = on_event(event, shell.event_status(), iced::window::Id::unique()) {
                messages.push(message);
            }
            messages
        };
        assert_eq!(
            messages.iter().filter(|message| !matches!(message, Message::InputApplied)).count(),
            1,
            "one route per essential keyboard event"
        );
        for message in messages.drain(..) {
            let _ = update(&mut s, message);
        }
        if code == Code::KeyR {
            assert!(s.details_generation > initial);
            let generation = s.details_generation;
            let _ = update(&mut s, loaded(generation));
        }
        assert_eq!(s.query, "report");
        assert_eq!(s.scope, Scope::Files);
        assert!(s.opening.is_none());
        assert!(s.usage.recent(usage::now(), 8).is_empty());
    }
    assert!(s.file_details.is_none());
    assert_eq!(s.selected().unwrap().target, captured);
}

#[test]
fn preview_widget_and_subscription_keep_search_navigation_and_sidebar_scrolling() {
    use iced::advanced::{Layout, Shell, layout, widget::Tree};
    use iced::keyboard::key::{Code, Physical};
    use iced::{Rectangle, Size};

    let mut s = search_state();
    let _ = update(&mut s, Message::Scope(Scope::Files));
    let _ = update(&mut s, Message::Query("report".into()));
    let generation = s.generation;
    let _ = update(&mut s, Message::Files { generation, result: Ok(Some(file_hits())) });
    let captured = s.selected().unwrap().target.clone();
    let _ = s.perform(Action::Preview);
    let initial = s.preview_generation;
    let loaded = |generation| Message::PreviewLoaded { generation, result: preview_data("synthetic text") };
    let _ = update(&mut s, loaded(initial));
    for (key, code, modifiers) in [
        (Key::Named(keyboard::key::Named::PageDown), Code::PageDown, Modifiers::ALT),
        (Key::Character("r".into()), Code::KeyR, Modifiers::CTRL),
        (Key::Character("C".into()), Code::KeyC, Modifiers::CTRL | Modifiers::SHIFT),
        (Key::Named(keyboard::key::Named::Escape), Code::Escape, Modifiers::empty()),
    ] {
        let event = Event::Keyboard(keyboard::Event::KeyPressed {
            modified_key: key.clone(),
            key,
            physical_key: Physical::Code(code),
            location: keyboard::Location::Standard,
            modifiers,
            text: None,
            repeat: false,
        });
        let mut messages = {
            let renderer =
                iced::Renderer::Secondary(iced_tiny_skia::Renderer::new(iced::Font::DEFAULT, iced::Pixels(16.0)));
            let mut element = view(&s);
            let widget = element.as_widget_mut();
            let mut tree = Tree::new(&*widget);
            let bounds = Size::new(PREVIEW_WIDTH as f32, full_height() as f32);
            let node = widget.layout(&mut tree, &renderer, &layout::Limits::new(Size::ZERO, bounds));
            widget.operate(
                &mut tree,
                Layout::new(&node),
                &renderer,
                &mut iced::advanced::widget::operation::focusable::focus::<()>(search_id()),
            );
            let mut messages = Vec::new();
            let mut shell = Shell::new(&mut messages);
            widget.update(
                &mut tree,
                &event,
                Layout::new(&node),
                mouse::Cursor::Unavailable,
                &renderer,
                &mut iced::advanced::clipboard::Null,
                &mut shell,
                &Rectangle::with_size(bounds),
            );
            if let Some(message) = on_event(event, shell.event_status(), iced::window::Id::unique()) {
                messages.push(message);
            }
            messages
        };
        assert_eq!(
            messages.iter().filter(|message| !matches!(message, Message::InputApplied)).count(),
            1,
            "one route per essential keyboard event"
        );
        for message in messages.drain(..) {
            let _ = update(&mut s, message);
        }
        if code == Code::KeyR {
            assert!(s.preview_generation > initial);
            let generation = s.preview_generation;
            let _ = update(&mut s, loaded(generation));
        }
        assert_eq!(s.query, "report");
        assert_eq!(s.scope, Scope::Files);
        assert!(s.opening.is_none());
        assert!(s.usage.recent(usage::now(), 8).is_empty());
    }
    assert!(s.text_preview.is_none());
    assert_eq!(s.selected().unwrap().target, captured);
}

#[test]
fn details_copy_non_utf8_locations_as_lossless_file_urls() {
    use std::os::unix::ffi::OsStringExt;
    let mut file = file_hits().remove(0);
    file.id = "file:///synthetic/non%FF.pdf".into();
    file.path = Some(std::ffi::OsString::from_vec(b"/synthetic/non\xff.pdf".to_vec()).into());
    let mut panel = details::Panel::new(file.clone());
    assert!(matches!(panel.command(Command::CopyPath), details::Effect::Copy(url) if url == file.id));
}

#[test]
fn search_in_folder_action_preserves_the_original_query_scope_and_target() {
    let mut s = search_state();
    let _ = update(&mut s, Message::Query("report".into()));
    let mut folder = file_hits().remove(0);
    folder.id = "file:///synthetic/Reports".into();
    folder.path = Some("/synthetic/Reports".into());
    folder.name = "Reports".into();
    folder.is_dir = true;
    let generation = s.generation;
    let _ = update(&mut s, Message::Files { generation, result: Ok(Some(vec![folder.clone()])) });
    let target = Target::File(folder);
    s.cursor.select_target(&target, &s.items);
    let _ = command(&mut s, Command::OpenActions);
    let _ = update(&mut s, Message::ActionQuery("within".into()));
    let actions = s.panel.as_ref().unwrap().filtered_actions();
    assert_eq!(actions.len(), 1);
    assert_eq!(actions[0].label(), "Browse folder");
    let _ = command(&mut s, Command::Activate);
    assert_eq!(s.scope, Scope::Files);
    assert!(s.query.is_empty());
    assert!(s.items.is_empty(), "global suggestions must not appear inside a folder");
    let _ = update(&mut s, Message::Query("nested".into()));
    let _ = command(&mut s, Command::Back);
    assert!(s.query.is_empty());
    let _ = command(&mut s, Command::Back);
    assert_eq!(s.query, "report");
    assert_eq!(s.scope, Scope::All);
    assert_eq!(s.selected().unwrap().target, target);
    assert!(s.usage.recent(usage::now(), 8).is_empty());
}

fn search_folder_hit(path: &str) -> FileHit {
    FileHit {
        id: format!("file://{path}"),
        path: Some(path.into()),
        name: std::path::Path::new(path).file_name().unwrap().to_string_lossy().into(),
        icon: "folder".into(),
        subtext: "/synthetic".into(),
        is_dir: true,
    }
}

#[test]
fn nested_folder_search_rejects_stale_replies_and_back_buttons() {
    let mut s = search_state();
    let _ = update(&mut s, Message::Query("report".into()));
    let root_generation = s.generation;
    let root_input = s.scope_generation;
    let parent = search_folder_hit("/synthetic/Reports");
    let _ = update(&mut s, Message::Files { generation: root_generation, result: Ok(Some(vec![parent.clone()])) });
    let root_target = Target::File(parent.clone());
    s.cursor.select_target(&root_target, &s.items);
    let _ = command(&mut s, Command::BrowseFolder);
    let parent_back = s.scope_generation;
    let _ = update(&mut s, Message::ContextQuery { generation: root_input, query: "late global input".into() });
    assert!(s.query.is_empty());
    let _ = update(&mut s, Message::Files { generation: root_generation, result: Ok(Some(file_hits())) });
    assert!(s.items.is_empty());
    let _ = update(&mut s, Message::Scope(Scope::All));
    let _ = command(&mut s, Command::NextScope);
    assert_eq!(s.scope, Scope::Files);
    let _ = update(&mut s, Message::Query("=2+3".into()));
    assert!(s.items.is_empty(), "folder searches cannot become calculator queries");
    let parent_query = s.generation;
    let child = search_folder_hit("/synthetic/Reports/Child");
    let _ = update(&mut s, Message::Files { generation: parent_query, result: Ok(Some(vec![child.clone()])) });
    let _ = command(&mut s, Command::BrowseFolder);
    assert_eq!(s.search_folder.as_ref(), Some(&child));
    let _ = update(&mut s, Message::FolderBack { generation: parent_back });
    assert_eq!(s.search_folder.as_ref(), Some(&child));
    let _ = update(&mut s, Message::Files { generation: parent_query, result: Ok(Some(file_hits())) });
    assert!(s.items.is_empty());
    let _ = update(&mut s, Message::Query("child".into()));
    let old = s.generation;
    let _ = update(&mut s, Message::Files { generation: old, result: Err("unavailable".into()) });
    let _ = update(&mut s, Message::RefreshSearchStatus);
    let pending_child = s.generation;
    assert!(pending_child > old);
    let _ = update(&mut s, Message::Files { generation: old, result: Err("late".into()) });
    assert_eq!(s.status, Status::Searching);
    let _ = command(&mut s, Command::BackFolder);
    assert_eq!(s.search_folder.as_ref(), Some(&parent));
    assert_eq!(s.scope, Scope::Files);
    let _ = update(
        &mut s,
        Message::ContextQuery { generation: parent_back, query: "input from departed parent view".into() },
    );
    assert_eq!(s.query, "=2+3");
    assert_eq!(s.selected().unwrap().target, Target::File(child));
    let _ = update(&mut s, Message::Files { generation: pending_child, result: Ok(Some(file_hits())) });
    assert_eq!(s.items.len(), 1);
    let _ = command(&mut s, Command::BackFolder);
    assert!(s.search_folder.is_none());
    assert_eq!(s.scope, Scope::All);
    assert_eq!(s.query, "report");
    assert_eq!(s.selected().unwrap().target, root_target);
    assert!(s.usage.recent(usage::now(), 8).is_empty());
}

#[test]
fn applying_folder_configuration_restores_the_original_global_query() {
    let mut s = search_state();
    let _ = update(&mut s, Message::Query("report".into()));
    let generation = s.generation;
    let _ =
        update(&mut s, Message::Files { generation, result: Ok(Some(vec![search_folder_hit("/synthetic/Reports")])) });
    s.cursor.select_target(&Target::File(search_folder_hit("/synthetic/Reports")), &s.items);
    let _ = command(&mut s, Command::BrowseFolder);
    let _ = update(&mut s, Message::Query("local query".into()));
    let stale = s.generation;
    let _ = s.finish_folders(Task::none(), folders::Effect::Applied);
    assert!(s.search_folder.is_none() && s.folder_search_history.is_empty());
    assert_eq!(s.query, "report");
    assert_eq!(s.scope, Scope::All);
    let _ = update(&mut s, Message::Files { generation: stale, result: Ok(Some(file_hits())) });
    assert!(s.files.is_empty());
}

#[test]
fn empty_folder_queries_do_not_show_global_recent_files() {
    let dir = tempfile::tempdir().unwrap();
    let file = dir.path().join("global-recent.txt");
    std::fs::write(&file, "fixture").unwrap();
    let mut s = search_state();
    s.usage.record(Kind::File, &format!("file://{}", file.display()), "", usage::now());
    let _ = update(&mut s, Message::Scope(Scope::Files));
    let _ = update(&mut s, Message::Query(String::new()));
    assert!(!s.items.is_empty());
    let _ = s.enter_folder_search(search_folder_hit("/synthetic/Reports"));
    assert!(s.items.is_empty());
    let generation = s.generation;
    let _ = update(&mut s, Message::Files { generation, result: Ok(Some(Vec::new())) });
    assert!(s.items.is_empty());
}

#[test]
fn folder_search_keys_pass_through_the_focused_widget_and_subscription() {
    use iced::advanced::{Layout, Shell, layout, widget::Tree};
    use iced::keyboard::key::{Code, Physical};
    use iced::{Rectangle, Size};
    let mut s = search_state();
    let _ = update(&mut s, Message::Query("report".into()));
    let folder = search_folder_hit("/synthetic/Reports");
    let target = Target::File(folder.clone());
    let generation = s.generation;
    let _ = update(&mut s, Message::Files { generation, result: Ok(Some(vec![folder])) });
    s.cursor.select_target(&target, &s.items);
    let mut departed_root_input = None;
    let mut departed_child_input = None;
    for (key, code, modifiers, typed) in [
        (Key::Character("x".into()), Code::KeyX, Modifiers::empty(), Some("x")),
        (Key::Named(keyboard::key::Named::ArrowRight), Code::ArrowRight, Modifiers::CTRL, None),
        (Key::Named(keyboard::key::Named::ArrowRight), Code::ArrowRight, Modifiers::ALT, None),
        (Key::Character("n".into()), Code::KeyN, Modifiers::empty(), Some("n")),
        (Key::Character("x".into()), Code::KeyX, Modifiers::empty(), Some("x")),
        (Key::Character("r".into()), Code::KeyR, Modifiers::CTRL, None),
        (Key::Named(keyboard::key::Named::ArrowLeft), Code::ArrowLeft, Modifiers::ALT, None),
    ] {
        let event = Event::Keyboard(keyboard::Event::KeyPressed {
            modified_key: key.clone(),
            key,
            physical_key: Physical::Code(code),
            location: keyboard::Location::Standard,
            modifiers,
            text: typed.map(Into::into),
            repeat: false,
        });
        let messages = {
            let renderer =
                iced::Renderer::Secondary(iced_tiny_skia::Renderer::new(iced::Font::DEFAULT, iced::Pixels(16.0)));
            let mut element = view(&s);
            let widget = element.as_widget_mut();
            let mut tree = Tree::new(&*widget);
            let bounds = Size::new(720.0, 500.0);
            let node = widget.layout(&mut tree, &renderer, &layout::Limits::new(Size::ZERO, bounds));
            let layout = Layout::new(&node);
            widget.operate(
                &mut tree,
                layout,
                &renderer,
                &mut iced::advanced::widget::operation::focusable::focus::<()>(search_id()),
            );
            let mut messages = Vec::new();
            let mut shell = Shell::new(&mut messages);
            widget.update(
                &mut tree,
                &Event::Keyboard(keyboard::Event::ModifiersChanged(modifiers)),
                layout,
                mouse::Cursor::Unavailable,
                &renderer,
                &mut iced::advanced::clipboard::Null,
                &mut shell,
                &Rectangle::with_size(bounds),
            );
            widget.update(
                &mut tree,
                &event,
                layout,
                mouse::Cursor::Unavailable,
                &renderer,
                &mut iced::advanced::clipboard::Null,
                &mut shell,
                &Rectangle::with_size(bounds),
            );
            if let Some(message) = on_event(event, shell.event_status(), iced::window::Id::unique()) {
                messages.push(message);
            }
            messages
        };
        if code == Code::ArrowRight && modifiers == Modifiers::CTRL {
            assert!(messages.is_empty(), "word navigation must not enter a folder");
            assert!(s.search_folder.is_none());
            assert_eq!(s.query, "report");
            continue;
        }
        assert_eq!(messages.iter().filter(|message| !matches!(message, Message::InputApplied)).count(), 1);
        if code == Code::KeyX {
            if s.search_folder.is_none() {
                departed_root_input = messages.into_iter().next();
            } else {
                departed_child_input = messages.into_iter().next();
            }
            continue;
        }
        for message in messages {
            let _ = update(&mut s, message);
        }
        if code == Code::ArrowRight {
            let _ = update(&mut s, departed_root_input.take().unwrap());
            assert!(s.query.is_empty() && s.search_folder.is_some());
        }
        if code == Code::ArrowLeft {
            let _ = update(&mut s, departed_child_input.take().unwrap());
        }
        if code == Code::KeyN || code == Code::KeyR {
            assert_eq!(s.query, "n");
        }
        assert!(s.opening.is_none());
    }
    assert!(s.search_folder.is_none());
    assert_eq!(s.query, "report");
    assert_eq!(s.scope, Scope::All);
    assert_eq!(s.selected().unwrap().target, target);
    assert!(s.usage.recent(usage::now(), 8).is_empty());
}

#[test]
fn preview_action_preserves_search_and_allows_editing_without_recording_a_launch() {
    let mut s = search_state();
    let _ = update(&mut s, Message::Scope(Scope::Files));
    let _ = update(&mut s, Message::Query("report".into()));
    let generation = s.generation;
    let _ = update(&mut s, Message::Files { generation, result: Ok(Some(file_hits())) });
    assert_eq!(s.preview_generation, 0);
    let _ = command(&mut s, Command::OpenActions);
    let _ = update(&mut s, Message::ActionQuery("preview".into()));
    assert_eq!(s.panel.as_ref().unwrap().filtered_actions(), [Action::Preview]);
    let _ = command(&mut s, Command::Activate);
    assert!(s.preview_enabled && s.preview_visible());
    assert_eq!(s.window_width, PREVIEW_WIDTH);
    assert_eq!(s.query, "report");
    let _ = update(&mut s, Message::Query("quarterly".into()));
    let _ = update(&mut s, Message::Scope(Scope::All));
    assert_eq!(s.query, "quarterly");
    assert_eq!(s.scope, Scope::All);
    assert!(s.text_preview.is_none(), "a new query clears the old text immediately");
    let _ = command(&mut s, Command::Back);
    assert!(!s.preview_enabled);
    assert_eq!(s.query, "quarterly", "Esc hides the sidebar before clearing the query");
    assert_eq!(s.window_width, WIDTH);
    assert!(s.opening.is_none());
    assert!(s.usage.recent(usage::now(), 8).is_empty());
}

#[test]
fn preview_follows_selection_and_rejects_stale_content_and_departed_widgets() {
    let mut s = search_state();
    let _ = update(&mut s, Message::Scope(Scope::Files));
    let _ = update(&mut s, Message::Query("report".into()));
    let search = s.generation;
    let files = file_hits();
    let _ = update(&mut s, Message::Files { generation: search, result: Ok(Some(files.clone())) });
    let _ = s.perform(Action::Preview);
    let old = s.preview_generation;
    let _ = command(&mut s, Command::Down);
    let current = s.preview_generation;
    assert!(current > old);
    assert_eq!(s.text_preview.as_ref().unwrap().file.id, files[1].id);
    for message in [
        Message::PreviewLoaded { generation: old, result: Err("late old file".into()) },
        Message::Preview { generation: old, event: preview::Event::Back },
        Message::Preview { generation: old, event: preview::Event::Refresh },
    ] {
        let _ = update(&mut s, message);
    }
    let _ = update(&mut s, Message::RefreshSearchStatus);
    assert_eq!(s.preview_generation, current, "stale replies leave the current read loading");
    let _ = update(&mut s, Message::PreviewLoaded { generation: current, result: Err("unavailable".into()) });
    let _ = update(&mut s, Message::RefreshSearchStatus);
    let refreshing = s.preview_generation;
    assert!(refreshing > current);
    let _ = update(
        &mut s,
        Message::Files { generation: search, result: Ok(Some(vec![files[1].clone(), files[0].clone()])) },
    );
    assert_eq!(s.preview_generation, refreshing, "a stable target does not trigger another read");
    assert!(
        matches!(s.text_preview.as_mut().unwrap().event(preview::Event::CopyLocation), preview::Effect::Copy(path) if path == "/synthetic/report.pdf")
    );
    let _ = command(&mut s, Command::Back);
    let stopped = s.preview_generation;
    let _ = command(&mut s, Command::Down);
    let _ = update(&mut s, Message::PreviewLoaded { generation: refreshing, result: preview_data("closed") });
    assert!(!s.preview_enabled && s.text_preview.is_none());
    assert_eq!(s.preview_generation, stopped, "disabled preview never reads on navigation");
    assert_eq!(s.query, "report");
    assert!(s.usage.recent(usage::now(), 8).is_empty());
}

#[test]
fn preview_from_details_returns_to_results_with_the_captured_file_selected() {
    let mut s = search_state();
    let _ = update(&mut s, Message::Scope(Scope::Files));
    let _ = update(&mut s, Message::Query("report".into()));
    let generation = s.generation;
    let _ = update(&mut s, Message::Files { generation, result: Ok(Some(file_hits())) });
    let target = s.selected().unwrap().target.clone();
    let _ = s.perform(Action::Details);
    let details = s.details_generation;
    let _ = command(&mut s, Command::Activate);
    assert!(s.preview_visible() && s.file_details.is_none());
    assert_eq!(s.selected().unwrap().target, target);
    let _ = update(&mut s, Message::Details { generation: details, event: details::Event::Back });
    assert!(s.preview_visible(), "the departed details button cannot hide the sidebar");
    let _ = command(&mut s, Command::Back);
    assert!(!s.preview_enabled && s.file_details.is_none());
    assert_eq!(s.query, "report");
    assert!(s.opening.is_none());
}

#[test]
fn preview_keeps_normal_open_actions_and_pauses_reads_behind_modal_screens() {
    let mut s = search_state();
    let _ = update(&mut s, Message::Scope(Scope::Files));
    let _ = update(&mut s, Message::Query("report".into()));
    let generation = s.generation;
    let files = file_hits();
    let _ = update(&mut s, Message::Files { generation, result: Ok(Some(files.clone())) });
    let _ = s.perform(Action::Preview);
    let read = s.preview_generation;
    let _ = update(&mut s, Message::PreviewLoaded { generation: read, result: preview_data("ready") });
    let _ = update(&mut s, Message::ShowSearchStatus);
    assert!(!s.preview_visible());
    let paused = s.preview_generation;
    let _ = update(&mut s, Message::ResultsWheel(-1.0));
    assert_eq!(s.preview_generation, paused);
    let _ = update(&mut s, Message::CloseSearchStatus);
    assert!(s.preview_visible());
    assert_eq!(s.preview_generation, paused, "returning to the same target reuses its completed data");
    let _ = update(&mut s, Message::Selected(Target::File(files[1].clone())));
    assert_eq!(s.text_preview.as_ref().unwrap().file.id, files[1].id);
    let _ = command(&mut s, Command::Activate);
    assert_eq!(s.opening.as_ref().unwrap().0, files[1].id);
    assert!(s.usage.recent(usage::now(), 8).is_empty(), "only confirmed launches record history");
}

#[test]
fn an_explicit_preview_follows_keyboard_selection_without_leaving_the_results() {
    let mut s = search_state();
    let _ = update(&mut s, Message::Scope(Scope::Files));
    let _ = update(&mut s, Message::Query("report".into()));
    let generation = s.generation;
    let files = file_hits();
    let _ = update(&mut s, Message::Files { generation, result: Ok(Some(files.clone())) });
    assert!(s.text_preview.is_none());
    let _ = s.perform(Action::Preview);
    let old = s.preview_generation;
    let _ = command(&mut s, Command::Down);
    assert_eq!(s.selected().unwrap().target, Target::File(files[1].clone()));
    assert_eq!(s.text_preview.as_ref().unwrap().file.id, files[1].id);
    assert!(s.preview_generation > old);
    assert_eq!(s.query, "report");
    assert!(s.opening.is_none());
    assert!(s.usage.recent(usage::now(), 8).is_empty());
}

fn pointer_messages(s: &Launcher, point: iced::Point, events: Vec<mouse::Event>) -> Vec<Message> {
    use iced::advanced::{Layout, Shell, layout, widget::Tree};
    use iced::{Rectangle, Size};
    let renderer = iced::Renderer::Secondary(iced_tiny_skia::Renderer::new(iced::Font::DEFAULT, iced::Pixels(16.0)));
    let mut element = view(s);
    let widget = element.as_widget_mut();
    let mut tree = Tree::new(&*widget);
    let bounds = Size::new(s.window_width as f32, full_height() as f32);
    let node = widget.layout(&mut tree, &renderer, &layout::Limits::new(Size::ZERO, bounds));
    let mut messages = Vec::new();
    for event in events {
        let event = Event::Mouse(event);
        let status = {
            let mut shell = Shell::new(&mut messages);
            widget.update(
                &mut tree,
                &event,
                Layout::new(&node),
                mouse::Cursor::Available(point),
                &renderer,
                &mut iced::advanced::clipboard::Null,
                &mut shell,
                &Rectangle::with_size(bounds),
            );
            shell.event_status()
        };
        if let Some(message) = on_event(event, status, iced::window::Id::unique()) {
            messages.push(message);
        }
    }
    // These fixtures assert domain actions; full cached-tree tests above
    // also apply and verify the internal input acknowledgement.
    messages.retain(|message| !matches!(message, Message::InputApplied));
    messages
}

#[test]
fn sidebar_pointer_and_wheel_events_keep_text_and_result_navigation_separate() {
    let mut s = search_state();
    let _ = update(&mut s, Message::Scope(Scope::Files));
    let _ = update(&mut s, Message::Query("report".into()));
    let generation = s.generation;
    let files = file_hits();
    let _ = update(&mut s, Message::Files { generation, result: Ok(Some(files.clone())) });
    let _ = s.perform(Action::Preview);
    let read = s.preview_generation;
    let _ = update(
        &mut s,
        Message::PreviewLoaded { generation: read, result: preview_data(&"synthetic line\n".repeat(100)) },
    );
    let wheel = || mouse::Event::WheelScrolled { delta: ScrollDelta::Lines { x: 0.0, y: -1.0 } };
    let messages = pointer_messages(&s, iced::Point::new(650.0, 200.0), vec![wheel()]);
    assert!(messages.is_empty(), "the text scrollable captures its wheel without changing selection");
    assert_eq!(s.selected().unwrap().target, Target::File(files[0].clone()));
    let messages = pointer_messages(&s, iced::Point::new(120.0, 100.0), vec![wheel()]);
    assert!(matches!(&messages[..], [Message::ResultsWheel(y)] if *y == -1.0), "one route per list wheel event");
    for message in messages {
        let _ = update(&mut s, message);
    }
    assert_eq!(s.selected().unwrap().target, Target::File(files[1].clone()));
    assert_eq!(s.text_preview.as_ref().unwrap().file.id, files[1].id);
    let messages = pointer_messages(
        &s,
        // The empty inset around the first row remains part of its hit target.
        iced::Point::new(PAD + 1.0, PAD + INPUT_HEIGHT + 1.0 + PAD / 2.0 + 1.0),
        vec![mouse::Event::ButtonPressed(mouse::Button::Left), mouse::Event::ButtonReleased(mouse::Button::Left)],
    );
    assert!(messages.iter().any(|message| matches!(message, Message::Selected(_))));
    for message in messages {
        let _ = update(&mut s, message);
    }
    assert_eq!(s.selected().unwrap().target, Target::File(files[0].clone()));
    assert_eq!(s.text_preview.as_ref().unwrap().file.id, files[0].id);
    assert!(s.opening.is_none(), "one click selects without opening");
    assert!(s.usage.recent(usage::now(), 8).is_empty());
}

#[test]
fn action_panel_owns_wheel_events_with_and_without_a_preview() {
    for preview in [false, true] {
        let mut s = search_state();
        let _ = update(&mut s, Message::Scope(Scope::Files));
        let _ = update(&mut s, Message::Query("report".into()));
        let generation = s.generation;
        let files = file_hits();
        let _ = update(&mut s, Message::Files { generation, result: Ok(Some(files.clone())) });
        if preview {
            let _ = s.perform(Action::Preview);
            let generation = s.preview_generation;
            let _ = update(
                &mut s,
                Message::PreviewLoaded { generation, result: preview_data(&"synthetic line\n".repeat(100)) },
            );
        }
        let _ = command(&mut s, Command::OpenActions);
        let _ = update(&mut s, Message::ActionQuery("copy".into()));
        let generation = s.preview_generation;
        for (label, delta, expected) in [
            ("Copy name", ScrollDelta::Lines { x: 0.0, y: -1.0 }, Action::CopyName),
            ("report.txt", ScrollDelta::Pixels { x: 0.0, y: ROW_HEIGHT }, Action::CopyPath),
            ("Run", ScrollDelta::Lines { x: 1.0, y: 0.0 }, Action::CopyPath),
        ] {
            let labels = rendered_labels(view(&s), iced::Size::new(s.window_width as f32, full_height() as f32));
            let (_, label) = labels.text.iter().rev().find(|(value, _)| value == label).unwrap();
            let messages = pointer_messages(&s, label.center(), vec![mouse::Event::WheelScrolled { delta }]);
            assert!(matches!(&messages[..], [Message::Wheel(_)]), "preview={preview}: {messages:?}");
            for message in messages {
                let _ = update(&mut s, message);
            }
            let panel = s.panel.as_ref().unwrap();
            assert_eq!(panel.action_for_command(Command::Activate), Some(expected), "preview={preview}");
            assert_eq!(panel.query, "copy");
        }
        assert_eq!(s.preview_generation, generation);
        assert_eq!(s.selected().unwrap().target, Target::File(files[0].clone()));
        assert_eq!(s.query, "report");
        assert!(s.opening.is_none());
        let _ = command(&mut s, Command::Back);
        assert!(s.panel.as_ref().unwrap().query.is_empty());
        let _ = command(&mut s, Command::Back);
        assert!(s.panel.is_none());
        assert_eq!(s.preview_visible(), preview);
    }
}

#[test]
fn hiding_or_disabling_preview_invalidates_pending_bus_requests() {
    let mut s = search_state();
    let _ = update(&mut s, Message::Scope(Scope::Files));
    let _ = update(&mut s, Message::Query("report".into()));
    let generation = s.generation;
    let _ = update(&mut s, Message::Files { generation, result: Ok(Some(file_hits())) });
    let _ = s.perform(Action::Preview);
    let first = s.preview_request.clone().unwrap();
    let _ = command(&mut s, Command::Down);
    assert!(!first.is_active(), "old queued selections cannot start later");
    let second = s.preview_request.clone().unwrap();
    let old = s.preview_generation;
    let _ = update(&mut s, Message::ShowSearchStatus);
    assert!(!second.is_active());
    assert!(s.text_preview.is_none());
    let _ = update(&mut s, Message::PreviewLoaded { generation: old, result: preview_data("hidden") });
    assert!(s.text_preview.is_none());
    let _ = update(&mut s, Message::CloseSearchStatus);
    let resumed = s.preview_request.clone().unwrap();
    assert!(resumed.is_active());
    let _ = command(&mut s, Command::Back);
    assert!(!resumed.is_active());
    assert!(!s.preview_enabled && s.text_preview.is_none());
}

#[test]
fn a_new_query_invalidates_text_even_when_home_selects_the_same_file() {
    let temp = tempfile::tempdir().unwrap();
    let path = temp.path().join("report.txt");
    std::fs::write(&path, "synthetic contents").unwrap();
    let file = FileHit {
        id: format!("file://{}", path.display()),
        name: "report.txt".into(),
        path: Some(path),
        icon: String::new(),
        subtext: String::new(),
        is_dir: false,
    };
    let mut s = search_state();
    let _ = update(&mut s, Message::Scope(Scope::Files));
    let _ = update(&mut s, Message::Query("report".into()));
    let generation = s.generation;
    let _ = update(&mut s, Message::Files { generation, result: Ok(Some(vec![file.clone()])) });
    let _ = s.perform(Action::AddFavorite);
    let _ = s.perform(Action::Preview);
    let old = s.preview_generation;
    let _ = update(&mut s, Message::PreviewLoaded { generation: old, result: preview_data("old text") });
    let _ = update(&mut s, Message::Query(String::new()));
    assert!(matches!(&s.selected().unwrap().target, Target::File(selected) if selected.id == file.id));
    let fresh = s.preview_generation;
    assert!(fresh > old);
    assert!(s.preview_request.as_ref().unwrap().is_active());
    let _ = update(&mut s, Message::PreviewLoaded { generation: old, result: preview_data("stale") });
    let _ = update(&mut s, Message::RefreshSearchStatus);
    assert_eq!(s.preview_generation, fresh, "stale content cannot replace the new loading state");
}

#[test]
fn sidebar_toolbar_buttons_enter_refresh_copy_and_hide_through_real_widgets() {
    let mut s = search_state();
    let _ = update(&mut s, Message::Scope(Scope::Files));
    let _ = update(&mut s, Message::Query("report".into()));
    let generation = s.generation;
    let _ = update(&mut s, Message::Files { generation, result: Ok(Some(file_hits())) });
    let _ = s.perform(Action::Details);
    let click = |s: &Launcher, x, y| {
        pointer_messages(
            s,
            iced::Point::new(x, y),
            vec![mouse::Event::ButtonPressed(mouse::Button::Left), mouse::Event::ButtonReleased(mouse::Button::Left)],
        )
    };
    let control = |s: &Launcher, name: &str| {
        let width = if s.preview_visible() { PREVIEW_WIDTH } else { WIDTH };
        let labels = rendered_labels(view_content(s), iced::Size::new(width as f32, full_height() as f32));
        let (_, bounds) = labels.text.iter().find(|(value, _)| value == name).unwrap();
        click(s, bounds.center_x(), bounds.center_y())
    };
    let messages = control(&s, "Preview text");
    assert!(matches!(&messages[..], [Message::Details { event: details::Event::Preview, .. }]));
    for message in messages {
        let _ = update(&mut s, message);
    }
    assert!(s.preview_visible());
    assert!(control(&s, "Refresh").is_empty(), "refresh is disabled while loading");
    let initial = s.preview_generation;
    let _ = update(&mut s, Message::PreviewLoaded { generation: initial, result: preview_data("synthetic text") });
    let messages = control(&s, "Copy path");
    assert!(matches!(&messages[..], [Message::Preview { event: preview::Event::CopyLocation, .. }]));
    for message in messages {
        let _ = update(&mut s, message);
    }
    let messages = control(&s, "Refresh");
    assert!(matches!(&messages[..], [Message::Preview { event: preview::Event::Refresh, .. }]));
    for message in messages {
        let _ = update(&mut s, message);
    }
    assert!(s.preview_generation > initial);
    let pending = s.preview_request.clone().unwrap();
    let messages = control(&s, "Hide");
    assert!(matches!(&messages[..], [Message::Preview { event: preview::Event::Back, .. }]));
    for message in messages {
        let _ = update(&mut s, message);
    }
    assert!(!s.preview_enabled && !pending.is_active());
    let _ = s.perform(Action::Details);
    let messages = control(&s, "Back");
    assert!(matches!(&messages[..], [Message::Details { event: details::Event::Back, .. }]));
    for message in messages {
        let _ = update(&mut s, message);
    }
    assert!(s.file_details.is_none());
    assert_eq!(s.query, "report");
    assert_eq!(s.scope, Scope::Files);
    assert!(s.opening.is_none());
    assert!(s.usage.recent(usage::now(), 8).is_empty());
}

fn browser_state() -> (Launcher, FileHit) {
    let mut s = search_state();
    let _ = update(&mut s, Message::Scope(Scope::Files));
    let _ = update(&mut s, Message::Query("report".into()));
    let mut folder = file_hits().remove(0);
    folder.id = "file:///synthetic/Reports".into();
    folder.path = Some("/synthetic/Reports".into());
    folder.name = "Reports".into();
    folder.is_dir = true;
    let generation = s.generation;
    let _ = update(&mut s, Message::Files { generation, result: Ok(Some(vec![folder.clone()])) });
    let _ = command(&mut s, Command::BrowseFolder);
    (s, folder)
}

#[test]
fn browsing_rejects_old_lists_and_restores_parent_selection_and_limit_state() {
    let (mut s, folder) = browser_state();
    let generation = s.generation;
    let mut child = folder.clone();
    child.id.push_str("/Nested");
    child.path = Some("/synthetic/Reports/Nested".into());
    child.name = "Nested".into();
    let file = file_hits().remove(0);
    let children = vec![child.clone(), file.clone()];
    let _ = update(&mut s, Message::FolderListed { generation, result: Ok((children.clone(), true)) });
    assert!(s.folder_more);
    assert_eq!(s.selected().unwrap().target, Target::File(child.clone()));
    assert_eq!(s.preview_generation, 0, "listing alone does not read file content");
    let _ = command(&mut s, Command::BrowseFolder);
    let nested = s.generation;
    let _ = update(&mut s, Message::FolderListed { generation: nested, result: Ok((vec![file], false)) });
    let _ = command(&mut s, Command::BackFolder);
    assert_eq!(s.search_folder.as_ref().unwrap().id, folder.id);
    assert_eq!(s.selected().unwrap().target, Target::File(child));
    assert!(s.folder_more);
    let _ = update(&mut s, Message::FolderListed { generation: nested, result: Ok((Vec::new(), false)) });
    assert_eq!(s.items.len(), 2);
    assert!(s.folder_more);
    let _ = command(&mut s, Command::BackFolder);
    assert!(s.search_folder.is_none());
    assert_eq!(s.query, "report");
    assert!(!s.folder_more);
    assert_eq!(s.selected().unwrap().target, Target::File(folder));
    assert!(s.usage.recent(usage::now(), 8).is_empty());
}

#[test]
fn listing_and_recursive_queries_cannot_replace_each_others_results() {
    let (mut s, _) = browser_state();
    let listing = s.generation;
    let _ = update(&mut s, Message::Query("report".into()));
    let recursive = s.generation;
    let _ = update(&mut s, Message::FolderListed { generation: listing, result: Ok((file_hits(), true)) });
    assert!(s.items.is_empty() && !s.folder_more);
    let _ = update(&mut s, Message::Files { generation: recursive, result: Ok(Some(file_hits())) });
    assert_eq!(s.items.len(), 2);
    let _ = update(&mut s, Message::Query(String::new()));
    let current = s.generation;
    let _ = update(&mut s, Message::Files { generation: recursive, result: Ok(Some(file_hits())) });
    assert!(s.items.is_empty());
    let _ = update(&mut s, Message::FolderListed { generation: current, result: Ok((Vec::new(), false)) });
    assert!(s.items.is_empty() && matches!(s.status, Status::Idle));
    assert!(s.search_folder.is_some(), "an empty folder does not fall back to global suggestions");
}

#[test]
fn folder_queries_wait_for_two_characters_before_searching() {
    let (mut s, _) = browser_state();
    for query in ["a", " é ", "日", "🦀"] {
        let _ = update(&mut s, Message::Query("report".into()));
        let previous = s.generation;
        let _ = update(&mut s, Message::Files { generation: previous, result: Ok(Some(file_hits())) });
        assert!(!s.items.is_empty());

        let _ = update(&mut s, Message::Query(query.into()));
        assert_eq!(s.status, Status::Idle, "short query: {query:?}");
        assert!(s.files.is_empty() && s.items.is_empty());
        let current = s.generation;
        let _ = update(&mut s, Message::RefreshSearchStatus);
        assert_eq!(s.generation, current, "refresh must not start a short search");
        assert_eq!(s.status, Status::Idle);

        let _ = update(&mut s, Message::Files { generation: previous, result: Ok(Some(file_hits())) });
        let _ = update(&mut s, Message::FolderListed { generation: previous, result: Ok((file_hits(), true)) });
        assert!(s.items.is_empty() && !s.folder_more);
        assert_eq!(s.status, Status::Idle);
    }

    for query in ["ré", " 日本 ", "=x"] {
        let _ = update(&mut s, Message::Query(query.into()));
        assert_eq!(s.status, Status::Searching, "searchable query: {query:?}");
        let generation = s.generation;
        let _ = update(&mut s, Message::Files { generation, result: Ok(Some(file_hits())) });
        assert_eq!(s.items.len(), 2);
    }

    for query in ["", " \t "] {
        let _ = update(&mut s, Message::Query(query.into()));
        assert_eq!(s.status, Status::Searching);
        let generation = s.generation;
        let _ = update(&mut s, Message::FolderListed { generation, result: Ok((file_hits(), true)) });
        assert_eq!(s.items.len(), 2);
        assert!(s.folder_more && s.search_folder.is_some());
    }
}

#[test]
fn folder_refresh_preserves_selection_and_reloads_an_enabled_preview() {
    let (mut s, _) = browser_state();
    let generation = s.generation;
    let files = file_hits();
    let _ = update(&mut s, Message::FolderListed { generation, result: Ok((files.clone(), false)) });
    let _ = command(&mut s, Command::Down);
    let target = s.selected().unwrap().target.clone();
    let _ = s.perform(Action::Preview);
    let pending = s.preview_request.clone().unwrap();
    let _ = update(&mut s, Message::RefreshSearchStatus);
    assert!(!pending.is_active());
    assert_eq!(s.selected().unwrap().target, target);
    let refresh = s.generation;
    let _ = update(
        &mut s,
        Message::FolderListed { generation: refresh, result: Ok((vec![files[1].clone(), files[0].clone()], false)) },
    );
    assert_eq!(s.selected().unwrap().target, target);
    assert!(s.preview_request.as_ref().unwrap().is_active());
    assert_eq!(s.text_preview.as_ref().unwrap().file.id, files[1].id);
}

#[test]
fn folder_back_button_restores_the_child_and_rejects_a_departed_header_click() {
    let (mut s, folder) = browser_state();
    let mut child = folder.clone();
    child.id.push_str("/Nested");
    child.path = Some("/synthetic/Reports/Nested".into());
    child.name = "Nested".into();
    let generation = s.generation;
    let _ = update(&mut s, Message::FolderListed { generation, result: Ok((vec![child.clone()], false)) });
    let _ = command(&mut s, Command::BrowseFolder);
    let generation = s.generation;
    let _ = update(&mut s, Message::FolderListed { generation, result: Ok((Vec::new(), false)) });
    let departed = s.scope_generation;
    let messages = pointer_messages(
        &s,
        iced::Point::new(70.0, 22.0),
        vec![mouse::Event::ButtonPressed(mouse::Button::Left), mouse::Event::ButtonReleased(mouse::Button::Left)],
    );
    assert!(matches!(&messages[..], [Message::FolderBack { .. }]));
    for message in messages {
        let _ = update(&mut s, message);
    }
    assert_eq!(s.search_folder.as_ref().unwrap().id, folder.id);
    assert_eq!(s.selected().unwrap().target, Target::File(child.clone()));
    let refresh = s.generation;
    let _ = update(&mut s, Message::FolderListed { generation: refresh, result: Ok((vec![child.clone()], false)) });
    let _ = update(&mut s, Message::FolderBack { generation: departed });
    assert_eq!(s.search_folder.as_ref().unwrap().id, folder.id);
    assert_eq!(s.selected().unwrap().target, Target::File(child));
    assert!(s.query.is_empty());
    assert!(s.usage.recent(usage::now(), 8).is_empty());
}

fn preview_data(text: &str) -> Result<crate::backend::PreviewData, String> {
    Ok(crate::backend::PreviewData {
        content: Ok(crate::backend::TextPreview::Text { text: text.into(), truncated: false }),
        details: Ok(crate::backend::FileDetails { mime: "text/plain".into(), size: text.len() as u64, modified: 0 }),
    })
}

fn picker_state() -> Launcher {
    let mut s = search_state();
    let _ = update(&mut s, Message::Scope(Scope::Files));
    let _ = update(&mut s, Message::Query("report".into()));
    let generation = s.generation;
    let _ = update(&mut s, Message::Files { generation, result: Ok(Some(file_hits())) });
    let _ = s.perform(Action::OpenWith);
    s
}

fn picker_choices(count: usize) -> Vec<crate::backend::ApplicationChoice> {
    (0..count)
        .map(|i| crate::backend::ApplicationChoice {
            id: format!("editor-{i:02}.desktop"),
            name: format!("Editor {i:02}"),
            icon: String::new(),
        })
        .collect()
}

#[test]
fn picker_escape_restores_search_and_ignores_closed_or_reopened_generations() {
    let mut s = picker_state();
    let old = s.application_generation;
    let target = s.selected().unwrap().target.clone();
    let _ = update(&mut s, Message::ApplicationsLoaded { generation: old, result: Ok(picker_choices(12)) });
    let _ = update(&mut s, Message::OpenWith { generation: old, event: open_with::Event::Query("07".into()) });
    let _ = command(&mut s, Command::Back);
    assert!(s.application_picker.as_ref().unwrap().query.is_empty());
    let _ = command(&mut s, Command::Back);
    assert!(s.application_picker.is_none());
    assert_eq!(s.query, "report");
    assert_eq!(s.scope, Scope::Files);
    assert_eq!(s.selected().unwrap().target, target);
    let _ = update(&mut s, Message::ApplicationsLoaded { generation: old, result: Err("late".into()) });
    assert!(s.application_picker.is_none());
    let _ = s.perform(Action::OpenWith);
    let current = s.application_generation;
    let _ = update(&mut s, Message::ApplicationsLoaded { generation: old, result: Ok(picker_choices(12)) });
    let _ = update(
        &mut s,
        Message::OpenWith { generation: old, event: open_with::Event::Open("editor-00.desktop".into()) },
    );
    assert!(s.opening.is_none());
    let _ = update(&mut s, Message::ApplicationsLoaded { generation: current, result: Ok(picker_choices(12)) });
    let _ = update(&mut s, Message::OpenWith { generation: current, event: open_with::Event::Query("07".into()) });
    let _ = update(
        &mut s,
        Message::OpenWith { generation: current, event: open_with::Event::Open("editor-00.desktop".into()) },
    );
    assert!(s.opening.is_none(), "a filtered-out pointer choice must not dispatch");
    let _ = command(&mut s, Command::Activate);
    assert!(s.opening.is_some());
}

#[test]
fn picker_back_button_returns_to_the_captured_search_with_a_filter() {
    let mut s = picker_state();
    let target = s.selected().unwrap().target.clone();
    let generation = s.application_generation;
    let _ = update(&mut s, Message::ApplicationsLoaded { generation, result: Ok(picker_choices(3)) });
    let _ = update(&mut s, Message::OpenWith { generation, event: open_with::Event::Query("02".into()) });

    let labels = rendered_labels(view_content(&s), iced::Size::new(WIDTH as f32, full_height() as f32));
    let (_, back) = labels.text.iter().find(|(text, _)| text == "Back").expect("visible Back button");
    let messages = pointer_messages(
        &s,
        iced::Point::new(back.center_x(), back.center_y()),
        vec![mouse::Event::ButtonPressed(mouse::Button::Left), mouse::Event::ButtonReleased(mouse::Button::Left)],
    );
    assert_eq!(messages.len(), 1);
    for message in messages {
        let _ = update(&mut s, message);
    }
    assert_eq!(
        (s.application_picker.is_none(), s.query.as_str(), s.scope, &s.selected().unwrap().target),
        (true, "report", Scope::Files, &target),
    );
}

#[test]
fn picker_reload_keeps_selection_across_read_errors() {
    for failures in [0, 1, 2] {
        let mut s = picker_state();
        let generation = s.application_generation;
        let _ = update(&mut s, Message::ApplicationsLoaded { generation, result: Ok(picker_choices(3)) });
        let _ = command(&mut s, Command::Down);
        for _ in 0..failures {
            let _ = update(&mut s, Message::RefreshSearchStatus);
            let generation = s.application_generation;
            let _ =
                update(&mut s, Message::ApplicationsLoaded { generation, result: Err("service unavailable".into()) });
        }
        let _ = update(&mut s, Message::RefreshSearchStatus);
        let generation = s.application_generation;
        let mut choices = picker_choices(3);
        choices.rotate_right(1);
        let _ = update(&mut s, Message::ApplicationsLoaded { generation, result: Ok(choices) });
        assert!(
            matches!(
                s.application_picker.as_mut().unwrap().command(Command::Activate),
                open_with::Effect::Open(id) if id == "editor-01.desktop"
            ),
            "selection lost after {failures} read errors"
        );
        assert!(s.opening.is_none());
    }
}

#[test]
fn picker_clearing_filter_during_reload_resets_selection() {
    let mut s = picker_state();
    let generation = s.application_generation;
    let _ = update(&mut s, Message::ApplicationsLoaded { generation, result: Ok(picker_choices(3)) });
    let _ = update(&mut s, Message::OpenWith { generation, event: open_with::Event::Query("Editor".into()) });
    let _ = command(&mut s, Command::Down);
    let _ = update(&mut s, Message::OpenWith { generation, event: open_with::Event::Reload });
    let _ = command(&mut s, Command::Back);
    assert!(s.application_picker.as_ref().unwrap().query.is_empty());
    let generation = s.application_generation;
    let _ = update(&mut s, Message::ApplicationsLoaded { generation, result: Ok(picker_choices(3)) });
    assert!(matches!(
        s.application_picker.as_mut().unwrap().command(Command::Activate),
        open_with::Effect::Open(id) if id == "editor-00.desktop"
    ));
}

#[test]
fn picker_read_errors_can_retry_but_unconfirmed_dispatch_cannot_duplicate() {
    let mut s = picker_state();
    let old = s.application_generation;
    let _ = update(&mut s, Message::RefreshSearchStatus);
    assert_eq!(s.application_generation, old, "one list request at a time");
    let _ = update(&mut s, Message::ApplicationsLoaded { generation: old, result: Err("service unavailable".into()) });
    let _ = command(&mut s, Command::Activate);
    assert!(s.opening.is_none());
    let _ = update(&mut s, Message::RefreshSearchStatus);
    let generation = s.application_generation;
    assert!(generation > old);
    let _ = update(&mut s, Message::ApplicationsLoaded { generation: old, result: Ok(picker_choices(2)) });
    let _ = command(&mut s, Command::Activate);
    assert!(s.opening.is_none());
    let _ = update(&mut s, Message::ApplicationsLoaded { generation, result: Ok(Vec::new()) });
    let _ = command(&mut s, Command::Activate);
    assert!(s.opening.is_none());
    let _ = update(&mut s, Message::RefreshSearchStatus);
    let generation = s.application_generation;
    let _ = update(&mut s, Message::ApplicationsLoaded { generation, result: Ok(picker_choices(2)) });
    let _ = command(&mut s, Command::Activate);
    assert!(s.opening.is_some());
    let _ = update(&mut s, Message::Opened(Err(RunError::Unconfirmed("connection lost".into()))));
    assert!(s.unconfirmed_open);
    assert!(s.usage.recent(usage::now(), 8).is_empty());
    let labels = rendered_labels(view_content(&s), iced::Size::new(WIDTH as f32, full_height() as f32));
    assert!(!labels.text.iter().any(|(value, _)| value == "Open"), "hide Open when launch cannot be retried");
    let _ = command(&mut s, Command::Activate);
    assert!(s.opening.is_none());
    let _ = command(&mut s, Command::Back);
    let _ = s.perform(Action::OpenWith);
    assert!(s.application_picker.is_none(), "close and reopen the process before retrying an uncertain launch");
}

#[test]
fn picker_rejected_dispatch_can_choose_another_application() {
    let mut s = picker_state();
    let generation = s.application_generation;
    let _ = update(&mut s, Message::ApplicationsLoaded { generation, result: Ok(picker_choices(2)) });
    let _ = command(&mut s, Command::Activate);
    let (file, query) = s.opening.clone().unwrap();
    let _ = update(&mut s, Message::Opened(Err(RunError::Failed("executable is unavailable".into()))));
    assert!(!s.unconfirmed_open);
    assert!(s.opening.is_none());
    assert_eq!(s.usage.boost(Kind::File, &file, &query, usage::now()), 0);
    let _ = update(&mut s, Message::OpenWith { generation, event: open_with::Event::Query("01".into()) });
    let labels = rendered_labels(view_content(&s), iced::Size::new(WIDTH as f32, full_height() as f32));
    let (_, open) = labels.text.iter().find(|(text, _)| text == "Open").unwrap();
    let messages = pointer_messages(
        &s,
        iced::Point::new(open.center_x(), open.center_y()),
        vec![mouse::Event::ButtonPressed(mouse::Button::Left), mouse::Event::ButtonReleased(mouse::Button::Left)],
    );
    assert!(
        matches!(messages.as_slice(), [Message::OpenWith { event: open_with::Event::Open(id), .. }] if id == "editor-01.desktop")
    );
    for message in messages {
        let _ = update(&mut s, message);
    }
    assert_eq!(s.opening.as_ref(), Some(&(file.clone(), query.clone())));
    assert_eq!(s.application_picker.as_ref().unwrap().query, "01");
    let _ = update(&mut s, Message::Opened(Ok(())));
    assert!(s.usage.boost(Kind::File, &file, &query, usage::now()) > 0);
}

#[test]
fn focused_picker_captures_numbered_keys_before_the_filter_input() {
    use iced::advanced::{Layout, Shell, layout, widget::Tree};
    use iced::keyboard::key::{Code, Physical};
    use iced::{Rectangle, Size};

    for (symbol, code, modifiers, slot) in [
        ("1", Code::Digit1, Modifiers::CTRL, Some(0)),
        ("é", Code::Digit2, Modifiers::CTRL, Some(1)),
        ("7", Code::Digit7, Modifiers::empty(), None),
    ] {
        let mut s = picker_state();
        let generation = s.application_generation;
        let _ = update(&mut s, Message::ApplicationsLoaded { generation, result: Ok(picker_choices(12)) });
        let _ = command(&mut s, Command::Last);
        let messages = {
            let renderer =
                iced::Renderer::Secondary(iced_tiny_skia::Renderer::new(iced::Font::DEFAULT, iced::Pixels(16.0)));
            let mut element = view(&s);
            let widget = element.as_widget_mut();
            let mut tree = Tree::new(&*widget);
            let bounds = Size::new(720.0, 500.0);
            let node = widget.layout(&mut tree, &renderer, &layout::Limits::new(Size::ZERO, bounds));
            let layout = Layout::new(&node);
            widget.operate(
                &mut tree,
                layout,
                &renderer,
                &mut iced::advanced::widget::operation::focusable::focus::<()>(open_with::input_id()),
            );
            let mut messages = Vec::new();
            let mut shell = Shell::new(&mut messages);
            for event in [
                Event::Keyboard(keyboard::Event::ModifiersChanged(modifiers)),
                Event::Keyboard(keyboard::Event::KeyPressed {
                    key: Key::Character(symbol.into()),
                    modified_key: Key::Character(symbol.into()),
                    physical_key: Physical::Code(code),
                    location: keyboard::Location::Standard,
                    modifiers,
                    text: Some(symbol.into()),
                    repeat: false,
                }),
            ] {
                widget.update(
                    &mut tree,
                    &event,
                    layout,
                    mouse::Cursor::Unavailable,
                    &renderer,
                    &mut iced::advanced::clipboard::Null,
                    &mut shell,
                    &Rectangle::with_size(bounds),
                );
            }
            assert_eq!(shell.event_status(), iced::event::Status::Captured);
            messages
        };
        assert_eq!(messages.iter().filter(|message| !matches!(message, Message::InputApplied)).count(), 1);
        match slot {
            Some(slot) => assert!(matches!(&messages[0], Message::Cmd(Command::ActivateVisible(i)) if *i == slot)),
            None => assert!(
                matches!(&messages[0], Message::OpenWith { event: open_with::Event::Query(query), .. } if query == symbol)
            ),
        }
        for message in messages {
            let _ = update(&mut s, message);
        }
        assert_eq!(s.application_picker.as_ref().unwrap().query, if slot.is_some() { "" } else { symbol });
        assert_eq!(s.opening.is_some(), slot.is_some());
        assert_eq!(s.query, "report");
    }
}

#[test]
fn picker_numbered_commands_resolve_current_filtered_and_scrolled_app_ids() {
    let file = file_hits().remove(0);
    let mut panel = open_with::Panel::new(file);
    panel.loaded(Ok(picker_choices(12)), &mut Icons::disabled());
    let _ = panel.command(Command::Last);
    assert!(
        matches!(panel.command(Command::ActivateVisible(0)), open_with::Effect::Open(id) if id == "editor-06.desktop")
    );
    assert!(matches!(panel.command(Command::ActivateVisible(6)), open_with::Effect::None));
    let _ = panel.event(open_with::Event::Query("07".into()));
    assert!(
        matches!(panel.command(Command::ActivateVisible(0)), open_with::Effect::Open(id) if id == "editor-07.desktop")
    );
}

#[test]
fn picker_numbered_choice_remains_selected_after_a_failed_open() {
    for query in ["", "editor 0"] {
        let mut panel = open_with::Panel::new(file_hits().remove(0));
        panel.loaded(Ok(picker_choices(12)), &mut Icons::disabled());
        let _ = panel.event(open_with::Event::Query(query.into()));
        let _ = panel.command(Command::Last);
        let open_with::Effect::Open(chosen) = panel.command(Command::ActivateVisible(0)) else {
            panic!("the first visible application must be available");
        };
        panel.opening();
        panel.failed("Application could not be started".into());
        assert!(
            matches!(panel.command(Command::Activate), open_with::Effect::Open(id) if id == chosen),
            "Enter must retry {chosen} after its numbered launch failed, query: {query:?}"
        );
    }
}

#[test]
fn folder_settings_are_modal_and_discard_late_replies_after_reopening() {
    let mut s = search_state();
    let _ = update(&mut s, Message::Query("report".into()));
    let search_generation = s.generation;
    let _ = update(&mut s, Message::ShowFolders);
    let old = s.folder_generation;
    assert!(s.folder_settings.is_some());
    assert!(!s.compact);
    for message in [Message::Query("changed".into()), Message::Scope(Scope::Files), Message::ShowSearchStatus] {
        let _ = update(&mut s, message);
    }
    let _ = command(&mut s, Command::ActivateVisible(0));
    assert_eq!(s.query, "report");
    assert_eq!(s.scope, Scope::All);
    assert_eq!(s.generation, search_generation);
    assert!(s.opening.is_none());
    let _ = update(&mut s, Message::Files { generation: search_generation, result: Ok(Some(file_hits())) });
    assert!(s.folder_settings.is_some());
    assert_eq!(s.files.len(), 2);
    let _ = command(&mut s, Command::Back);
    assert!(s.folder_settings.is_none());
    assert_eq!(s.query, "report");
    let _ = update(&mut s, Message::ShowFolders);
    let _ = update(&mut s, Message::Folder { generation: old, event: folders::Event::Back });
    assert!(s.folder_settings.is_some(), "a late event cannot close a reopened settings view");
}

#[test]
fn focus_loss_closes_unless_an_open_folder_settings_or_a_copy_must_finish() {
    fn exits(task: Task<Message>) -> bool {
        let actions = iced::futures::executor::block_on(async {
            match iced_runtime::task::into_stream(task) {
                Some(stream) => stream.collect::<Vec<_>>().await,
                None => Vec::new(),
            }
        });
        actions.iter().any(|action| matches!(action, iced_runtime::Action::Exit))
    }
    let mut s = search_state();
    let _ = update(&mut s, Message::Query("report".into()));
    assert!(exits(update(&mut s, Message::FocusLost)));

    let mut s = search_state();
    s.opening = Some(("report.desktop".into(), "report".into()));
    assert!(!exits(update(&mut s, Message::FocusLost)), "a pending open reports its own result");

    let mut s = search_state();
    let _ = update(&mut s, Message::ShowFolders);
    assert!(!exits(update(&mut s, Message::FocusLost)), "unsaved folder edits stay open");
    assert!(s.folder_settings.is_some());

    let mut s = search_state();
    let _ = update(&mut s, Message::Query("report".into()));
    let _ = s.perform(Action::CopyName);
    let _ = update(&mut s, Message::Query("other".into()));
    assert!(!exits(update(&mut s, Message::FocusLost)), "the launcher still serves the copied name");
}

#[test]
fn copying_or_cutting_input_text_survives_focus_loss() {
    use iced::advanced::{Layout, Shell, clipboard, layout, widget::Tree};
    use iced::keyboard::key::{Code, Physical};
    use iced::{Rectangle, Size};

    #[derive(Default)]
    struct Clipboard(Option<String>);
    impl clipboard::Clipboard for Clipboard {
        fn read(&self, _kind: clipboard::Kind) -> Option<String> {
            self.0.clone()
        }
        fn write(&mut self, kind: clipboard::Kind, contents: String) {
            assert_eq!(kind, clipboard::Kind::Standard);
            self.0 = Some(contents);
        }
    }

    for (letter, code, modifiers) in [
        ("c", Code::KeyC, Modifiers::CTRL),
        ("x", Code::KeyX, Modifiers::CTRL),
        ("c", Code::KeyC, Modifiers::CTRL | Modifiers::SHIFT),
    ] {
        let mut s = search_state();
        let _ = update(&mut s, Message::Query("report".into()));
        let mut clipboard = Clipboard::default();
        let messages = {
            let renderer =
                iced::Renderer::Secondary(iced_tiny_skia::Renderer::new(iced::Font::DEFAULT, iced::Pixels(16.0)));
            let mut element = view(&s);
            let widget = element.as_widget_mut();
            let mut tree = Tree::new(&*widget);
            let bounds = Size::new(720.0, 500.0);
            let node = widget.layout(&mut tree, &renderer, &layout::Limits::new(Size::ZERO, bounds));
            widget.operate(
                &mut tree,
                Layout::new(&node),
                &renderer,
                &mut iced::advanced::widget::operation::focusable::focus::<()>(search_id()),
            );
            let pressed = |letter: &str, code, modifiers| {
                Event::Keyboard(keyboard::Event::KeyPressed {
                    key: Key::Character(letter.into()),
                    modified_key: Key::Character(letter.into()),
                    physical_key: Physical::Code(code),
                    location: keyboard::Location::Standard,
                    modifiers,
                    text: None,
                    repeat: false,
                })
            };
            let mut messages = Vec::new();
            for event in [
                Event::Keyboard(keyboard::Event::ModifiersChanged(Modifiers::CTRL)),
                pressed("a", Code::KeyA, Modifiers::CTRL),
                Event::Keyboard(keyboard::Event::ModifiersChanged(modifiers)),
                pressed(letter, code, modifiers),
            ] {
                let mut shell = Shell::new(&mut messages);
                widget.update(
                    &mut tree,
                    &event,
                    Layout::new(&node),
                    mouse::Cursor::Unavailable,
                    &renderer,
                    &mut clipboard,
                    &mut shell,
                    &Rectangle::with_size(bounds),
                );
                if let Some(message) = on_event(event, shell.event_status(), iced::window::Id::unique()) {
                    messages.push(message);
                }
            }
            messages
        };
        assert_eq!(clipboard.0.as_deref(), Some("report"), "Ctrl+{letter} must copy the actual input selection");
        for message in messages {
            let _ = update(&mut s, message);
        }
        let task = update(&mut s, Message::FocusLost);
        let actions = iced::futures::executor::block_on(async {
            match iced_runtime::task::into_stream(task) {
                Some(stream) => stream.collect::<Vec<_>>().await,
                None => Vec::new(),
            }
        });
        assert!(
            !actions.iter().any(|action| matches!(action, iced_runtime::Action::Exit)),
            "{modifiers:?}+{letter} clipboard contents must remain available after clicking the paste destination"
        );
    }
}

#[test]
fn result_copy_path_preserves_non_utf8_files_and_folders() {
    use std::os::unix::ffi::OsStringExt;

    for (bytes, url, expected) in [
        (b"/synthetic/non\xff.txt".as_slice(), "file:///synthetic/non%FF.txt", "file:///synthetic/non%FF.txt"),
        (
            "/synthetic/caffè notes.txt".as_bytes(),
            "file:///synthetic/caff%C3%A8%20notes.txt",
            "/synthetic/caffè notes.txt",
        ),
    ] {
        for is_dir in [false, true] {
            for from_panel in [false, true] {
                let mut s = search_state();
                let _ = update(&mut s, Message::Scope(Scope::Files));
                let _ = update(&mut s, Message::Query("synthetic".into()));
                let mut file = file_hits().remove(0);
                file.id = url.into();
                file.path = Some(std::ffi::OsString::from_vec(bytes.to_vec()).into());
                file.is_dir = is_dir;
                let target = Target::File(file.clone());
                let generation = s.generation;
                let _ = update(&mut s, Message::Files { generation, result: Ok(Some(vec![file])) });
                let task = if from_panel {
                    let _ = command(&mut s, Command::OpenActions);
                    update(&mut s, Message::PanelClicked { target, action: Action::CopyPath })
                } else {
                    command(&mut s, Command::CopyPath)
                };
                let actions = iced::futures::executor::block_on(async {
                    iced_runtime::task::into_stream(task).unwrap().collect::<Vec<_>>().await
                });
                let copied: Vec<_> = actions
                    .iter()
                    .filter_map(|action| match action {
                        iced_runtime::Action::Clipboard(iced_runtime::clipboard::Action::Write {
                            contents, ..
                        }) => Some(contents.as_str()),
                        _ => None,
                    })
                    .collect();
                assert_eq!(copied, [expected], "is_dir={is_dir}, from_panel={from_panel}");
                assert_eq!(
                    s.status,
                    Status::Notice(format!("Copied {}", if expected == url { "file URL" } else { "path" }))
                );
                assert!(s.clipboard_owner);
                assert!(s.opening.is_none());
            }
        }
    }
}

#[test]
fn captured_input_copy_also_runs_the_copy_path_command() {
    use iced::keyboard::key::{Code, Physical};

    let mut s = search_state();
    let _ = update(&mut s, Message::Scope(Scope::Files));
    let _ = update(&mut s, Message::Query("report".into()));
    let generation = s.generation;
    let _ = update(&mut s, Message::Files { generation, result: Ok(Some(file_hits())) });
    let expected = match &s.selected().unwrap().target {
        Target::File(file) => file.path.as_ref().unwrap().to_string_lossy().into_owned(),
        _ => panic!("a file must be selected"),
    };
    let event = Event::Keyboard(keyboard::Event::KeyPressed {
        key: Key::Character("c".into()),
        modified_key: Key::Character("C".into()),
        physical_key: Physical::Code(Code::KeyC),
        location: keyboard::Location::Standard,
        modifiers: Modifiers::CTRL | Modifiers::SHIFT,
        text: None,
        repeat: false,
    });
    let message = input_event(&event, event::Status::Captured).unwrap();
    let task = update(&mut s, message);
    let actions = iced::futures::executor::block_on(async {
        iced_runtime::task::into_stream(task).unwrap().collect::<Vec<_>>().await
    });
    assert!(actions.iter().any(|action| matches!(
        action,
        iced_runtime::Action::Clipboard(iced_runtime::clipboard::Action::Write { contents, .. })
            if *contents == expected
    )));
}

#[test]
fn first_run_defers_file_queries_until_folders_are_applied() {
    let mut s = search_state();
    s.deferred_query = Some("report".into());
    let _ = s.show_folders(true);
    let _ = update(&mut s, Message::Query("report".into()));
    assert_eq!(s.generation, 0);
    assert!(s.query.is_empty());
    let _ = s.finish_folders(Task::none(), folders::Effect::Applied);
    assert!(s.folder_settings.is_none());
    assert!(s.deferred_query.is_none());
    assert_eq!(s.query, "report");
    assert_eq!(s.generation, 1);
    assert!(s.files.is_empty());
    assert!(matches!(
        crate::shortcuts::Routing::Commands(s.shortcuts.bindings).message(&typed(",", Modifiers::CTRL)),
        Some(Message::ShowFolders)
    ));
}

#[test]
fn search_status_errors_can_retry_and_empty_search_restores_its_size() {
    let mut s = search_state();
    let _ = update(&mut s, Message::RefreshSearchStatus);
    assert_eq!(s.status_generation, 0, "refresh outside the view must not activate the daemon");
    s.opening = Some(("file:///synthetic/report.txt".into(), String::new()));
    let _ = update(&mut s, Message::ShowSearchStatus);
    assert!(s.search_status.is_none(), "an ongoing file open must finish before opening status");
    assert_eq!(s.status_generation, 0);
    s.opening = None;
    let _ = update(&mut s, Message::ShowSearchStatus);
    assert!(!s.compact);
    let generation = s.status_generation;
    let _ = update(&mut s, Message::SearchStatusLoaded { generation, result: Err("service unavailable".into()) });
    assert_eq!(s.search_status, Some(SearchStatusPanel::Error("service unavailable".into())));
    let _ = update(&mut s, Message::RefreshSearchStatus);
    assert_eq!(s.search_status, Some(SearchStatusPanel::Loading));
    let _ = update(&mut s, Message::ShowSearchStatus);
    assert!(s.search_status.is_none(), "the status shortcut also closes the view");
    assert!(s.compact);
    assert_eq!(
        crate::shortcuts::Routing::Commands(s.shortcuts.bindings)
            .message(&typed("I", Modifiers::CTRL))
            .map(|m| matches!(m, Message::ShowSearchStatus)),
        Some(true)
    );
    assert_eq!(
        key_message(&Key::Character("r".into()), Modifiers::CTRL).map(|m| matches!(m, Message::RefreshSearchStatus)),
        Some(true)
    );
}

#[test]
fn search_status_describes_paused_and_unknown_states_without_claiming_readiness() {
    assert_eq!(index_summary(&search_info("scanning")).0, "Indexing files");
    assert_eq!(index_summary(&search_info("paused")).0, "Indexing paused");
    assert_eq!(index_summary(&search_info("idle")).0, "Watching for changes");
    assert_eq!(index_summary(&search_info("future-state")).0, "Unrecognized search status");
}

#[test]
fn unconfirmed_open_blocks_duplicates_but_preserves_search_and_copy() {
    let mut s = search_state();
    s.apps[0].exec.clear(); // a regression must not spawn a program in this test
    let _ = update(&mut s, Message::Query("report".into()));
    let generation = s.generation;
    let _ = update(&mut s, Message::Files { generation, result: Ok(Some(file_hits())) });
    s.cursor.select(1, s.items.len());
    let _ = command(&mut s, Command::Activate);
    let (id, query) = s.opening.clone().unwrap();
    let _ = update(&mut s, Message::Opened(Err(RunError::Unconfirmed("timed out".into()))));
    assert!(s.unconfirmed_open);
    assert!(s.opening.is_none());
    assert_eq!(s.usage.boost(Kind::File, &id, &query, usage::now()), 0);
    for cmd in [Command::Activate, Command::Secondary, Command::ActivateVisible(2)] {
        let _ = command(&mut s, cmd);
        assert!(s.opening.is_none());
        assert_eq!(s.status, Status::Error(UNCONFIRMED_OPEN.into()));
    }
    let _ = update(&mut s, Message::Opened(Ok(())));
    assert_eq!(
        s.usage.boost(Kind::File, &id, &query, usage::now()),
        0,
        "late confirmation must not record use or exit"
    );
    let _ = update(&mut s, Message::Query("report".into()));
    let _ = command(&mut s, Command::Activate);
    assert_eq!(s.status, Status::Error(UNCONFIRMED_OPEN.into()), "app launches are blocked too");
    let _ = s.perform(Action::CopyName);
    assert_eq!(s.status, Status::Notice("Copied name".into()));
    assert!(s.unconfirmed_open, "copying must not unlock another open");
    let _ = update(&mut s, Message::Query("= 2+3".into()));
    let _ = command(&mut s, Command::Activate);
    assert_eq!(s.status, Status::Notice("Copied result".into()));
    let _ = update(&mut s, Message::ShowSearchStatus);
    assert_eq!(s.search_status, Some(SearchStatusPanel::Loading));
    let _ = command(&mut s, Command::Back);
    assert!(s.unconfirmed_open);
}

#[test]
fn rejected_open_can_be_corrected_and_only_success_records_usage() {
    let mut s = search_state();
    let _ = update(&mut s, Message::Query("report".into()));
    let generation = s.generation;
    let _ = update(&mut s, Message::Files { generation, result: Ok(Some(file_hits())) });
    s.cursor.select(1, s.items.len());
    let _ = command(&mut s, Command::Activate);
    let (id, query) = s.opening.clone().unwrap();
    let _ = update(&mut s, Message::Opened(Err(RunError::Failed("path is not indexed".into()))));
    assert!(!s.unconfirmed_open);
    assert!(s.opening.is_none());
    assert_eq!(s.usage.boost(Kind::File, &id, &query, usage::now()), 0);
    let _ = command(&mut s, Command::Activate);
    assert!(s.opening.is_some(), "a rejected request permits an explicit corrected attempt");
    let _ = update(&mut s, Message::Opened(Ok(())));
    assert!(s.opening.is_none());
    assert!(s.usage.boost(Kind::File, &id, &query, usage::now()) > 0);
    let now = usage::now();
    let before = s.usage.boost(Kind::File, &id, &query, now);
    let _ = update(&mut s, Message::Opened(Ok(())));
    assert_eq!(s.usage.boost(Kind::File, &id, &query, now), before);
}

#[test]
fn stale_search_failure_does_not_replace_a_newer_success() {
    let mut s = search_state();
    let _ = update(&mut s, Message::Query("old report".into()));
    let old = s.generation;
    let _ = update(&mut s, Message::Query("report".into()));
    let generation = s.generation;
    let _ = update(&mut s, Message::Files { generation, result: Ok(Some(file_hits())) });
    let items = s.items.clone();
    let _ = update(&mut s, Message::Files { generation: old, result: Err("timed out".into()) });
    assert_eq!(s.status, Status::Idle);
    assert_eq!(s.items, items);
}

fn application_actions(names: &[&str]) -> Launcher {
    let mut s = search_state();
    s.apps[0].actions = names
        .iter()
        .enumerate()
        .map(|(i, name)| ferroseek_desktop::apps::DesktopAction {
            id: format!("action-{i}"),
            name: (*name).into(),
            exec: format!("synthetic-viewer --action-{i}"),
        })
        .collect();
    let _ = update(&mut s, Message::Query("report".into()));
    let _ = command(&mut s, Command::OpenActions);
    s
}

#[test]
fn application_action_names_are_searchable_and_keep_distinct_identities() {
    let mut s = application_actions(&["Nuova finestra privata", "New window", "New window"]);
    let panel = s.panel.as_ref().unwrap();
    assert_eq!(panel.label(Action::Desktop(0)), "Nuova finestra privata");
    assert_eq!(panel.action_for_command(Command::Activate), Some(Action::Launch));
    assert_eq!(panel.action_for_command(Command::Secondary), None);
    let _ = update(&mut s, Message::ActionQuery("privata nuova".into()));
    let panel = s.panel.as_ref().unwrap();
    assert_eq!(panel.filtered_actions(), [Action::Desktop(0)]);
    assert_eq!(panel.action_for_command(Command::Activate), Some(Action::Desktop(0)));
    assert_eq!(panel.action_for_command(Command::ActivateVisible(0)), Some(Action::Desktop(0)));
    assert_eq!(s.query, "report");
    let _ = update(&mut s, Message::ActionQuery("  NEW  window ".into()));
    assert_eq!(s.panel.as_ref().unwrap().filtered_actions(), [Action::Desktop(1), Action::Desktop(2)]);
    let _ = update(&mut s, Message::ActionQuery("nwwin".into()));
    assert_eq!(s.panel.as_ref().unwrap().filtered_actions(), [Action::Desktop(1), Action::Desktop(2)]);
}

#[test]
fn long_action_menus_scroll_page_and_number_the_visible_window() {
    let names: Vec<_> = (0..12).map(|i| format!("Window {i}")).collect();
    let mut s = application_actions(&names.iter().map(String::as_str).collect::<Vec<_>>());
    let _ = command(&mut s, Command::PageDown);
    assert_eq!(s.panel.as_ref().unwrap().cursor.selected, 7);
    let _ = command(&mut s, Command::PageDown);
    let panel = s.panel.as_ref().unwrap();
    assert_eq!(panel.cursor.selected, 14);
    assert_eq!(panel.cursor.window(17), 7..15);
    assert_eq!(panel.action_for_command(Command::ActivateVisible(0)), Some(Action::Desktop(3)));
    let _ = command(&mut s, Command::Last);
    let panel = s.panel.as_ref().unwrap();
    assert_eq!(panel.cursor.window(17), 9..17);
    assert_eq!(panel.action_for_command(Command::Activate), Some(Action::EditAlias));
    assert_eq!(panel.action_for_command(Command::ActivateVisible(0)), Some(Action::Desktop(5)));
    assert_eq!(panel.action_for_command(Command::ActivateVisible(6)), Some(Action::Desktop(11)));
    assert_eq!(panel.action_for_command(Command::ActivateVisible(7)), Some(Action::EditAlias));
    assert_eq!(panel.action_for_command(Command::ActivateVisible(8)), None);
    let _ = update(&mut s, Message::Wheel(1.0));
    assert_eq!(s.panel.as_ref().unwrap().cursor.selected, 15);
    assert_eq!(s.cursor.selected, 0, "menu scrolling must leave the result selected");
    let _ = update(&mut s, Message::ActionQuery("Window 11".into()));
    let panel = s.panel.as_ref().unwrap();
    assert_eq!(panel.cursor.first, 0);
    assert_eq!(panel.action_for_command(Command::ActivateVisible(0)), Some(Action::Desktop(11)));
    assert_eq!(panel.action_for_command(Command::ActivateVisible(1)), None);
    let _ = command(&mut s, Command::Back);
    let panel = s.panel.as_ref().unwrap();
    assert_eq!(panel.cursor.window(17), 0..8);
    assert_eq!(panel.action_for_command(Command::Activate), Some(Action::Launch));
    let _ = command(&mut s, Command::Last);
    let _ = command(&mut s, Command::PageUp);
    assert_eq!(s.panel.as_ref().unwrap().cursor.selected, 9);
    let _ = command(&mut s, Command::First);
    assert_eq!(s.panel.as_ref().unwrap().cursor.selected, 0);
}

#[test]
fn application_confirmation_keeps_the_original_target_and_query() {
    let root = tempfile::tempdir().unwrap();
    let mut s = search_state();
    s.usage = Usage::load(Some(root.path().join("usage")));
    s.apps.push(
        crate::apps::parse(
            "other.desktop",
            std::path::Path::new("/synthetic/other.desktop"),
            "[Desktop Entry]\nType=Application\nName=Other\nExec=missing\n",
            &[],
            &[],
        )
        .unwrap(),
    );
    let _ = update(&mut s, Message::Query("report".into()));
    let _pending = command(&mut s, Command::Activate);
    assert_eq!(s.usage.boost(Kind::App, "report.desktop", "report", usage::now()), 0);
    let _ = update(&mut s, Message::Query("other".into()));
    assert_eq!(command(&mut s, Command::Activate).units(), 0, "a second Enter must not start another open");
    let _ = update(&mut s, Message::ApplicationOpened { id: "other.desktop".into(), result: Ok(()) });
    assert_eq!(s.usage.boost(Kind::App, "other.desktop", "other", usage::now()), 0);
    let _ = update(&mut s, Message::ApplicationOpened { id: "report.desktop".into(), result: Ok(()) });
    let now = usage::now();
    assert!(
        s.usage.boost(Kind::App, "report.desktop", "report", now)
            > s.usage.boost(Kind::App, "report.desktop", "unrelated", now)
    );
    assert_eq!(s.usage.boost(Kind::File, "report.desktop", "report", now), 0);
    assert_eq!(s.usage.boost(Kind::App, "other.desktop", "other", now), 0);
    let boost = s.usage.boost(Kind::App, "report.desktop", "report", now);
    let _ = update(&mut s, Message::ApplicationOpened { id: "report.desktop".into(), result: Ok(()) });
    assert_eq!(s.usage.boost(Kind::App, "report.desktop", "report", now), boost);
}

#[test]
fn application_submission_error_keeps_actions_usable_without_recording_usage() {
    let root = tempfile::tempdir().unwrap();
    let mut s = application_actions(&["New window"]);
    s.usage = Usage::load(Some(root.path().join("usage")));
    let id = s.apps[0].id.clone();
    let _pending = s.perform(Action::Desktop(0));
    let _ = update(&mut s, Message::ApplicationOpened { id: id.clone(), result: Err("Cannot launch target".into()) });
    assert!(s.panel.is_some() && s.opening.is_none());
    assert!(matches!(s.status, Status::Error(_)));
    assert_eq!(s.usage.boost(Kind::App, &id, "report", usage::now()), 0);
    assert!(s.perform(Action::Desktop(0)).units() > 0, "an explicitly retried action is permitted");
}

#[test]
fn closing_aborts_deferred_file_actions() {
    for route in [Action::Open, Action::Reveal, Action::OpenWith] {
        for close in ["escape", "toggle", "drop", "complete"] {
            let mut s = search_state();
            let _ = update(&mut s, Message::Query("report".into()));
            let generation = s.generation;
            let files = file_hits();
            let target = Target::File(files[0].clone());
            let _ = update(&mut s, Message::Files { generation, result: Ok(Some(files)) });
            let _ = update(&mut s, Message::Selected(target));
            let task = if route == Action::OpenWith {
                let _ = s.perform(route);
                let generation = s.application_generation;
                let _ = update(
                    &mut s,
                    Message::ApplicationsLoaded {
                        generation,
                        result: Ok(vec![crate::backend::ApplicationChoice {
                            id: "chosen.desktop".into(),
                            name: "Chosen".into(),
                            icon: String::new(),
                        }]),
                    },
                );
                command(&mut s, Command::Activate)
            } else {
                s.perform(route)
            };
            if close == "escape" {
                // Close chooser, clear query, then close the launcher.
                for _ in 0..3 {
                    let _ = command(&mut s, Command::Back);
                }
            } else if close == "toggle" {
                let _ = update(&mut s, Message::Ui(UiEvent::Toggle));
            }
            let mut state = Some(s);
            if close == "drop" {
                drop(state.take());
            }
            let output = iced::futures::executor::block_on(async {
                if let Some(stream) = iced_runtime::task::into_stream(task) {
                    stream.collect::<Vec<_>>().await
                } else {
                    Vec::new()
                }
            });
            if close == "complete" {
                assert_eq!(output.len(), 1, "control must observe backend failure");
                let state = state.as_mut().unwrap();
                for action in output {
                    if let iced_runtime::Action::Output(message) = action {
                        let _ = update(state, message);
                    }
                }
                assert!(state.opening.is_none());
                assert!(matches!(state.status, Status::Error(_)));
            } else {
                assert!(output.is_empty(), "closing must cancel {route:?} before submission ({close})");
            }
        }
    }
}

#[test]
fn application_tasks_preserve_the_target_and_respect_closing() {
    let case = std::env::var("FERROSEEK_TEST_CANCEL_APP");
    let Ok(case) = case else {
        let root = tempfile::tempdir().unwrap();
        for case in ["escape", "toggle", "dispatch-app", "dispatch-action"] {
            let output = std::process::Command::new(std::env::current_exe().unwrap())
                .args([
                    "--exact",
                    "ui::tests::application_tasks_preserve_the_target_and_respect_closing",
                    "--nocapture",
                ])
                .env("FERROSEEK_TEST_CANCEL_APP", case)
                .env("FERROSEEK_TEST_CANCEL_ROOT", root.path())
                .env("FERROSEEK_TEST_DISPATCH_OUTPUT", root.path().join(case))
                .env("HOME", root.path())
                .env("PATH", "/usr/bin:/bin")
                .env("XDG_CONFIG_HOME", root.path().join("config"))
                .env("XDG_DATA_HOME", root.path().join("data"))
                .env("XDG_STATE_HOME", root.path().join("state"))
                .env("XDG_CACHE_HOME", root.path().join("cache"))
                .env("XDG_RUNTIME_DIR", root.path().join("runtime"))
                .env("DBUS_SESSION_BUS_ADDRESS", "unix:path=/nonexistent-ferroseek-test-bus")
                .output()
                .unwrap();
            assert!(
                output.status.success(),
                "{case}: {}{}",
                String::from_utf8_lossy(&output.stdout),
                String::from_utf8_lossy(&output.stderr)
            );
        }
        return;
    };
    let root = std::path::PathBuf::from(std::env::var_os("FERROSEEK_TEST_CANCEL_ROOT").unwrap());
    let dispatch = case.starts_with("dispatch-");
    let mut s = if dispatch { application_actions(&["New window"]) } else { search_state() };
    if dispatch {
        use std::os::unix::fs::PermissionsExt;
        let capture = root.join(format!("capture-{case}"));
        std::fs::write(&capture, "#!/bin/sh\nset -eu\nprintf '%s\\n' \"$@\" > \"$FERROSEEK_TEST_DISPATCH_OUTPUT.tmp\"\nmv \"$FERROSEEK_TEST_DISPATCH_OUTPUT.tmp\" \"$FERROSEEK_TEST_DISPATCH_OUTPUT\"\n").unwrap();
        std::fs::set_permissions(&capture, std::fs::Permissions::from_mode(0o700)).unwrap();
        s.apps[0].exec = format!("\"{}\" captured-app", capture.display());
        s.apps[0].actions[0].exec = format!("\"{}\" captured-action", capture.display());
        s.apps.push(
            crate::apps::parse(
                "other.desktop",
                std::path::Path::new("/synthetic/other.desktop"),
                &format!(
                    "[Desktop Entry]\nType=Application\nName=Other\nExec=\"{}\" wrong-target\n",
                    capture.display()
                ),
                &[],
                &[],
            )
            .unwrap(),
        );
        s.usage = Usage::load(Some(root.join(format!("usage-{case}"))));
    } else {
        s.apps[0].exec = format!("\"{}\"", root.join("nonexistent-command").display());
    }
    let _ = update(&mut s, Message::Query("report".into()));
    let task =
        if case == "dispatch-action" { s.perform(Action::Desktop(0)) } else { command(&mut s, Command::Activate) };
    if dispatch {
        let _ = update(&mut s, Message::Query("other".into()));
        assert_eq!(command(&mut s, Command::Activate).units(), 0);
    } else if case == "escape" {
        let _ = update(&mut s, Message::Query(String::new()));
        let _ = command(&mut s, Command::Back);
    } else {
        let _ = update(&mut s, Message::Ui(UiEvent::Toggle));
    }
    let output = iced::futures::executor::block_on(async {
        if let Some(stream) = iced_runtime::task::into_stream(task) {
            stream.collect::<Vec<_>>().await
        } else {
            Vec::new()
        }
    });
    if dispatch {
        assert_eq!(output.len(), 1);
        for action in output {
            if let iced_runtime::Action::Output(message) = action {
                let _ = update(&mut s, message);
            }
        }
        let path = std::path::PathBuf::from(std::env::var_os("FERROSEEK_TEST_DISPATCH_OUTPUT").unwrap());
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(3);
        while !path.exists() && std::time::Instant::now() < deadline {
            std::thread::sleep(std::time::Duration::from_millis(5));
        }
        let expected = if case == "dispatch-action" { "captured-action\n" } else { "captured-app\n" };
        assert_eq!(std::fs::read_to_string(path).unwrap(), expected);
        assert!(s.usage.boost(Kind::App, "report.desktop", "report", usage::now()) > 0);
        assert_eq!(s.usage.boost(Kind::App, "other.desktop", "other", usage::now()), 0);
    } else {
        assert!(output.is_empty(), "closing must abort deferred submission, not produce a launch result");
    }
}

#[test]
fn invalid_application_action_keeps_the_menu_usable_and_does_not_record_usage() {
    let mut s = application_actions(&["New window"]);
    let invalid = Action::Desktop(usize::MAX);
    s.panel.as_mut().unwrap().actions.push(invalid);
    let target = s.selected().unwrap().target.clone();
    // While a file open is pending, application actions obey the same gate.
    s.opening = Some(("file:///synthetic/report.txt".into(), "report".into()));
    let _ = update(&mut s, Message::PanelClicked { target: target.clone(), action: invalid });
    assert!(!matches!(s.status, Status::Error(_)));
    s.opening = None;
    let _ = update(&mut s, Message::PanelClicked { target, action: invalid });
    assert!(matches!(s.status, Status::Error(_)));
    assert!(s.panel.is_some());
    assert_eq!(s.usage.boost(Kind::App, &s.apps[0].id, "report", usage::now()), 0);
    let _ = update(&mut s, Message::ActionQuery("missing".into()));
    let status = s.status.clone();
    let target = s.selected().unwrap().target.clone();
    let _ = update(&mut s, Message::PanelClicked { target, action: invalid });
    assert_eq!(s.status, status, "a filtered-out action cannot run from a queued click");
}

#[derive(Default)]
struct RenderedLabels {
    text: Vec<(String, iced::Rectangle)>,
    inputs: Vec<iced::Rectangle>,
    scroll_offsets: Vec<iced::Vector>,
}

impl iced::advanced::widget::Operation for RenderedLabels {
    fn traverse(&mut self, operate: &mut dyn FnMut(&mut dyn iced::advanced::widget::Operation)) {
        operate(self);
    }
    fn text(&mut self, _: Option<&Id>, bounds: iced::Rectangle, value: &str) {
        self.text.push((value.to_string(), bounds));
    }
    fn scrollable(
        &mut self,
        _: Option<&Id>,
        _: iced::Rectangle,
        _: iced::Rectangle,
        translation: iced::Vector,
        _: &mut dyn iced::advanced::widget::operation::Scrollable,
    ) {
        self.scroll_offsets.push(translation);
    }
    fn text_input(
        &mut self,
        _: Option<&Id>,
        bounds: iced::Rectangle,
        _: &mut dyn iced::advanced::widget::operation::TextInput,
    ) {
        self.inputs.push(bounds);
    }
}

fn rendered_labels<M>(mut element: Element<'_, M>, size: iced::Size) -> RenderedLabels {
    use iced::advanced::{Layout, layout, widget::Tree};

    let renderer = iced::Renderer::Secondary(iced_tiny_skia::Renderer::new(iced::Font::DEFAULT, iced::Pixels(16.0)));
    let widget = element.as_widget_mut();
    let mut tree = Tree::new(&*widget);
    let node = widget.layout(&mut tree, &renderer, &layout::Limits::new(iced::Size::ZERO, size));
    let mut labels = RenderedLabels::default();
    widget.operate(&mut tree, Layout::new(&node), &renderer, &mut labels);
    labels
}

#[test]
fn displayed_home_paths_preserve_absolute_copy_and_url_fallbacks() {
    use iced::Size;
    use std::os::unix::ffi::OsStringExt;

    if let Ok(home) = std::env::var("HOME")
        && !home.is_empty()
    {
        let path = format!("{home}/Corpus/report.md");
        assert_eq!(model::home_relative(&home), "~");
        assert_eq!(model::home_relative(&path), "~/Corpus/report.md");
        assert_eq!(model::home_relative(&format!("{home}x/report.md")), format!("{home}x/report.md"));
        let mut file = file_hits().remove(0);
        file.path = Some(path.clone().into());
        file.name = "report.md".into();
        file.subtext = "~/Corpus".into();
        let mut preview = preview::Panel::new(file.clone());
        let labels = rendered_labels(preview.view(), Size::new(500.0, 400.0));
        assert!(labels.text.iter().any(|(value, _)| value == "~/Corpus"));
        assert!(matches!(preview.event(preview::Event::CopyLocation), preview::Effect::Copy(value) if value == path));
        let chooser = open_with::Panel::new(file.clone());
        let labels = rendered_labels(chooser.view(false, false), Size::new(640.0, 500.0));
        assert!(labels.text.iter().any(|(value, _)| value == "report.md"));
        assert!(labels.text.iter().any(|(value, _)| value == "~/Corpus"));
        let mut state = search_state();
        state.search_folder = Some(file);
        let labels = rendered_labels(view_content(&state), Size::new(640.0, full_height() as f32));
        assert!(labels.text.iter().any(|(value, _)| value == "~/Corpus/report.md"));
    }
    let mut file = file_hits().remove(0);
    file.id = "file:///synthetic/non%FF.md".into();
    file.path = Some(std::ffi::OsString::from_vec(b"/synthetic/non\xff.md".to_vec()).into());
    let mut preview = preview::Panel::new(file.clone());
    let labels = rendered_labels(preview.view(), Size::new(500.0, 400.0));
    assert!(labels.text.iter().any(|(value, _)| value == &file.id));
    let chooser = open_with::Panel::new(file.clone());
    let labels = rendered_labels(chooser.view(false, false), Size::new(640.0, 500.0));
    assert!(labels.text.iter().any(|(value, _)| value == &file.id));
    assert!(matches!(preview.event(preview::Event::CopyLocation), preview::Effect::Copy(value) if value == file.id));
    let mut state = search_state();
    state.search_folder = Some(file.clone());
    let labels = rendered_labels(view_content(&state), Size::new(640.0, full_height() as f32));
    assert!(labels.text.iter().any(|(value, _)| value == &file.id));
}

#[test]
fn search_query_has_its_own_full_width_and_navigation_stays_in_the_header() {
    for width in [320, 608, 640, 720, 880] {
        let mut state = search_state();
        let geometry = layout::calculate(Preferences::default(), Some((width + 32, 800)), false, false);
        state.placement = Some(layout::Geometry { width, ..geometry });
        let labels = rendered_labels(view_content(&state), iced::Size::new(width as f32, 600.0));
        let input = labels.inputs[0];
        assert_eq!(input.width, width as f32 - 32.0);
        for name in ["All", "Applications", "Files", "Commands", "Settings"] {
            let (_, bounds) = labels.text.iter().find(|(value, _)| value == name).unwrap();
            assert!(bounds.y >= input.y + input.height, "{name} overlaps the query at {width}px");
            assert!(
                bounds.width >= 12.0 && bounds.x >= PAD && bounds.x + bounds.width <= width as f32 - PAD,
                "{width}px {name}: {bounds:?}"
            );
            assert!(bounds.y + bounds.height <= PAD + INPUT_HEIGHT);
        }
    }
}

#[test]
fn settings_navigation_keeps_search_and_ignores_stale_search_input() {
    let mut state = search_state();
    let _ = update(&mut state, Message::Scope(Scope::Files));
    let _ = update(&mut state, Message::Query("report".into()));
    let generation = state.generation;
    let mut shortcut = typed("S", Modifiers::CTRL | Modifiers::SHIFT);
    if let Event::Keyboard(keyboard::Event::KeyPressed { text, .. }) = &mut shortcut {
        *text = Some("S".into());
    }
    let mut cache = iced_runtime::user_interface::Cache::default();
    widget_batch(&mut state, &mut cache, &[shortcut], false);
    assert!(state.settings_menu_open());
    let _ = update(&mut state, Message::Query("queued edit".into()));
    let _ = update(&mut state, Message::Scope(Scope::Applications));
    let _ = update(&mut state, Message::Files { generation, result: Ok(Some(file_hits())) });
    let selected = state.selected().unwrap().target.clone();
    let _ = update(&mut state, Message::Cmd(Command::Down));
    let _ = update(&mut state, Message::Cmd(Command::Down));
    let _ = update(&mut state, Message::Cmd(Command::Activate));
    assert!(state.folder_settings.is_some());
    assert!(!state.settings_menu_open());
    let _ = update(&mut state, Message::Cmd(Command::Back));
    assert!(state.settings_menu_open());
    assert_eq!(state.settings, Some(settings::Entry::SearchFolders));
    let _ = update(&mut state, Message::Cmd(Command::Down));
    let _ = update(&mut state, Message::Cmd(Command::Activate));
    assert!(state.search_status.is_some());
    let _ = update(&mut state, Message::Cmd(Command::Back));
    assert_eq!(state.settings, Some(settings::Entry::SearchStatus));
    let _ = update(&mut state, Message::ShowFolders);
    let _ = update(&mut state, Message::Cmd(Command::Back));
    assert_eq!(state.settings, Some(settings::Entry::SearchFolders));
    let _ = update(&mut state, Message::Cmd(Command::Back));
    assert!(state.settings.is_none() && state.search_status.is_none());
    assert_eq!((state.query.as_str(), state.generation, state.scope), ("report", generation, Scope::Files));
    assert_eq!(state.selected().unwrap().target, selected);
}

#[test]
fn settings_pointer_controls_open_and_restore_the_menu_at_narrow_width() {
    let mut state = search_state();
    state.output_bounds = Some((352, 800));
    let _ = state.resize();
    let click = |state: &mut Launcher, label: &str| {
        let width = state.window_width as f32;
        let labels = rendered_labels(view_content(state), iced::Size::new(width, full_height() as f32));
        let (_, bounds) = labels.text.iter().find(|(value, _)| value == label).unwrap();
        assert!(bounds.x >= PAD && bounds.x + bounds.width <= width - PAD, "{label}: {bounds:?}");
        let messages = pointer_messages(
            state,
            iced::Point::new(bounds.center_x(), bounds.center_y()),
            vec![mouse::Event::ButtonPressed(mouse::Button::Left), mouse::Event::ButtonReleased(mouse::Button::Left)],
        );
        assert!(!messages.is_empty(), "{label} must be clickable for {:?}: {bounds:?}", state.settings);
        for message in messages {
            let _ = update(state, message);
        }
    };
    click(&mut state, "Settings");
    for (label, entry) in
        [("Appearance", settings::Entry::Appearance), ("Search status", settings::Entry::SearchStatus)]
    {
        click(&mut state, label);
        if entry == settings::Entry::Appearance {
            let generation = state.appearance_generation;
            let _ = update(
                &mut state,
                Message::Appearance {
                    generation,
                    event: appearance::Event::Loaded(Err("Cannot read settings".into())),
                },
            );
        }
        assert!(!state.settings_menu_open());
        click(&mut state, "Back");
        assert!(state.settings_menu_open());
        assert_eq!(state.settings, Some(entry));
    }
    click(&mut state, "Back to search");
    assert!(state.settings.is_none());
}

#[test]
fn settings_scroll_does_not_carry_into_child_pages() {
    check_settings_scroll(Command::Last, settings::Entry::SearchStatus);
    check_settings_scroll(Command::Down, settings::Entry::Shortcuts);
}

fn check_settings_scroll(step: Command, destination: settings::Entry) {
    use iced::Size;
    use iced_runtime::{Action as RuntimeAction, UserInterface, task, user_interface::Cache};

    let mut state = search_state();
    state.output_bounds = Some((352, 300));
    let _ = update(&mut state, Message::ShowSettings);
    let scroll = update(&mut state, Message::Cmd(step));
    let mut renderer =
        iced::Renderer::Secondary(iced_tiny_skia::Renderer::new(iced::Font::DEFAULT, iced::Pixels(16.0)));
    let size = Size::new(state.window_width as f32, state.window_height as f32);
    let mut menu = UserInterface::build(view(&state), size, Cache::default(), &mut renderer);
    let mut stream = task::into_stream(scroll).unwrap();
    let Some(RuntimeAction::Widget(mut operation)) = iced::futures::executor::block_on(stream.next()) else {
        panic!("moving down in Settings must scroll the selected entry into view");
    };
    menu.operate(&renderer, operation.as_mut());
    let mut layout = RenderedLabels::default();
    menu.operate(&renderer, &mut layout);
    assert!(layout.scroll_offsets.iter().any(|offset| offset.y > 0.0));
    let cache = menu.into_cache();

    let _ = update(&mut state, Message::Cmd(Command::Activate));
    assert_eq!(state.settings, Some(destination));
    if destination == settings::Entry::SearchStatus {
        let generation = state.status_generation;
        let error = "The search service is temporarily unavailable. ".repeat(20);
        let _ = update(&mut state, Message::SearchStatusLoaded { generation, result: Err(error) });
    }
    let size = Size::new(state.window_width as f32, state.window_height as f32);
    let mut page = UserInterface::build(view(&state), size, cache, &mut renderer);
    let mut layout = RenderedLabels::default();
    page.operate(&renderer, &mut layout);
    assert_eq!(layout.scroll_offsets, vec![iced::Vector::ZERO], "{destination:?}");
}

#[test]
fn folder_header_keeps_controls_and_input_inside_its_height() {
    use iced::Size;

    let (state, _) = browser_state();
    for width in [320.0, 608.0, 640.0, 880.0] {
        let labels = rendered_labels(view_content(&state), Size::new(width, full_height() as f32));
        let input = labels.inputs[0];
        assert!(input.height >= 26.0, "folder input was squeezed at {width}px: {input:?}");
        assert!(input.y >= PAD && input.y + input.height <= PAD + INPUT_HEIGHT + 0.5, "{input:?}");
        for control in ["Back", "Refresh"] {
            let (_, bounds) = labels.text.iter().find(|(value, _)| value == control).unwrap();
            assert!(bounds.y >= PAD && bounds.y + bounds.height <= PAD + INPUT_HEIGHT + 0.5, "{bounds:?}");
        }
        // The measured path must not consume the Back/Refresh hit areas.
        let paths: Vec<_> = labels
            .text
            .iter()
            .filter(|(value, bounds)| {
                bounds.y + bounds.height <= input.y
                    && !matches!(value.as_str(), "Alt+←" | "Back" | "Ctrl+R" | "Refresh")
            })
            .collect();
        assert_eq!(paths.len(), 1, "expected one folder label at {width}px: {:?}", labels.text);
        let (value, path) = paths[0];
        assert!(!value.is_empty(), "folder label disappeared at {width}px");
        assert!(path.x >= 90.0 && path.x + path.width < width - 90.0);
    }
}

#[test]
fn open_with_labels_use_available_width_and_keep_suffixes() {
    use iced::Size;
    use iced::advanced::text::Paragraph;

    let mut state = picker_state();
    let name = "Visual document handler with a long descriptive name and 日本語";
    let id = "org.example.Documents.Visual.document.handler.with.a.long.identifier.desktop";
    let panel = state.application_picker.as_mut().unwrap();
    panel.loaded(
        Ok(vec![crate::backend::ApplicationChoice { id: id.into(), name: name.into(), icon: String::new() }]),
        &mut state.icons,
    );
    for width in [320.0, 608.0, 704.0, 880.0] {
        let labels = rendered_labels(view_content(&state), Size::new(width, full_height() as f32)).text;
        for (prefix, size) in [("Visual", 15.0), ("org.example", 11.0)] {
            let (value, bounds) = labels.iter().find(|(value, _)| value.starts_with(prefix)).unwrap();
            let paragraph =
                <iced::Renderer as iced::advanced::text::Renderer>::Paragraph::with_text(iced::advanced::text::Text {
                    content: value,
                    bounds: Size::INFINITE,
                    size: iced::Pixels(size),
                    line_height: Default::default(),
                    font: iced::Font::DEFAULT,
                    align_x: Default::default(),
                    align_y: Vertical::Top,
                    shaping: Default::default(),
                    wrapping: text::Wrapping::None,
                });
            assert!(paragraph.min_width() <= bounds.width + 0.5, "{value:?} is clipped at chooser width {width}");
            if size == 11.0 {
                assert!(value.ends_with(".desktop"));
            } else if width == 880.0 {
                assert_eq!(value, name, "do not truncate a name that fits");
            }
        }
        assert!(labels.iter().any(|(value, _)| value == "Ctrl+1"));
        let (_, refresh) = labels.iter().find(|(value, _)| value == "Refresh").unwrap();
        let (_, count) = labels.iter().find(|(value, _)| value == "1 application").unwrap();
        let (_, open) = labels.iter().find(|(value, _)| value == "Open").unwrap();
        assert!(refresh.width >= 42.0 && refresh.x + refresh.width <= width - PAD, "{refresh:?}");
        assert!(count.width >= 70.0 && count.x + count.width <= open.x, "{count:?}");
        assert!(open.width >= 26.0 && open.x + open.width <= width - PAD, "{open:?}");
    }
}

#[test]
fn narrow_result_labels_fit_and_keep_distinguishing_text() {
    use iced::Size;
    use iced::advanced::text::Paragraph;

    let mut state = search_state();
    let mut file = file_hits().remove(0);
    file.name = "Quarterly report 2026 with reviewed figures and final recommendations.md".into();
    file.subtext = "~/Corpus/Documents/Department/Annual planning/Reports 2026".into();
    state.apps[0].comment = Some("Create and protect your documents with the synthetic report viewer".into());
    state.items = model::build_items(&state.apps, &[(0, crate::matcher::EXACT)], &[file]);
    state.items.push(model::calc_item("1 + 2 + 3 + 4 + 5 + 6 + 7 + 8 + 9 + 10 + 11 + 12").unwrap());
    for (width, preview) in [(280.0, false), (320.0, false), (384.0, false), (704.0, false), (352.0, true)] {
        state.preview_enabled = preview;
        for i in 0..state.items.len() {
            let labels = rendered_labels(result_row(&state, i, i), Size::new(width, ROW_HEIGHT)).text;
            assert!(labels.len() >= 2);
            assert_eq!(labels.iter().any(|(value, _)| value == state.items[i].tag), !preview);
            assert!(labels.iter().any(|(value, _)| value == &format!("Ctrl+{}", i + 1)));
            for ((value, bounds), size) in labels.iter().take(2).zip([15.0, 12.0]) {
                let paragraph = <iced::Renderer as iced::advanced::text::Renderer>::Paragraph::with_text(
                    iced::advanced::text::Text {
                        content: value,
                        bounds: Size::INFINITE,
                        size: iced::Pixels(size),
                        line_height: Default::default(),
                        font: iced::Font::DEFAULT,
                        align_x: Default::default(),
                        align_y: Vertical::Top,
                        shaping: Default::default(),
                        wrapping: text::Wrapping::None,
                    },
                );
                assert!(paragraph.min_width() <= bounds.width + 0.5, "{value:?} is clipped at list width {width}");
            }
            if matches!(state.items[i].target, Target::File(_)) {
                assert!(labels[0].0.ends_with(".md"), "the extension distinguishes similar files");
                assert!(labels[1].0.ends_with("2026"), "keep the distinguishing end of the parent path");
            } else {
                let subtitle = &labels[1].0;
                if subtitle != &state.items[i].subtitle {
                    assert!(
                        subtitle.ends_with('…'),
                        "descriptions and expressions must truncate at the end: {subtitle}"
                    );
                    assert!(state.items[i].subtitle.starts_with(subtitle.trim_end_matches('…')));
                }
            }
        }
    }
}

#[test]
fn secondary_pages_share_title_and_return_action_alignment() {
    let file = file_hits().remove(0);
    let status = SearchStatusPanel::Loading;
    let folders = folders::Panel::new("/synthetic/config.toml".into(), false);
    let appearance =
        appearance::Panel::new("/synthetic/config.toml".into(), Preferences::default(), appearance::Tab::Colors);
    let details = details::Panel::new(file.clone());
    let chooser = open_with::Panel::new(file);
    for (title, page) in [
        ("Search status", search_status_view(&status).map(|_| ())),
        ("Search folders", folders.view().map(|_| ())),
        ("Appearance", appearance.view(None, true, false).map(|_| ())),
        ("File details", details.view().map(|_| ())),
        ("Open with…", chooser.view(false, false).map(|_| ())),
    ] {
        let labels = rendered_labels(page, iced::Size::new(720.0, 515.0)).text;
        let (_, heading) = labels.iter().find(|(value, _)| value == title).unwrap();
        assert!((heading.center_y() - 44.0).abs() < 0.5, "{title}: {heading:?}");
        let (_, back) = labels.iter().rev().find(|(value, _)| value == "Back to search" || value == "Back").unwrap();
        assert!((back.center_y() - 490.0).abs() < 0.5, "{title}: {back:?}");
    }
}

#[test]
fn narrow_file_details_footer_keeps_copy_feedback_and_actions_visible() {
    let mut file = file_hits().remove(0);
    file.path = None;
    let mut panel = details::Panel::new(file);
    let _ = panel.event(details::Event::CopyLocation);
    let width = 320.0;
    let labels = rendered_labels(panel.view(), iced::Size::new(width, 515.0)).text;
    let (_, refresh) = labels.iter().find(|(value, _)| value == "Refresh").unwrap();
    assert!(refresh.width >= 42.0 && refresh.x + refresh.width <= width - PAD, "{refresh:?}");
    let mut end = PAD;
    for label in ["Copied file URL", "Copy file URL", "Preview text"] {
        let (_, bounds) = labels.iter().find(|(value, _)| value == label).unwrap();
        assert!(
            bounds.width >= 65.0 && bounds.x >= end && bounds.x + bounds.width <= width - PAD,
            "{label}: {bounds:?}"
        );
        end = bounds.x + bounds.width;
    }
}

#[test]
fn folder_settings_only_repeat_locations_that_are_actually_shortened() {
    let temp = tempfile::tempdir().unwrap();
    let path = temp.path().join("config.toml");
    let root = "/synthetic/medium-length-project-directory/documents";
    std::fs::write(&path, format!("roots = ['{root}']\n")).unwrap();
    let mut panel = folders::Panel::new(path.clone(), false);
    let _ = panel.update(
        folders::Event::Loaded { revision: 0, result: ferroseek_config::folders::Document::load(path) },
        Backend::disconnected(),
    );
    let labels = rendered_labels(panel.view(), iced::Size::new(720.0, 515.0)).text;
    assert_eq!(labels.iter().filter(|(value, _)| value == root).count(), 1);
    let labels = rendered_labels(panel.view(), iced::Size::new(320.0, 515.0)).text;
    assert_eq!(
        labels.iter().filter(|(value, _)| value == root).count(),
        1,
        "the complete selected location remains keyboard-accessible"
    );
    assert!(labels.iter().any(|(value, _)| value.contains('…')));
}

#[test]
fn ellipsizing() {
    assert_eq!(ellipsize_end("abcdef", 6), "abcdef");
    assert_eq!(ellipsize_end("abcdefg", 6), "abcde…");
    assert_eq!(ellipsize_middle("~/a/very/long/path/name", 11), "~/a/…/name");
    assert_eq!(ellipsize_middle("Visual report with final notes.md", 21), "Visual…notes.md");
    for path in [
        "~/Projects/averyveryverylongprojectdirectoryname/src",
        "~/Work/Client deliverables for the second quarter review/final",
    ] {
        let shortened = ellipsize_middle(path, 40);
        assert!(shortened.chars().count() >= 30, "unused path budget: {shortened}");
    }
    let name = "Visual report 2026 with reviewed figures and final recommendations.md";
    assert!(ellipsize_middle(name, 34).ends_with("recommendations.md"));
    let path = format!("~/Corpus/Documents/Department with a long name/Reports 2026/{name}");
    assert!(ellipsize_middle(&path, 90).ends_with(name));
    assert_eq!(ellipsize_middle("short", 11), "short");
    assert_eq!(ellipsize_middle("日本語のパス/ファイル", 7).chars().count(), 7);
}

#[test]
fn heights_fit_the_rows() {
    assert!(full_height() > compact_height());
    assert!(full_height() as f32 >= compact_height() as f32 + ROW_HEIGHT * VISIBLE_ROWS as f32);
}

#[test]
fn action_panel_keeps_its_return_hint_visible_on_short_outputs() {
    let mut state = application_actions(&["New window", "Other window"]);
    state.panel = None;
    state.output_bounds = Some((640, 332));
    let _ = state.resize();
    let _ = command(&mut state, Command::OpenActions);
    for bounds in [(640, 332), (640, 480), (1280, 800), (320, 332), (640, 332)] {
        let _ = command(&mut state, Command::Last);
        let expected = state.panel.as_ref().unwrap().cursor.selected;
        state.output_bounds = Some(bounds);
        let _ = state.resize();
        let panel = state.panel.as_ref().unwrap();
        assert_eq!(panel.cursor.selected, expected);
        let target = &state.selected().unwrap().target;
        let labels = rendered_labels(
            action_panel(panel, target),
            iced::Size::new(state.window_width as f32, state.window_height as f32),
        );
        for name in ["Esc", "Close actions", "Run"] {
            let (_, rect) = labels.text.iter().find(|(value, _)| value == name).unwrap();
            assert!(rect.height >= 14.0, "{name} is clipped on {bounds:?}: {rect:?}");
            assert!(rect.x >= 0.0 && rect.x + rect.width <= state.window_width as f32, "{name}: {rect:?}");
        }
        let actions = panel.filtered_actions();
        for (slot, index) in panel.cursor.window(actions.len()).enumerate() {
            let name = panel.label(actions[index]);
            assert!(labels.text.iter().any(|(value, rect)| value == name && rect.height >= 18.0));
            let number = format!("Ctrl+{}", slot + 1);
            assert!(labels.text.iter().any(|(value, rect)| value == &number && rect.height >= 14.0));
            assert_eq!(panel.action_for_command(Command::ActivateVisible(slot)), Some(actions[index]));
        }
    }
    let _ = command(&mut state, Command::Back);
    assert!(state.panel.is_none());
}

#[test]
fn filtering_actions_keeps_the_field_and_return_hint_in_place() {
    let mut state = application_actions(&["New window", "Other window"]);
    for bounds in [(320, 332), (640, 332), (640, 480), (1280, 800)] {
        state.output_bounds = Some(bounds);
        let _ = state.resize();
        let snapshot = |state: &Launcher| {
            let panel = state.panel.as_ref().unwrap();
            rendered_labels(
                action_panel(panel, &state.selected().unwrap().target),
                iced::Size::new(state.window_width as f32, state.window_height as f32),
            )
        };
        let before = snapshot(&state);
        let escape = |labels: &RenderedLabels| labels.text.iter().find(|(value, _)| value == "Esc").unwrap().1;
        for query in ["other window", "no matching command"] {
            let _ = update(&mut state, Message::ActionQuery(query.into()));
            let after = snapshot(&state);
            assert_eq!(after.inputs, before.inputs, "the focused field moved after filtering at {bounds:?}");
            assert_eq!(escape(&after).y, escape(&before).y, "the return hint moved at {bounds:?}");
            assert_eq!(after.text.iter().any(|(value, _)| value == "Run"), query == "other window");
        }
        let _ = command(&mut state, Command::Back);
    }
}
