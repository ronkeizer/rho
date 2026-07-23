//! Modal rendering. `view_modal()` is one giant match on `Prompt` —
//! every variant gets its own branch. The `docker_header` /
//! `process_header` helpers and the `modal_style` / `backdrop_style`
//! container styles live here too because they're only used by modals.

use iced::alignment::Horizontal;
use iced::widget::{
    button, column, container, mouse_area, row, scrollable, stack, text, text_input, Space,
};
use iced::{Border, Color, Element, Font, Length, Padding, Shadow, Theme, Vector};

use crate::config::{blend, dim, RowColors};
use crate::config::FtpPerms;
use crate::domain::{
    bar_glyph, filtered_actions, filtered_apps, filtered_branches, filtered_containers,
    filtered_paperless, filtered_processes, filtered_recents, filtered_servers,
    format_log_timestamp,
    keyboard_shortcuts, palette_action_enabled, AppsState,
    DeleteFocus, DockerSortBy,
    DockerState, FileChoice, FtpInfoFocus, FtpLogEntry, FtpLogLevel, FtpReplaceFocus,
    FtpServerInfo, GitBranchesState, NewFilesFocus, PaperlessState, ProcessSortBy, ProcessesState,
    Prompt, Side, SortDir, SshServersState, StatSample,
};
use crate::view_pane::format_size;
use crate::{Message, MODAL_LIST_ID, PROMPT_ID};

