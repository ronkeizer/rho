//! Modal rendering. `view_modal()` is one giant match on `Prompt` —
//! every variant gets its own branch. The `docker_header` /
//! `process_header` helpers and the `modal_style` / `backdrop_style`
//! container styles live here too because they're only used by modals.

use iced::alignment::Horizontal;
use iced::widget::{
    button, column, container, mouse_area, row, scrollable, stack, text, text_input, Space,
};
use iced::{Border, Color, Element, Font, Length, Padding, Shadow, Theme, Vector};

use crate::config::dim;
use crate::domain::{
    filtered_actions, filtered_apps, filtered_branches, filtered_containers, filtered_processes,
    filtered_recents, filtered_servers, keyboard_shortcuts, AppsState, DeleteFocus, DockerSortBy,
    DockerState, GitBranchesState, NewFilesFocus, ProcessSortBy, ProcessesState,
    Prompt, Side, SortDir, SshServersState,
};
use crate::view_pane::format_size;
use crate::{Message, PROMPT_ID};

pub fn view_modal(prompt: &Prompt) -> Element<'_, Message> {
    // Docker / Processes / Apps / GitBranches are wider so the list rows
    // have room. KeyboardShortcuts is two-column text — wider than the
    // single-column modals but narrower than the table-shaped ones.
    let modal_width = match prompt {
        Prompt::Docker { .. }
        | Prompt::Processes { .. }
        | Prompt::Apps { .. }
        | Prompt::GitBranches { .. }
        | Prompt::SshServers { .. } => 720.0,
        Prompt::KeyboardShortcuts => 600.0,
        _ => 440.0,
    };
    let dialog_inner = container(match prompt {
        Prompt::Open {
            input,
            recents,
            selected,
        } => {
            let filtered = filtered_recents(recents, input);
            let input_widget = text_input("type a path or filter recents…", input)
                .id(text_input::Id::new(PROMPT_ID))
                .on_input(Message::PromptChanged)
                .on_submit(Message::PromptSubmit)
                .padding(8);

            let list_body: Element<'_, Message> = if filtered.is_empty() {
                container(
                    text(if recents.is_empty() {
                        "No recent locations yet — type a path and press Enter."
                    } else {
                        "No recent locations match — type a path and press Enter."
                    })
                    .size(11),
                )
                .padding(Padding::from([4, 8]))
                .into()
            } else {
                let list_col = filtered.iter().enumerate().fold(
                    column![].spacing(2),
                    |col, (i, path)| {
                        // Highlighted row gets the primary fill; the rest
                        // inherit the modal background via `button::text`.
                        let style: fn(&Theme, button::Status) -> button::Style = if i == *selected {
                            button::primary
                        } else {
                            button::text
                        };
                        col.push(
                            button(
                                text(path.display().to_string())
                                    .font(Font::MONOSPACE)
                                    .size(12)
                                    .wrapping(iced::widget::text::Wrapping::None),
                            )
                            .on_press(Message::OpenRecent((*path).clone()))
                            .padding(Padding::from([4, 8]))
                            .width(Length::Fill)
                            .style(style),
                        )
                    },
                );
                scrollable(list_col)
                    .height(Length::Fixed(240.0))
                    .into()
            };

            column![
                text("Go to folder").size(15),
                input_widget,
                list_body,
                text("↑/↓ or Tab select  ·  Enter open  ·  Esc cancel").size(11),
            ]
            .spacing(10)
        }
        Prompt::Copy { input } => column![
            text("Copy selected to").size(15),
            text_input("/path/to/destination", input)
                .id(text_input::Id::new(PROMPT_ID))
                .on_input(Message::PromptChanged)
                .on_submit(Message::PromptSubmit)
                .padding(8),
            text("Enter to copy  ·  Esc or click outside to cancel").size(11),
        ]
        .spacing(10),
        Prompt::Move { input } => column![
            text("Move selected to").size(15),
            text_input("/path/to/destination", input)
                .id(text_input::Id::new(PROMPT_ID))
                .on_input(Message::PromptChanged)
                .on_submit(Message::PromptSubmit)
                .padding(8),
            text("Enter to move  ·  Esc or click outside to cancel").size(11),
        ]
        .spacing(10),
        Prompt::Compress { input } => column![
            text("Compress selected to").size(15),
            text_input("/path/to/output.zip", input)
                .id(text_input::Id::new(PROMPT_ID))
                .on_input(Message::PromptChanged)
                .on_submit(Message::PromptSubmit)
                .padding(8),
            text(
                "Bundled into one .zip (uses `zip -r`)  ·  Enter to compress  ·  Esc cancels",
            )
            .size(11),
        ]
        .spacing(10),
        Prompt::Uncompress { input } => column![
            text("Extract selected to").size(15),
            text_input("/path/to/destination", input)
                .id(text_input::Id::new(PROMPT_ID))
                .on_input(Message::PromptChanged)
                .on_submit(Message::PromptSubmit)
                .padding(8),
            text(
                "Each .zip / .tar.gz extracts to the destination directory  ·  Enter  ·  Esc cancels",
            )
            .size(11),
        ]
        .spacing(10),
        Prompt::CommandPalette {
            input,
            selected,
            actions,
        } => {
            let filtered = filtered_actions(actions, input);
            let input_widget = text_input("filter actions…", input)
                .id(text_input::Id::new(PROMPT_ID))
                .on_input(Message::PromptChanged)
                .on_submit(Message::PromptSubmit)
                .padding(8);

            let list_body: Element<'_, Message> = if filtered.is_empty() {
                container(text("No actions match.").size(11))
                    .padding(Padding::from([4, 8]))
                    .into()
            } else {
                let actions_col = filtered.iter().enumerate().fold(
                    column![].spacing(2),
                    |col, (i, action)| {
                        // Same idea as the Open modal: highlight only the
                        // active row; the rest inherit the modal background.
                        let style: fn(&Theme, button::Status) -> button::Style = if i == *selected {
                            button::primary
                        } else {
                            button::text
                        };
                        col.push(
                            button(text(action.label()).size(12))
                                .on_press(Message::PaletteSelect(*action))
                                .padding(Padding::from([2, 12]))
                                .width(Length::Fill)
                                .style(style),
                        )
                    },
                );
                actions_col.into()
            };

            column![
                text("Command Palette").size(15),
                input_widget,
                list_body,
                text("Type to filter  ·  ↑/↓ or Tab select  ·  Enter activate  ·  Esc dismiss")
                    .size(11),
            ]
            .spacing(10)
        }
        Prompt::Delete { paths, focus } => {
            let question = if paths.len() == 1 {
                let name = paths[0]
                    .file_name()
                    .map(|n| n.to_string_lossy().into_owned())
                    .unwrap_or_else(|| paths[0].display().to_string());
                format!("Do you really want to delete \"{}\"?", name)
            } else {
                format!("Do you really want to delete {} files?", paths.len())
            };

            // Focused button gets a prominent style; unfocused stays subdued.
            // Same fn-pointer type, so the conditional resolves to a single
            // `fn(&Theme, button::Status) -> button::Style` for `.style()`.
            let cancel_style: fn(&Theme, button::Status) -> button::Style =
                if *focus == DeleteFocus::Cancel {
                    button::primary
                } else {
                    button::secondary
                };
            let confirm_style: fn(&Theme, button::Status) -> button::Style =
                if *focus == DeleteFocus::Confirm {
                    button::danger
                } else {
                    button::secondary
                };

            let actions = row![
                button(text("Cancel"))
                    .on_press(Message::PromptCancel)
                    .padding(Padding::from([6, 16]))
                    .style(cancel_style),
                button(text("Delete"))
                    .on_press(Message::PromptSubmit)
                    .padding(Padding::from([6, 16]))
                    .style(confirm_style),
            ]
            .spacing(10);
            column![
                text("Confirm delete").size(15),
                text(question),
                actions,
                text("Tab or ←/→ switch  ·  Enter activate  ·  Esc cancel").size(11),
            ]
            .spacing(10)
        }
        Prompt::ConfirmLargeExtract {
            archive_path,
            size_bytes,
            focus,
        } => {
            let name = archive_path
                .file_name()
                .map(|n| n.to_string_lossy().into_owned())
                .unwrap_or_else(|| archive_path.display().to_string());
            let question = format!(
                "\"{}\" is {}. Extract it to /tmp and browse?",
                name,
                format_size(*size_bytes),
            );

            let cancel_style: fn(&Theme, button::Status) -> button::Style =
                if *focus == DeleteFocus::Cancel {
                    button::primary
                } else {
                    button::secondary
                };
            let confirm_style: fn(&Theme, button::Status) -> button::Style =
                if *focus == DeleteFocus::Confirm {
                    button::primary
                } else {
                    button::secondary
                };

            let actions = row![
                button(text("Cancel"))
                    .on_press(Message::PromptCancel)
                    .padding(Padding::from([6, 16]))
                    .style(cancel_style),
                button(text("Extract"))
                    .on_press(Message::PromptSubmit)
                    .padding(Padding::from([6, 16]))
                    .style(confirm_style),
            ]
            .spacing(10);
            column![
                text("Extract large archive").size(15),
                text(question),
                actions,
                text("Tab or ←/→ switch  ·  Enter activate  ·  Esc cancel").size(11),
            ]
            .spacing(10)
        }
        Prompt::NewFiles {
            folder,
            files,
            focus,
        } => {
            let focus = *focus;
            let primary: fn(&Theme, button::Status) -> button::Style = button::primary;
            let secondary: fn(&Theme, button::Status) -> button::Style = button::secondary;

            let header = if files.len() == 1 {
                format!("New file detected in {}:", folder.display())
            } else {
                format!(
                    "{} new files detected in {}:",
                    files.len(),
                    folder.display()
                )
            };

            // Cap the listed file count so the modal doesn't grow unbounded
            // when a big extraction lands.
            const MAX_LISTED: usize = 8;
            let mut listing = String::new();
            for name in files.iter().take(MAX_LISTED) {
                listing.push_str("  • ");
                listing.push_str(name);
                listing.push('\n');
            }
            if files.len() > MAX_LISTED {
                listing.push_str(&format!("  …and {} more\n", files.len() - MAX_LISTED));
            }
            let listing = listing.trim_end().to_string();

            let actions = row![
                button(text("No"))
                    .on_press(Message::PromptCancel)
                    .padding(Padding::from([6, 16]))
                    .style(if focus == NewFilesFocus::No {
                        primary
                    } else {
                        secondary
                    }),
                button(text("Left pane"))
                    .on_press(Message::NewFilesPickSide(Side::Left))
                    .padding(Padding::from([6, 16]))
                    .style(if focus == NewFilesFocus::Left {
                        primary
                    } else {
                        secondary
                    }),
                button(text("Right pane"))
                    .on_press(Message::NewFilesPickSide(Side::Right))
                    .padding(Padding::from([6, 16]))
                    .style(if focus == NewFilesFocus::Right {
                        primary
                    } else {
                        secondary
                    }),
            ]
            .spacing(10);

            column![
                text("New files detected").size(15),
                text(header),
                text(listing).font(Font::MONOSPACE).size(12),
                text("Would you like to switch a pane to this folder?"),
                actions,
                text("Tab cycles focus  ·  Enter activates  ·  Esc dismisses").size(11),
            ]
            .spacing(10)
        }
        Prompt::Docker {
            state,
            input,
            sort_by,
            sort_dir,
        } => {
            let filter_widget = text_input("filter by name or image…", input)
                .id(text_input::Id::new(PROMPT_ID))
                .on_input(Message::PromptChanged)
                .on_submit(Message::PromptSubmit)
                .padding(8);
            // Header wrapped in the same container shape as a body row (same
            // padding, same 1px border — invisible here) so widths line up
            // exactly. The "Actions" column uses FillPortion(2), matched in
            // the body row below.
            let header_row = container(
                row![
                    docker_header(
                        DockerSortBy::Name,
                        *sort_by,
                        *sort_dir,
                        Length::FillPortion(3)
                    ),
                    docker_header(
                        DockerSortBy::Image,
                        *sort_by,
                        *sort_dir,
                        Length::FillPortion(4)
                    ),
                    docker_header(
                        DockerSortBy::Status,
                        *sort_by,
                        *sort_dir,
                        Length::FillPortion(3)
                    ),
                    Space::with_width(Length::FillPortion(2)),
                ]
                .spacing(8)
                .align_y(iced::Alignment::Center),
            )
            .padding(Padding::from([3, 8]))
            .style(|_theme: &Theme| container::Style {
                border: Border {
                    color: Color::TRANSPARENT,
                    width: 1.0,
                    radius: 4.0.into(),
                },
                ..Default::default()
            });
            let body: Element<'_, Message> = match state {
                DockerState::Loading => container(text("Loading…").size(12))
                    .padding(Padding::from([8, 8]))
                    .into(),
                DockerState::Error(msg) => container(
                    text(msg.clone())
                        .font(Font::MONOSPACE)
                        .size(11)
                        .style(|theme: &Theme| iced::widget::text::Style {
                            color: Some(theme.extended_palette().danger.base.color),
                        }),
                )
                .padding(Padding::from([8, 8]))
                .into(),
                DockerState::Loaded(containers) if containers.is_empty() => {
                    container(text("No running containers.").size(12))
                        .padding(Padding::from([8, 8]))
                        .into()
                }
                DockerState::Loaded(containers) => {
                    let filtered = filtered_containers(containers, input);
                    if filtered.is_empty() {
                        container(text("No containers match.").size(12))
                            .padding(Padding::from([8, 8]))
                            .into()
                    } else {
                        let list_col = filtered.iter().fold(column![].spacing(3), |col, c| {
                            let id = c.id.clone();
                            let id_for_shell = c.id.clone();
                            let name_cell = text(
                                if c.name.is_empty() { c.id.clone() } else { c.name.clone() },
                            )
                            .font(Font::MONOSPACE)
                            .size(11)
                            .width(Length::FillPortion(3))
                            .wrapping(iced::widget::text::Wrapping::None);
                            let image_cell = text(c.image.clone())
                                .font(Font::MONOSPACE)
                                .size(11)
                                .width(Length::FillPortion(4))
                                .wrapping(iced::widget::text::Wrapping::None);
                            let status_cell = text(c.status.clone())
                                .font(Font::MONOSPACE)
                                .size(11)
                                .width(Length::FillPortion(3))
                                .wrapping(iced::widget::text::Wrapping::None)
                                .style(|theme: &Theme| iced::widget::text::Style {
                                    color: Some(dim(theme.extended_palette().background.base.text)),
                                });
                            // Actions packed into a FillPortion(2) column so
                            // the header's matching spacer lines up exactly.
                            let actions_col = row![
                                button(
                                    text("Kill")
                                        .size(10)
                                        .align_x(Horizontal::Center)
                                        .width(Length::Fill),
                                )
                                .on_press(Message::DockerKill(id))
                                .padding(Padding::from([2, 0]))
                                .width(Length::Fill)
                                .style(button::danger),
                                button(
                                    text("Shell")
                                        .size(10)
                                        .align_x(Horizontal::Center)
                                        .width(Length::Fill),
                                )
                                .on_press(Message::DockerShell(id_for_shell))
                                .padding(Padding::from([2, 0]))
                                .width(Length::Fill)
                                .style(button::primary),
                            ]
                            .spacing(6)
                            .width(Length::FillPortion(2));
                            let row_widget = row![name_cell, image_cell, status_cell, actions_col]
                                .spacing(8)
                                .align_y(iced::Alignment::Center);
                            col.push(
                                container(row_widget)
                                    .padding(Padding::from([3, 8]))
                                    .style(|theme: &Theme| container::Style {
                                        // Inherit the modal's background
                                        // (palette.background.base) — rows
                                        // are visually separated by the
                                        // border alone.
                                        border: Border {
                                            color: theme.extended_palette().background.strong.color,
                                            width: 1.0,
                                            radius: 4.0.into(),
                                        },
                                        ..Default::default()
                                    }),
                            )
                        });
                        scrollable(list_col).height(Length::Fixed(360.0)).into()
                    }
                }
            };

            column![
                text("Docker containers").size(15),
                filter_widget,
                header_row,
                body,
                text("Type to filter  ·  click a header to sort  ·  click Kill or Shell  ·  Esc dismisses").size(11),
            ]
            .spacing(6)
        }
        Prompt::Processes {
            state,
            input,
            sort_by,
            sort_dir,
        } => {
            let filter_widget = text_input("filter by name…", input)
                .id(text_input::Id::new(PROMPT_ID))
                .on_input(Message::PromptChanged)
                .on_submit(Message::PromptSubmit)
                .padding(8);
            // Header wrapped to mirror the body-row container shape (same
            // padding + invisible 1px border so widths line up exactly).
            let header_row = container(
                row![
                    process_header(ProcessSortBy::Name, *sort_by, *sort_dir, Length::Fill),
                    process_header(
                        ProcessSortBy::Pid,
                        *sort_by,
                        *sort_dir,
                        Length::Fixed(96.0)
                    ),
                    process_header(
                        ProcessSortBy::Cpu,
                        *sort_by,
                        *sort_dir,
                        Length::Fixed(96.0)
                    ),
                    process_header(
                        ProcessSortBy::Mem,
                        *sort_by,
                        *sort_dir,
                        Length::Fixed(96.0)
                    ),
                    Space::with_width(Length::Fixed(60.0)),
                ]
                .spacing(8)
                .align_y(iced::Alignment::Center),
            )
            .padding(Padding::from([3, 8]))
            .style(|_theme: &Theme| container::Style {
                border: Border {
                    color: Color::TRANSPARENT,
                    width: 1.0,
                    radius: 4.0.into(),
                },
                ..Default::default()
            });
            let body: Element<'_, Message> = match state {
                ProcessesState::Loading => container(text("Loading…").size(12))
                    .padding(Padding::from([8, 8]))
                    .into(),
                ProcessesState::Error(msg) => container(
                    text(msg.clone())
                        .font(Font::MONOSPACE)
                        .size(11)
                        .style(|theme: &Theme| iced::widget::text::Style {
                            color: Some(theme.extended_palette().danger.base.color),
                        }),
                )
                .padding(Padding::from([8, 8]))
                .into(),
                ProcessesState::Loaded(procs) if procs.is_empty() => {
                    container(text("No processes.").size(12))
                        .padding(Padding::from([8, 8]))
                        .into()
                }
                ProcessesState::Loaded(procs) => {
                    let filtered = filtered_processes(procs, input);
                    if filtered.is_empty() {
                        container(text("No processes match.").size(12))
                            .padding(Padding::from([8, 8]))
                            .into()
                    } else {
                        let list_col = filtered.iter().fold(column![].spacing(3), |col, p| {
                            let pid = p.pid;
                            let dim_style = |theme: &Theme| iced::widget::text::Style {
                                color: Some(dim(theme.extended_palette().background.base.text)),
                            };
                            let name_cell = text(p.name.clone())
                                .font(Font::MONOSPACE)
                                .size(11)
                                .width(Length::Fill)
                                .wrapping(iced::widget::text::Wrapping::None);
                            let pid_cell = text(format!("PID {}", p.pid))
                                .font(Font::MONOSPACE)
                                .size(11)
                                .width(Length::Fixed(96.0))
                                .style(dim_style);
                            let cpu_cell = text(format!("CPU {:>5.1}%", p.cpu_percent))
                                .font(Font::MONOSPACE)
                                .size(11)
                                .width(Length::Fixed(96.0))
                                .style(dim_style);
                            let mem_cell = text(format!("MEM {:>5.1}%", p.mem_percent))
                                .font(Font::MONOSPACE)
                                .size(11)
                                .width(Length::Fixed(96.0))
                                .style(dim_style);
                            // Kill column is Fixed(60) here and a matching
                            // 60-wide spacer in the header so widths align.
                            let row_widget = row![
                                name_cell,
                                pid_cell,
                                cpu_cell,
                                mem_cell,
                                button(
                                    text("Kill")
                                        .size(10)
                                        .align_x(Horizontal::Center)
                                        .width(Length::Fill),
                                )
                                .on_press(Message::ProcessKill(pid))
                                .padding(Padding::from([2, 0]))
                                .width(Length::Fixed(60.0))
                                .style(button::danger),
                            ]
                            .spacing(8)
                            .align_y(iced::Alignment::Center);
                            col.push(
                                container(row_widget)
                                    .padding(Padding::from([3, 8]))
                                    .style(|theme: &Theme| container::Style {
                                        // Inherit the modal's background
                                        // (palette.background.base) — rows
                                        // are visually separated by the
                                        // border alone.
                                        border: Border {
                                            color: theme.extended_palette().background.strong.color,
                                            width: 1.0,
                                            radius: 4.0.into(),
                                        },
                                        ..Default::default()
                                    }),
                            )
                        });
                        scrollable(list_col).height(Length::Fixed(360.0)).into()
                    }
                }
            };

            column![
                text("Processes").size(15),
                filter_widget,
                header_row,
                body,
                text("Type to filter  ·  click a header to sort  ·  click Kill (SIGTERM)  ·  Esc dismisses").size(11),
            ]
            .spacing(6)
        }
        Prompt::Apps {
            state,
            input,
            selected,
        } => {
            let filter_widget = text_input("filter apps…", input)
                .id(text_input::Id::new(PROMPT_ID))
                .on_input(Message::PromptChanged)
                .on_submit(Message::PromptSubmit)
                .padding(8);

            let body: Element<'_, Message> = match state {
                AppsState::Loading => container(text("Loading…").size(12))
                    .padding(Padding::from([8, 8]))
                    .into(),
                AppsState::Error(msg) => container(
                    text(msg.clone())
                        .font(Font::MONOSPACE)
                        .size(11)
                        .style(|theme: &Theme| iced::widget::text::Style {
                            color: Some(theme.extended_palette().danger.base.color),
                        }),
                )
                .padding(Padding::from([8, 8]))
                .into(),
                AppsState::Loaded(apps) => {
                    let filtered = filtered_apps(apps, input);
                    if filtered.is_empty() {
                        container(text("No apps match.").size(12))
                            .padding(Padding::from([8, 8]))
                            .into()
                    } else {
                        let list_col = filtered.iter().enumerate().fold(
                            column![].spacing(3),
                            |col, (i, app)| {
                                let path = app.path.clone();
                                let highlighted = i == *selected;
                                let name_cell = text(app.name.clone())
                                    .font(Font::MONOSPACE)
                                    .size(12)
                                    .width(Length::Fill)
                                    .wrapping(iced::widget::text::Wrapping::None);
                                // TODO: render an app icon here (parse the
                                // bundle's .icns and decode to an iced
                                // image::Handle). Skipped in v1 to avoid the
                                // extra dep + per-app I/O on modal open.
                                let row_widget = row![
                                    name_cell,
                                    button(
                                        text("Launch")
                                            .size(10)
                                            .align_x(Horizontal::Center)
                                            .width(Length::Fill),
                                    )
                                    .on_press(Message::LaunchApp(path))
                                    .padding(Padding::from([2, 0]))
                                    .width(Length::Fixed(70.0))
                                    .style(button::primary),
                                ]
                                .spacing(8)
                                .align_y(iced::Alignment::Center);
                                col.push(
                                    container(row_widget)
                                        .padding(Padding::from([3, 8]))
                                        .style(move |theme: &Theme| container::Style {
                                            background: if highlighted {
                                                Some(
                                                    theme
                                                        .extended_palette()
                                                        .primary
                                                        .weak
                                                        .color
                                                        .into(),
                                                )
                                            } else {
                                                None
                                            },
                                            border: Border {
                                                color: Color::TRANSPARENT,
                                                width: 1.0,
                                                radius: 4.0.into(),
                                            },
                                            ..Default::default()
                                        }),
                                )
                            },
                        );
                        scrollable(list_col).height(Length::Fixed(360.0)).into()
                    }
                }
            };

            column![
                text("Launch Application").size(15),
                filter_widget,
                body,
                text("Type to filter  ·  ↑/↓ select  ·  Enter or click Launch  ·  Esc dismisses")
                    .size(11),
            ]
            .spacing(6)
        }
        Prompt::GitBranches {
            state,
            input,
            selected,
            repo_path: _,
        } => {
            let filter_widget = text_input("filter branches…", input)
                .id(text_input::Id::new(PROMPT_ID))
                .on_input(Message::PromptChanged)
                .on_submit(Message::PromptSubmit)
                .padding(8);

            let body: Element<'_, Message> = match state {
                GitBranchesState::Loading => container(text("Loading…").size(12))
                    .padding(Padding::from([8, 8]))
                    .into(),
                GitBranchesState::Error(msg) => container(
                    text(msg.clone())
                        .font(Font::MONOSPACE)
                        .size(11)
                        .style(|theme: &Theme| iced::widget::text::Style {
                            color: Some(theme.extended_palette().danger.base.color),
                        }),
                )
                .padding(Padding::from([8, 8]))
                .into(),
                GitBranchesState::Loaded(branches) => {
                    let filtered = filtered_branches(branches, input);
                    if filtered.is_empty() {
                        container(text("No branches match.").size(12))
                            .padding(Padding::from([8, 8]))
                            .into()
                    } else {
                        let list_col = filtered.iter().enumerate().fold(
                            column![].spacing(3),
                            |col, (i, branch)| {
                                let name = branch.name.clone();
                                let highlighted = i == *selected;
                                let name_cell = text(branch.name.clone())
                                    .font(Font::MONOSPACE)
                                    .size(12)
                                    .width(Length::Fill)
                                    .wrapping(iced::widget::text::Wrapping::None);
                                let date_cell = text(branch.last_commit.clone())
                                    .font(Font::MONOSPACE)
                                    .size(11)
                                    .width(Length::Fixed(110.0))
                                    .style(|theme: &Theme| iced::widget::text::Style {
                                        color: Some(dim(theme.extended_palette().background.base.text)),
                                    });
                                let row_widget = row![
                                    name_cell,
                                    date_cell,
                                    button(
                                        text("Checkout")
                                            .size(10)
                                            .align_x(Horizontal::Center)
                                            .width(Length::Fill),
                                    )
                                    .on_press(Message::GitCheckout(name))
                                    .padding(Padding::from([2, 0]))
                                    .width(Length::Fixed(80.0))
                                    .style(button::primary),
                                ]
                                .spacing(8)
                                .align_y(iced::Alignment::Center);
                                col.push(
                                    container(row_widget)
                                        .padding(Padding::from([3, 8]))
                                        .style(move |theme: &Theme| container::Style {
                                            background: if highlighted {
                                                Some(
                                                    theme
                                                        .extended_palette()
                                                        .primary
                                                        .weak
                                                        .color
                                                        .into(),
                                                )
                                            } else {
                                                None
                                            },
                                            border: Border {
                                                color: Color::TRANSPARENT,
                                                width: 1.0,
                                                radius: 4.0.into(),
                                            },
                                            ..Default::default()
                                        }),
                                )
                            },
                        );
                        scrollable(list_col).height(Length::Fixed(360.0)).into()
                    }
                }
            };

            column![
                text("Git: branch").size(15),
                filter_widget,
                body,
                text("Type to filter  ·  ↑/↓ select  ·  Enter or click Checkout  ·  Esc dismisses")
                    .size(11),
            ]
            .spacing(6)
        }
        Prompt::SshServers {
            state,
            input,
            selected,
        } => {
            let filter_widget = text_input("filter by alias or hostname…", input)
                .id(text_input::Id::new(PROMPT_ID))
                .on_input(Message::PromptChanged)
                .on_submit(Message::PromptSubmit)
                .padding(8);

            let body: Element<'_, Message> = match state {
                SshServersState::Loading => container(text("Loading…").size(12))
                    .padding(Padding::from([8, 8]))
                    .into(),
                SshServersState::Error(msg) => container(
                    text(msg.clone())
                        .font(Font::MONOSPACE)
                        .size(11)
                        .style(|theme: &Theme| iced::widget::text::Style {
                            color: Some(theme.extended_palette().danger.base.color),
                        }),
                )
                .padding(Padding::from([8, 8]))
                .into(),
                SshServersState::Loaded(servers) => {
                    let filtered = filtered_servers(servers, input);
                    if filtered.is_empty() {
                        container(text("No servers match.").size(12))
                            .padding(Padding::from([8, 8]))
                            .into()
                    } else {
                        let list_col = filtered.iter().enumerate().fold(
                            column![].spacing(3),
                            |col, (i, server)| {
                                let alias = server.alias.clone();
                                let highlighted = i == *selected;
                                let user_host = match (&server.user, &server.hostname) {
                                    (Some(u), Some(h)) => format!("{}@{}", u, h),
                                    (None, Some(h)) => h.clone(),
                                    (Some(u), None) => format!("{}@?", u),
                                    (None, None) => String::new(),
                                };
                                let identity = server
                                    .identity_file
                                    .clone()
                                    .unwrap_or_default();
                                let alias_cell = text(server.alias.clone())
                                    .font(Font::MONOSPACE)
                                    .size(11)
                                    .width(Length::FillPortion(3))
                                    .wrapping(iced::widget::text::Wrapping::None);
                                let host_cell = text(user_host)
                                    .font(Font::MONOSPACE)
                                    .size(11)
                                    .width(Length::FillPortion(4))
                                    .wrapping(iced::widget::text::Wrapping::None);
                                let identity_cell = text(identity)
                                    .font(Font::MONOSPACE)
                                    .size(11)
                                    .width(Length::FillPortion(3))
                                    .wrapping(iced::widget::text::Wrapping::None)
                                    .style(|theme: &Theme| iced::widget::text::Style {
                                        color: Some(dim(theme.extended_palette().background.base.text)),
                                    });
                                let row_widget = row![
                                    alias_cell,
                                    host_cell,
                                    identity_cell,
                                    button(
                                        text("Connect")
                                            .size(10)
                                            .align_x(Horizontal::Center)
                                            .width(Length::Fill),
                                    )
                                    .on_press(Message::SshConnect(alias))
                                    .padding(Padding::from([2, 0]))
                                    .width(Length::Fixed(80.0))
                                    .style(button::primary),
                                ]
                                .spacing(8)
                                .align_y(iced::Alignment::Center);
                                col.push(
                                    container(row_widget)
                                        .padding(Padding::from([3, 8]))
                                        .style(move |theme: &Theme| container::Style {
                                            background: if highlighted {
                                                Some(
                                                    theme
                                                        .extended_palette()
                                                        .primary
                                                        .weak
                                                        .color
                                                        .into(),
                                                )
                                            } else {
                                                None
                                            },
                                            border: Border {
                                                color: Color::TRANSPARENT,
                                                width: 1.0,
                                                radius: 4.0.into(),
                                            },
                                            ..Default::default()
                                        }),
                                )
                            },
                        );
                        scrollable(list_col).height(Length::Fixed(360.0)).into()
                    }
                }
            };

            column![
                text("Connect to SSH server").size(15),
                filter_widget,
                body,
                text("Type to filter  ·  ↑/↓ select  ·  Enter or click Connect  ·  Esc dismisses")
                    .size(11),
            ]
            .spacing(6)
        }
        Prompt::KeyboardShortcuts => {
            // Two-column key/description rows, grouped by section. Static
            // content from `keyboard_shortcuts()`; keep in sync with the
            // mdBook chapter at docs/src/keybindings.md.
            let mut body = column![].spacing(10);
            for (section, bindings) in keyboard_shortcuts() {
                let mut sect = column![text(section).size(13)].spacing(2);
                for (keys, description) in bindings {
                    sect = sect.push(
                        row![
                            text(keys)
                                .font(Font::MONOSPACE)
                                .size(11)
                                .width(Length::Fixed(220.0)),
                            text(description).size(11).width(Length::Fill),
                        ]
                        .spacing(8),
                    );
                }
                body = body.push(sect);
            }
            column![
                text("Keyboard shortcuts").size(15),
                scrollable(container(body).padding(Padding::from([4, 4])))
                    .height(Length::Fixed(420.0)),
                text("Esc dismisses").size(11),
            ]
            .spacing(8)
        }
    })
    .padding(16)
    .width(Length::Fixed(modal_width))
    .style(modal_style);

    let dialog = mouse_area(dialog_inner).on_press(Message::NoOp);

    let backdrop = mouse_area(
        container(text(""))
            .width(Length::Fill)
            .height(Length::Fill)
            .style(backdrop_style),
    )
    .on_press(Message::PromptCancel);

    let centered = container(dialog)
        .center_x(Length::Fill)
        .center_y(Length::Fill);

    stack![backdrop, centered].into()
}