pub fn view_modal<'a>(
    prompt: &'a Prompt,
    colors: RowColors,
    ftp_log: &'a std::collections::VecDeque<FtpLogEntry>,
    stats: &'a std::collections::VecDeque<StatSample>,
    stats_interval: u64,
) -> Element<'a, Message> {
    // Docker / Processes / Apps / GitBranches are wider so the list rows
    // have room. KeyboardShortcuts is two-column text — wider than the
    // single-column modals but narrower than the table-shaped ones. FtpInfo
    // hosts a wide log table, so it grows similarly.
    let modal_width = match prompt {
        Prompt::Docker { .. }
        | Prompt::Processes { .. }
        | Prompt::Apps { .. }
        | Prompt::GitBranches { .. }
        | Prompt::SshServers { .. }
        | Prompt::Paperless { .. }
        | Prompt::SystemMonitor => 720.0,
        Prompt::KeyboardShortcuts => 600.0,
        Prompt::FtpInfo { .. } => 640.0,
        Prompt::FileActions { .. } => 520.0,
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
                    .id(scrollable::Id::new(MODAL_LIST_ID))
                    .on_scroll(|v| {
                        Message::ModalScrolled(
                            v.absolute_offset().y,
                            v.bounds().height,
                            v.content_bounds().height,
                        )
                    })
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
        Prompt::NewFolder { input } => column![
            text("New folder").size(15),
            text_input("folder name", input)
                .id(text_input::Id::new(PROMPT_ID))
                .on_input(Message::PromptChanged)
                .on_submit(Message::PromptSubmit)
                .padding(8),
            text("Created in the active pane  ·  Enter to create  ·  Esc cancels").size(11),
        ]
        .spacing(10),
        Prompt::NewFile { input } => column![
            text("New file").size(15),
            text_input("file name", input)
                .id(text_input::Id::new(PROMPT_ID))
                .on_input(Message::PromptChanged)
                .on_submit(Message::PromptSubmit)
                .padding(8),
            text("Created in the active pane, opens in your editor  ·  Enter to create  ·  Esc cancels")
                .size(11),
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
            dropbox_configured,
            paperless_configured,
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
                        let enabled = palette_action_enabled(
                            *action,
                            *dropbox_configured,
                            *paperless_configured,
                        );
                        // Same idea as the Open modal: highlight only the
                        // active row; the rest inherit the modal background.
                        // Disabled rows (e.g. Open Dropbox with no creds) stay
                        // listed but greyed and non-clickable so the feature is
                        // discoverable.
                        let style: fn(&Theme, button::Status) -> button::Style = if i == *selected {
                            button::primary
                        } else if enabled {
                            button::text
                        } else {
                            palette_disabled_style
                        };
                        // Disabled rows carry a trailing hint pointing at the
                        // config; enabled rows are just the label.
                        // Monospace to match the file-list rows and the other
                        // filterable modals (e.g. Go to folder).
                        let label: Element<'_, Message> = if enabled {
                            text(action.label()).font(Font::MONOSPACE).size(12).into()
                        } else {
                            row![
                                text(action.label()).font(Font::MONOSPACE).size(12),
                                Space::with_width(Length::Fill),
                                text("set credentials in ~/.rho.yaml")
                                    .font(Font::MONOSPACE)
                                    .size(10),
                            ]
                            .align_y(iced::alignment::Vertical::Center)
                            .into()
                        };
                        let mut btn = button(label)
                            .padding(Padding::from([2, 12]))
                            .width(Length::Fill)
                            .style(style);
                        if enabled {
                            btn = btn.on_press(Message::PaletteSelect(*action));
                        }
                        col.push(btn)
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
                    .path()
                    .file_name()
                    .map(|n| n.to_string_lossy().into_owned())
                    .unwrap_or_else(|| paths[0].to_string());
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
                        scrollable(list_col)
                            .id(scrollable::Id::new(MODAL_LIST_ID))
                            .on_scroll(|v| {
                                Message::ModalScrolled(
                                    v.absolute_offset().y,
                                    v.bounds().height,
                                    v.content_bounds().height,
                                )
                            })
                            .height(Length::Fixed(360.0))
                            .into()
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
            selected,
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
                        let list_col = filtered.iter().enumerate().fold(
                            column![],
                            |col, (row_index, p)| {
                            let pid = p.pid;
                            let is_selected = row_index == *selected;
                            // On the highlighted (blue) row every cell uses the
                            // cursor foreground so it stays legible; off-row,
                            // the metric columns are dimmed like before.
                            let dim_style = move |theme: &Theme| iced::widget::text::Style {
                                color: Some(if is_selected {
                                    cursor_fill(theme, colors).1
                                } else {
                                    dim(theme.extended_palette().background.base.text)
                                }),
                            };
                            let name_style = move |theme: &Theme| iced::widget::text::Style {
                                color: Some(if is_selected {
                                    cursor_fill(theme, colors).1
                                } else {
                                    theme.extended_palette().background.base.text
                                }),
                            };
                            let name_cell = text(p.name.clone())
                                .font(Font::MONOSPACE)
                                .size(11)
                                .width(Length::Fill)
                                .wrapping(iced::widget::text::Wrapping::None)
                                .style(name_style);
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
                                    .style(move |theme: &Theme| container::Style {
                                        // The highlighted row gets the blue
                                        // cursor fill (like the panes); the rest
                                        // use the same odd/even zebra stripe the
                                        // panes use — no borders.
                                        background: if is_selected {
                                            Some(cursor_fill(theme, colors).0.into())
                                        } else {
                                            process_stripe(theme, colors, row_index)
                                        },
                                        ..Default::default()
                                    }),
                            )
                        });
                        scrollable(list_col)
                            .id(scrollable::Id::new(MODAL_LIST_ID))
                            .on_scroll(|v| {
                                Message::ModalScrolled(
                                    v.absolute_offset().y,
                                    v.bounds().height,
                                    v.content_bounds().height,
                                )
                            })
                            .height(Length::Fixed(360.0))
                            .into()
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
        Prompt::Paperless {
            state,
            input,
            query,
            selected,
        } => {
            let filter_widget =
                text_input("filter titles · Enter = full-text search…", input)
                    .id(text_input::Id::new(PROMPT_ID))
                    .on_input(Message::PromptChanged)
                    .on_submit(Message::PromptSubmit)
                    .padding(8);
            // Static header (no sortable columns — order comes from the server:
            // recent-first, or by search score). Widths mirror the body rows.
            let header_row = container(
                row![
                    text("Title").size(11).width(Length::Fill),
                    text("Created").size(11).width(Length::Fixed(96.0)),
                    Space::with_width(Length::Fixed(158.0)),
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
                PaperlessState::Loading => container(text("Loading…").size(12))
                    .padding(Padding::from([8, 8]))
                    .into(),
                PaperlessState::Error(msg) => container(
                    text(msg.clone())
                        .font(Font::MONOSPACE)
                        .size(11)
                        .style(|theme: &Theme| iced::widget::text::Style {
                            color: Some(theme.extended_palette().danger.base.color),
                        }),
                )
                .padding(Padding::from([8, 8]))
                .into(),
                PaperlessState::Loaded(docs) if docs.is_empty() => {
                    container(text("No documents.").size(12))
                        .padding(Padding::from([8, 8]))
                        .into()
                }
                PaperlessState::Loaded(docs) => {
                    let filtered = filtered_paperless(docs, input);
                    if filtered.is_empty() {
                        container(text("No documents match.").size(12))
                            .padding(Padding::from([8, 8]))
                            .into()
                    } else {
                        let list_col = filtered.iter().enumerate().fold(
                            column![],
                            |col, (row_index, d)| {
                            let id = d.id;
                            let is_selected = row_index == *selected;
                            let created_style = move |theme: &Theme| iced::widget::text::Style {
                                color: Some(if is_selected {
                                    cursor_fill(theme, colors).1
                                } else {
                                    dim(theme.extended_palette().background.base.text)
                                }),
                            };
                            let title_style = move |theme: &Theme| iced::widget::text::Style {
                                color: Some(if is_selected {
                                    cursor_fill(theme, colors).1
                                } else {
                                    theme.extended_palette().background.base.text
                                }),
                            };
                            let title_cell = text(d.title.clone())
                                .size(11)
                                .width(Length::Fill)
                                .wrapping(iced::widget::text::Wrapping::None)
                                .style(title_style);
                            let created_cell = text(d.created.clone())
                                .font(Font::MONOSPACE)
                                .size(11)
                                .width(Length::Fixed(96.0))
                                .style(created_style);
                            let row_widget = row![
                                title_cell,
                                created_cell,
                                button(
                                    text("Open")
                                        .size(10)
                                        .align_x(Horizontal::Center)
                                        .width(Length::Fill),
                                )
                                .on_press(Message::PaperlessOpen(id))
                                .padding(Padding::from([2, 0]))
                                .width(Length::Fixed(60.0))
                                .style(button::primary),
                                button(
                                    text("Download")
                                        .size(10)
                                        .align_x(Horizontal::Center)
                                        .width(Length::Fill),
                                )
                                .on_press(Message::PaperlessDownload(id))
                                .padding(Padding::from([2, 0]))
                                .width(Length::Fixed(90.0))
                                .style(button::secondary),
                            ]
                            .spacing(8)
                            .align_y(iced::Alignment::Center);
                            col.push(
                                container(row_widget)
                                    .padding(Padding::from([3, 8]))
                                    .style(move |theme: &Theme| container::Style {
                                        background: if is_selected {
                                            Some(cursor_fill(theme, colors).0.into())
                                        } else {
                                            process_stripe(theme, colors, row_index)
                                        },
                                        ..Default::default()
                                    }),
                            )
                        });
                        scrollable(list_col)
                            .id(scrollable::Id::new(MODAL_LIST_ID))
                            .on_scroll(|v| {
                                Message::ModalScrolled(
                                    v.absolute_offset().y,
                                    v.bounds().height,
                                    v.content_bounds().height,
                                )
                            })
                            .height(Length::Fixed(360.0))
                            .into()
                    }
                }
            };

            let title = if query.trim().is_empty() {
                "Paperless documents".to_string()
            } else {
                format!("Paperless documents  ·  full-text: “{}”", query.trim())
            };
            column![
                text(title).size(15),
                filter_widget,
                header_row,
                body,
                text("Type to filter loaded docs  ·  Enter = full-text search  ·  Open (browser) / Download  ·  Esc dismisses").size(11),
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
                        scrollable(list_col)
                            .id(scrollable::Id::new(MODAL_LIST_ID))
                            .on_scroll(|v| {
                                Message::ModalScrolled(
                                    v.absolute_offset().y,
                                    v.bounds().height,
                                    v.content_bounds().height,
                                )
                            })
                            .height(Length::Fixed(360.0))
                            .into()
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
                        scrollable(list_col)
                            .id(scrollable::Id::new(MODAL_LIST_ID))
                            .on_scroll(|v| {
                                Message::ModalScrolled(
                                    v.absolute_offset().y,
                                    v.bounds().height,
                                    v.content_bounds().height,
                                )
                            })
                            .height(Length::Fixed(360.0))
                            .into()
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
                                let alias_for_open = server.alias.clone();
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
                                        text("Open")
                                            .size(10)
                                            .align_x(Horizontal::Center)
                                            .width(Length::Fill),
                                    )
                                    .on_press(Message::SshOpenInPane(alias_for_open))
                                    .padding(Padding::from([2, 0]))
                                    .width(Length::Fixed(70.0))
                                    .style(button::secondary),
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
                        scrollable(list_col)
                            .id(scrollable::Id::new(MODAL_LIST_ID))
                            .on_scroll(|v| {
                                Message::ModalScrolled(
                                    v.absolute_offset().y,
                                    v.bounds().height,
                                    v.content_bounds().height,
                                )
                            })
                            .height(Length::Fixed(360.0))
                            .into()
                    }
                }
            };

            column![
                text("Connect to SSH server").size(15),
                filter_widget,
                body,
                text("Type to filter  ·  ↑/↓ select  ·  Enter / Connect launches terminal  ·  Open lists the host in this pane  ·  Esc dismisses")
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
        Prompt::SystemMonitor => {
            // Two unicode-block sparklines (CPU on top, memory below) fed by
            // the always-on background sampler. Only the tail that fits the
            // modal width is drawn; the numeric current/peak read-outs above
            // each line cover the exact values.
            const MAX_BARS: usize = 64;
            const CPU_COLOR: Color = Color::from_rgb(0.40, 0.80, 0.45);
            const MEM_COLOR: Color = Color::from_rgb(0.42, 0.62, 1.0);

            let cadence = stats_interval.max(1);

            let sparkline = |pick: fn(&StatSample) -> f32| -> String {
                let n = stats.len();
                let start = n.saturating_sub(MAX_BARS);
                stats.iter().skip(start).map(|s| bar_glyph(pick(s))).collect()
            };
            let peak = |pick: fn(&StatSample) -> f32| -> f32 {
                stats.iter().map(pick).fold(0.0_f32, f32::max)
            };
            let cur = |pick: fn(&StatSample) -> f32| -> f32 {
                stats.back().map(pick).unwrap_or(0.0)
            };

            let metric = |label: &str,
                          color: Color,
                          pick: fn(&StatSample) -> f32|
             -> Element<'a, Message> {
                let header = text(format!(
                    "{}   {:>5.1}%   (peak {:>5.1}%)",
                    label,
                    cur(pick),
                    peak(pick)
                ))
                .font(Font::MONOSPACE)
                .size(12);
                let line = text(sparkline(pick))
                    .font(Font::MONOSPACE)
                    .size(15)
                    .color(color)
                    .width(Length::Fill);
                column![header, line].spacing(4).into()
            };

            let graphs: Element<'a, Message> = if stats.is_empty() {
                text("Collecting samples…").size(12).into()
            } else {
                column![
                    metric("CPU", CPU_COLOR, |s| s.cpu),
                    metric("Memory", MEM_COLOR, |s| s.mem),
                ]
                .spacing(14)
                .into()
            };

            column![
                text("System monitor").size(15),
                graphs,
                text(format!(
                    "sampling every {}s · {} samples · Esc to close",
                    cadence,
                    stats.len()
                ))
                .size(11),
            ]
            .spacing(12)
        }
        Prompt::FileActions {
            path,
            choices,
            selected,
            edit,
        } => {
            let file_name = path
                .file_name()
                .and_then(|s| s.to_str())
                .unwrap_or("")
                .to_string();
            let list_col = choices.iter().enumerate().fold(
                column![].spacing(2),
                |col, (i, choice)| {
                    let highlighted = i == *selected;
                    // Primary line: the choice label. Secondary (dim) line:
                    // the command, or a hint for the built-in default-open.
                    let (subtitle, terminal) = match choice {
                        FileChoice::OpenDefault => ("opens in the OS default app".to_string(), false),
                        FileChoice::Custom {
                            command, terminal, ..
                        } => (command.clone(), *terminal),
                    };
                    let label_line = if terminal {
                        format!("terminal: {}", choice.label())
                    } else {
                        choice.label().to_string()
                    };
                    // Only the highlighted row can be expanded, and only
                    // `Custom` rows have a command worth editing.
                    let editing = highlighted && edit.is_some() && matches!(choice, FileChoice::Custom { .. });
                    if editing {
                        let editor = text_input("command…", edit.as_deref().unwrap_or(""))
                            .id(text_input::Id::new(PROMPT_ID))
                            .font(Font::MONOSPACE)
                            .size(11)
                            .padding(4)
                            .on_input(Message::PromptChanged)
                            .on_submit(Message::PromptSubmit);
                        let row_body = column![text(label_line).size(13), editor].spacing(4);
                        col.push(
                            container(row_body)
                                .padding(Padding::from([6, 8]))
                                .width(Length::Fill)
                                .style(|theme: &Theme| container::Style {
                                    background: Some(
                                        theme.extended_palette().primary.weak.color.into(),
                                    ),
                                    border: Border {
                                        color: theme.extended_palette().primary.strong.color,
                                        width: 1.0,
                                        radius: 4.0.into(),
                                    },
                                    ..Default::default()
                                }),
                        )
                    } else {
                        let row_body = column![
                            text(label_line).size(13),
                            text(subtitle)
                                .font(Font::MONOSPACE)
                                .size(10)
                                .wrapping(iced::widget::text::Wrapping::None)
                                .style(|theme: &Theme| iced::widget::text::Style {
                                    color: Some(dim(theme.extended_palette().background.base.text)),
                                }),
                        ]
                        .spacing(2);
                        let style: fn(&Theme, button::Status) -> button::Style = if highlighted {
                            button::primary
                        } else {
                            button::text
                        };
                        col.push(
                            button(row_body)
                                .on_press(Message::FileChoiceActivate(i))
                                .padding(Padding::from([6, 8]))
                                .width(Length::Fill)
                                .style(style),
                        )
                    }
                },
            );
            column![
                text(format!("Open  {}", file_name)).size(15),
                scrollable(list_col)
                    .id(scrollable::Id::new(MODAL_LIST_ID))
                    .on_scroll(|v| {
                        Message::ModalScrolled(
                            v.absolute_offset().y,
                            v.bounds().height,
                            v.content_bounds().height,
                        )
                    })
                    .height(Length::Shrink),
                text("↑/↓ select  ·  Tab expand/edit command  ·  Enter or click to open  ·  Esc cancels").size(11),
            ]
            .spacing(8)
        }
        Prompt::FtpInfo { info, focus } => {
            let focus = *focus;
            let stop_style: fn(&Theme, button::Status) -> button::Style =
                if focus == FtpInfoFocus::Stop {
                    button::danger
                } else {
                    button::secondary
                };
            let close_style: fn(&Theme, button::Status) -> button::Style =
                if focus == FtpInfoFocus::Close {
                    button::primary
                } else {
                    button::secondary
                };

            let actions = row![
                button(text("Stop server"))
                    .on_press(Message::FtpServerStopRequest)
                    .padding(Padding::from([6, 16]))
                    .style(stop_style),
                button(text("Close"))
                    .on_press(Message::PromptCancel)
                    .padding(Padding::from([6, 16]))
                    .style(close_style),
            ]
            .spacing(10);

            column![
                text("FTP server running").size(15),
                ftp_detail_rows(info),
                ftp_log_panel(ftp_log),
                actions,
                text("Tab or ←/→ switch  ·  Enter activate  ·  Esc closes (server keeps running)")
                    .size(11),
            ]
            .spacing(10)
        }
        Prompt::FtpReplace {
            current,
            new_root,
            focus,
        } => {
            let focus = *focus;
            let cancel_style: fn(&Theme, button::Status) -> button::Style =
                if focus == FtpReplaceFocus::Cancel {
                    button::primary
                } else {
                    button::secondary
                };
            let replace_style: fn(&Theme, button::Status) -> button::Style =
                if focus == FtpReplaceFocus::Replace {
                    button::danger
                } else {
                    button::secondary
                };

            let actions = row![
                button(text("Cancel"))
                    .on_press(Message::PromptCancel)
                    .padding(Padding::from([6, 16]))
                    .style(cancel_style),
                button(text("Stop and restart here"))
                    .on_press(Message::FtpServerReplaceConfirmed(new_root.clone()))
                    .padding(Padding::from([6, 16]))
                    .style(replace_style),
            ]
            .spacing(10);

            let body = column![
                text(format!(
                    "An FTP server is already running on {}.",
                    current.display_addr()
                )),
                text(format!("Currently serving: {}", current.root.display())),
                text(format!("New root would be:  {}", new_root.display())),
                text("Stop the running server and restart it on the new folder?"),
            ]
            .spacing(4);

            column![
                text("Replace running FTP server?").size(15),
                body,
                actions,
                text("Tab or ←/→ switch  ·  Enter activate  ·  Esc cancels").size(11),
            ]
            .spacing(10)
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
/// Build the labeled detail rows for the FTP info modal. Each row is a
/// label + monospace value pair; the credentials need to be typed verbatim
/// into an FTP client, so the value font matters more than the label one.
/// Returns `Element` rather than a concrete `Column` so the caller can drop
/// it into a `column!` macro without thinking about type coercion.
fn ftp_detail_rows<'a>(info: &FtpServerInfo) -> Element<'a, Message> {
    let perms = match info.permissions {
        FtpPerms::ReadOnly => "read-only",
        FtpPerms::ReadWrite => "read-write",
    };
    let cred_line = |label: &'static str, value: String| -> iced::widget::Row<'a, Message> {
        row![
            text(label).size(12).width(Length::Fixed(95.0)),
            text(value).font(Font::MONOSPACE).size(12),
        ]
        .spacing(8)
    };
    let mut col = column![
        cred_line("Address", info.display_addr()),
        cred_line("Root", info.root.display().to_string()),
        cred_line("Permissions", perms.to_string()),
    ]
    .spacing(4);
    if let Some(user) = &info.username {
        col = col.push(cred_line("Username", user.clone()));
    }
    if let Some(pass) = &info.password {
        col = col.push(cred_line("Password", pass.clone()));
    }
    if info.username.is_none() && info.password.is_none() {
        col = col.push(cred_line("Auth", "anonymous (no credentials)".to_string()));
    }
    col.into()
}

/// Streaming log panel for the FTP info modal. Renders newest entries at
/// the top (no need to fight scroll position to follow new events) inside
/// a fixed-height scrollable, so the modal itself doesn't grow unbounded
/// as activity accumulates. Each row is `HH:MM:SS <tag> <message>` in a
/// monospace font; the tag + message inherit a per-level color.
fn ftp_log_panel<'a>(entries: &'a std::collections::VecDeque<FtpLogEntry>) -> Element<'a, Message> {
    if entries.is_empty() {
        return container(text("waiting for client activity…").size(11).style(
            |theme: &Theme| iced::widget::text::Style {
                color: Some(dim(theme.extended_palette().background.base.text)),
            },
        ))
        .padding(Padding::from([6, 8]))
        .width(Length::Fill)
        .into();
    }

    // Build a column of rows, newest first. The deque can be split across
    // its ring buffer, so iter().rev() goes through both halves correctly.
    let mut rows = column![].spacing(2);
    for entry in entries.iter().rev() {
        rows = rows.push(ftp_log_row(entry));
    }

    scrollable(rows)
        .height(Length::Fixed(160.0))
        .width(Length::Fill)
        .into()
}

/// One log line. The `style` closure picks a color from the iced palette
/// per [`FtpLogLevel`] — Auth dims slightly (less noisy than every other
/// entry), Warn goes amber, Error red. Plain Info keeps the default
/// text color so it doesn't compete with the brackets.
fn ftp_log_row<'a>(entry: &'a FtpLogEntry) -> Element<'a, Message> {
    let tag = match entry.level {
        FtpLogLevel::Info => " · ",
        FtpLogLevel::Auth => " ↪ ",
        FtpLogLevel::Warn => " ! ",
        FtpLogLevel::Error => " ✗ ",
    };
    let level = entry.level;
    let line_style = move |theme: &Theme| {
        let palette = theme.extended_palette();
        let color = match level {
            FtpLogLevel::Info => palette.background.base.text,
            FtpLogLevel::Auth => dim(palette.background.base.text),
            FtpLogLevel::Warn => Color::from_rgb(0.95, 0.75, 0.30),
            FtpLogLevel::Error => Color::from_rgb(1.0, 0.45, 0.45),
        };
        iced::widget::text::Style { color: Some(color) }
    };
    row![
        text(format_log_timestamp(entry.ts))
            .font(Font::MONOSPACE)
            .size(11)
            .style(move |theme: &Theme| iced::widget::text::Style {
                color: Some(dim(theme.extended_palette().background.base.text)),
            }),
        text(tag).font(Font::MONOSPACE).size(11).style(line_style),
        text(&entry.message)
            .font(Font::MONOSPACE)
            .size(11)
            .style(line_style),
    ]
    .spacing(2)
    .into()
}

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