/// Clickable column header for the Docker modal. Shows a `↑`/`↓` arrow on
/// whichever column is currently the active sort.
///
/// Font + horizontal padding are deliberately matched to the row cells
/// below — monospace, size 11, zero horizontal button-padding — so the
/// column-left of every header sits at the same x as the column-left of
/// every cell.
fn docker_header<'a>(
    column: DockerSortBy,
    active: DockerSortBy,
    dir: SortDir,
    width: Length,
) -> Element<'a, Message> {
    let arrow = if active == column {
        match dir {
            SortDir::Asc => " ↑",
            SortDir::Desc => " ↓",
        }
    } else {
        ""
    };
    button(
        text(format!("{}{}", column.label(), arrow))
            .font(Font::MONOSPACE)
            .size(11)
            .width(Length::Fill),
    )
    .width(width)
    .padding(Padding::from([2, 0]))
    .style(button::text)
    .on_press(Message::DockerToggleSort(column))
    .into()
}

/// Clickable column header for the Processes modal. Same pattern as
/// [`docker_header`] — monospace + zero horizontal padding so headers
/// align with the row cells.
fn process_header<'a>(
    column: ProcessSortBy,
    active: ProcessSortBy,
    dir: SortDir,
    width: Length,
) -> Element<'a, Message> {
    let arrow = if active == column {
        match dir {
            SortDir::Asc => " ↑",
            SortDir::Desc => " ↓",
        }
    } else {
        ""
    };
    button(
        text(format!("{}{}", column.label(), arrow))
            .font(Font::MONOSPACE)
            .size(11)
            .width(Length::Fill),
    )
    .width(width)
    .padding(Padding::from([2, 0]))
    .style(button::text)
    .on_press(Message::ProcessToggleSort(column))
    .into()
}

fn modal_style(theme: &Theme) -> container::Style {
    let palette = theme.extended_palette();
    container::Style {
        background: Some(palette.background.base.color.into()),
        border: Border {
            color: palette.background.strong.color,
            width: 1.0,
            radius: 8.0.into(),
        },
        shadow: Shadow {
            color: Color::from_rgba(0.0, 0.0, 0.0, 0.5),
            offset: Vector::new(0.0, 6.0),
            blur_radius: 24.0,
        },
        ..Default::default()
    }
}

fn backdrop_style(_theme: &Theme) -> container::Style {
    container::Style {
        background: Some(Color::from_rgba(0.0, 0.0, 0.0, 0.45).into()),
        ..Default::default()
    }
}