/// Zebra-stripe background for a process row, mirroring the panes'
/// `compute_row_style`: odd rows get `colors.stripe` (falling back to a blend
/// of the base and weak background, the same default the panes use), even rows
/// inherit the modal background (`None`).
fn process_stripe(theme: &Theme, colors: RowColors, row_index: usize) -> Option<iced::Background> {
    if row_index % 2 == 1 {
        let palette = theme.extended_palette();
        let stripe = colors.stripe.unwrap_or_else(|| {
            blend(
                palette.background.base.color,
                palette.background.weak.color,
                0.15,
            )
        });
        Some(stripe.into())
    } else {
        None
    }
}

/// `(background, foreground)` for the highlighted "current" process row,
/// mirroring the panes' cursor styling in `compute_row_style`: `colors.cursor`
/// (or the theme's primary-strong) as the fill, with a contrasting text color.
fn cursor_fill(theme: &Theme, colors: RowColors) -> (Color, Color) {
    let pair = theme.extended_palette().primary.strong;
    let bg = colors.cursor.unwrap_or(pair.color);
    let fg = if colors.cursor.is_some() {
        Color::WHITE
    } else {
        pair.text
    };
    (bg, fg)
}

/// Style for a non-selected, disabled command-palette row: same flat
/// background as `button::text`, but with dimmed text so it reads as
/// unavailable. Selected rows use `button::primary` regardless of enablement.
fn palette_disabled_style(theme: &Theme, status: button::Status) -> button::Style {
    button::Style {
        text_color: dim(theme.extended_palette().background.base.text),
        ..button::text(theme, status)
    }
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

#[cfg(test)]
mod tests {
    use super::*;
    use iced::Background;

    #[test]
    fn process_stripe_alternates_with_configured_color() {
        let stripe = Color::from_rgb(0.1, 0.2, 0.3);
        let colors = RowColors {
            cursor: None,
            mark: None,
            stripe: Some(stripe),
            folder: None,
            action: None,
        };
        let theme = Theme::Dark;
        // Even rows inherit the modal background (no override).
        assert!(process_stripe(&theme, colors, 0).is_none());
        assert!(process_stripe(&theme, colors, 2).is_none());
        // Odd rows get the configured stripe color verbatim.
        match process_stripe(&theme, colors, 1) {
            Some(Background::Color(c)) => assert_eq!(c, stripe),
            other => panic!("expected configured stripe, got {:?}", other),
        }
    }

    #[test]
    fn process_stripe_falls_back_to_theme_blend_without_config() {
        let colors = RowColors {
            cursor: None,
            mark: None,
            stripe: None,
            folder: None,
            action: None,
        };
        let theme = Theme::Dark;
        // Odd rows still get a (theme-derived) stripe; even rows stay bare.
        assert!(process_stripe(&theme, colors, 1).is_some());
        assert!(process_stripe(&theme, colors, 0).is_none());
    }

    #[test]
    fn cursor_fill_uses_configured_cursor_color_with_white_text() {
        let cursor = Color::from_rgb(0.0, 0.4, 0.9);
        let colors = RowColors {
            cursor: Some(cursor),
            mark: None,
            stripe: None,
            folder: None,
            action: None,
        };
        let (bg, fg) = cursor_fill(&Theme::Dark, colors);
        assert_eq!(bg, cursor);
        // A configured cursor color pairs with white text for contrast.
        assert_eq!(fg, Color::WHITE);
    }

    #[test]
    fn cursor_fill_falls_back_to_theme_primary() {
        let colors = RowColors {
            cursor: None,
            mark: None,
            stripe: None,
            folder: None,
            action: None,
        };
        let pair = Theme::Dark.extended_palette().primary.strong;
        let (bg, fg) = cursor_fill(&Theme::Dark, colors);
        assert_eq!(bg, pair.color);
        assert_eq!(fg, pair.text);
    }
}
