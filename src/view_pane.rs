//! Pane rendering — everything between `App::view` and the modal stack.
//! `view_pane()` builds one of the two side-by-side directory views; the
//! row helpers (`build_row`, `compute_row_style`, etc.) and the under-list
//! bars (`git_info_bar`, `filter_bar`, `claude_info_bar`) all live here too.
//!
//! Also home to the small layout/formatter helpers (`name_max_chars`,
//! `viewport_height_estimate`, `scroll_id`, `format_size`, `format_modified`,
//! `truncate_with_ellipsis`) — they're called from `App` for scroll math as
//! well, but their shape is view-side, so they ship with the view code.

use std::time::SystemTime;

use chrono::{DateTime, Local};
use iced::alignment::Horizontal;
use iced::widget::{button, column, container, mouse_area, row, scrollable, text, Space};
use iced::{Border, Color, Element, Font, Length, Padding, Shadow, Theme};

use crate::config::{blend, dim, Config, RowColors};
use crate::domain::{file_has_custom_action, GitInfo, Pane, RowVisual, Side, SortBy, SortDir};
use crate::Message;

pub fn view_pane<'a>(
    side: Side,
    pane: &'a Pane,
    active: bool,
    name_max_chars: usize,
    config: &Config,
    colors: RowColors,
    window_height: f32,
) -> Element<'a, Message> {
    let path_header = text(pane.location.to_string())
        .font(Font::MONOSPACE)
        .size(config.row_font_size)
        .style(move |theme: &Theme| iced::widget::text::Style {
            color: if active {
                None
            } else {
                Some(dim(theme.extended_palette().background.base.text))
            },
        });

    // Match the body row's leading git-marker gutter so the "Name" header
    // sits over the actual names rather than the marker column.
    let name_header = row![
        Space::with_width(Length::Fixed(config.row_font_size as f32)),
        header_cell(
            side,
            "Name",
            SortBy::Name,
            pane,
            Length::Fill,
            Horizontal::Left,
            config,
            active,
        ),
    ]
    .spacing(0)
    .width(Length::Fill);

    let column_header = container(
        row![
            name_header,
            header_cell(
                side,
                "Size",
                SortBy::Size,
                pane,
                Length::Fixed(config.size_column_px),
                Horizontal::Right,
                config,
                active,
            ),
            header_cell(
                side,
                "Modified",
                SortBy::Modified,
                pane,
                Length::Fixed(config.modified_column_px),
                Horizontal::Left,
                config,
                active,
            ),
        ]
        .spacing(8),
    )
    .padding(Padding::from([0, 8]))
    .style(move |theme: &Theme| {
        let palette = theme.extended_palette();
        let bg = if active {
            palette.background.weak.color
        } else {
            dim(palette.background.weak.color)
        };
        container::Style {
            background: Some(bg.into()),
            ..Default::default()
        }
    });

    let pad_y = config.row_padding_y();
    let stride = config.row_height_px;
    let fallback_vh = viewport_height_estimate(window_height);
    let vh = pane.viewport_height.unwrap_or(fallback_vh);

    // Total list rows: the synthetic ".." row plus all visible entries.
    let total_rows = 1 + pane.visible_indices.len();

    // Virtual rendering: only build widgets for rows inside (or near) the
    // current viewport. `Space` spacers above and below fill the remaining
    // height so the scrollable's total content size — and therefore its
    // scrollbar position — stays accurate regardless of how many entries
    // exist. This keeps view() O(visible rows) instead of O(total entries),
    // which is the main reason large directories stall key-press handling.
    let first_row = (pane.scroll_y / stride).floor() as usize;
    // +2 overdraw: ensures partial rows at the top and bottom edge of the
    // viewport are never blank while the user scrolls.
    let last_row = ((pane.scroll_y + vh) / stride).ceil() as usize + 2;
    let first_row = first_row.min(total_rows);
    let last_row = last_row.min(total_rows);

    let top_space = first_row as f32 * stride;
    let bottom_space = total_rows.saturating_sub(last_row) as f32 * stride;

    // ".." row — treated as a directory for color purposes, never dimmed.
    let up_visual = row_visual(pane, 0);
    let up_name_color = entry_name_color(true, false, up_visual, active, colors);
    let up = build_row(
        0,
        "..".to_string(),
        String::new(),
        String::new(),
        Message::RowClicked(side, 0),
        up_visual,
        active,
        config,
        colors,
        pad_y,
        up_name_color,
        false,
        false,
    );

    let mut list = column![].spacing(0).push(Space::with_height(top_space));
    if first_row == 0 {
        list = list.push(up);
    }
    for row_idx in first_row.max(1)..last_row {
        let visible_pos = row_idx - 1;
        let &entry_idx = match pane.visible_indices.get(visible_pos) {
            Some(i) => i,
            None => break,
        };
        let entry = match pane.entries.get(entry_idx) {
            Some(e) => e,
            None => continue,
        };
        let name = if entry.is_dir {
            // Reserve one slot for the trailing '/' so dir names never exceed
            // name_max_chars after the suffix is appended.
            let budget = name_max_chars.saturating_sub(1).max(1);
            format!("{}/", truncate_with_ellipsis(&entry.name, budget))
        } else {
            truncate_with_ellipsis(&entry.name, name_max_chars)
        };
        let size_str = entry.size.map(format_size).unwrap_or_default();
        let mod_str = entry.modified.map(format_modified).unwrap_or_default();
        let visual = row_visual(pane, row_idx);
        let has_action = !entry.is_dir && file_has_custom_action(&config.file_actions, &entry.name);
        let name_color = entry_name_color(entry.is_dir, has_action, visual, active, colors);
        let in_selection = active && matches!(visual, RowVisual::Cursor | RowVisual::Marked);
        let dim_row = entry.name.starts_with('.') && !in_selection;
        let git_modified = pane
            .git_info
            .as_ref()
            .map(|i| i.modified_names.contains(&entry.name))
            .unwrap_or(false);
        list = list.push(build_row(
            row_idx,
            name,
            size_str,
            mod_str,
            Message::RowClicked(side, row_idx),
            visual,
            active,
            config,
            colors,
            pad_y,
            name_color,
            dim_row,
            git_modified,
        ));
    }
    list = list.push(Space::with_height(bottom_space));

    // Show a placeholder only when we have nothing to display yet. Once the
    // first streamed chunk arrives we switch to the scrollable so the user
    // sees entries appear progressively even if more are still loading.
    let body: Element<'a, Message> = if pane.loading && pane.entries.is_empty() {
        container(text("Loading…").size(config.row_font_size))
            .padding(16)
            .width(Length::Fill)
            .into()
    } else {
        scrollable(list)
            .id(scroll_id(side))
            .height(Length::Fill)
            .on_scroll(move |viewport| {
                Message::Scrolled(side, viewport.absolute_offset().y, viewport.bounds().height)
            })
            .into()
    };

    let mut inner_col = column![path_header, column_header, body].spacing(4);
    if !pane.filter.is_empty() {
        inner_col = inner_col.push(filter_bar(pane, config, active));
    }
    if let Some(info) = &pane.git_info {
        inner_col = inner_col.push(git_info_bar(info, config, active));
    }
    if pane.has_claude_marker() {
        inner_col = inner_col.push(claude_info_bar(pane, config, active));
    }
    let inner = container(inner_col).padding(6);

    // No border around the pane any more. The "which pane is active" cue is
    // carried by the row-level text dimming (`compute_row_style` + the name
    // widget closure both look at `pane_active` and dim when it's false).
    let pane_container = container(inner)
        .width(Length::FillPortion(1))
        .height(Length::Fill);

    mouse_area(pane_container)
        .on_press(Message::Activate(side))
        .into()
}

/// Orange info bar shown when the active directory has a `CLAUDE.md` and/or
/// `.claude/`. Mirrors `git_info_bar`'s shape, just with hardcoded amber
/// colors since iced's theme doesn't ship an "orange" palette.
fn claude_info_bar<'a>(pane: &Pane, config: &Config, active: bool) -> Element<'a, Message> {
    container(
        text(pane.claude_marker_label())
            .font(Font::MONOSPACE)
            .size(config.header_font_size),
    )
    .padding(Padding::from([2, 8]))
    .width(Length::Fill)
    .style(move |_theme: &Theme| {
        let bg = Color::from_rgb8(0x4a, 0x30, 0x18);
        let fg_full = Color::from_rgb8(0xff, 0xa0, 0x40);
        let fg = if active { fg_full } else { dim(fg_full) };
        container::Style {
            background: Some(bg.into()),
            text_color: Some(fg),
            ..Default::default()
        }
    })
    .into()
}

fn git_info_bar<'a>(info: &GitInfo, config: &Config, active: bool) -> Element<'a, Message> {
    let mut parts: Vec<String> = vec![format!("branch: {}", info.branch)];
    if info.uncommitted > 0 {
        let noun = if info.uncommitted == 1 {
            "change"
        } else {
            "changes"
        };
        parts.push(format!("{} uncommitted {}", info.uncommitted, noun));
    }
    if info.ahead > 0 || info.behind > 0 {
        parts.push(format!("↑{} ↓{} vs upstream", info.ahead, info.behind));
    } else {
        // Best-effort hint when there's no divergence — but also no upstream
        // configured, you'll see exactly "↑0 ↓0" suppressed (this line stays
        // empty). Skipping it keeps the bar quieter for "clean" repos.
    }
    let label = parts.join("   ·   ");

    container(
        text(label)
            .font(Font::MONOSPACE)
            .size(config.header_font_size),
    )
    .padding(Padding::from([2, 8]))
    .width(Length::Fill)
    .style(move |theme: &Theme| {
        let palette = theme.extended_palette();
        let fg = if active {
            palette.success.weak.text
        } else {
            dim(palette.success.weak.text)
        };
        container::Style {
            background: Some(palette.success.weak.color.into()),
            text_color: Some(fg),
            ..Default::default()
        }
    })
    .into()
}

fn filter_bar<'a>(pane: &Pane, config: &Config, active: bool) -> Element<'a, Message> {
    let label = format!(
        "filter: /{}/   ({} of {})",
        pane.filter,
        pane.visible_indices.len(),
        pane.entries.len()
    );
    container(
        text(label)
            .font(Font::MONOSPACE)
            .size(config.header_font_size),
    )
    .padding(Padding::from([4, 8]))
    .width(Length::Fill)
    .style(move |theme: &Theme| {
        let palette = theme.extended_palette();
        let fg = if active {
            palette.primary.weak.text
        } else {
            dim(palette.primary.weak.text)
        };
        container::Style {
            background: Some(palette.primary.weak.color.into()),
            text_color: Some(fg),
            ..Default::default()
        }
    })
    .into()
}

/// Folder color applies only when the row is *not* part of the active pane's
/// selection — otherwise the cursor/mark text color would clash with it.
/// Name-cell color override for a row: the folder color for directories, the
/// action color for files that have a matching `file_actions` entry, and
/// `None` (theme default) otherwise. Suppressed while the row is part of the
/// active pane's selection so the highlight stays unambiguous — same rule the
/// folder color always followed.
fn entry_name_color(
    is_dir: bool,
    has_action: bool,
    visual: RowVisual,
    pane_active: bool,
    colors: RowColors,
) -> Option<Color> {
    let in_selection = pane_active && matches!(visual, RowVisual::Cursor | RowVisual::Marked);
    if in_selection {
        None
    } else if is_dir {
        colors.folder
    } else if has_action {
        colors.action
    } else {
        None
    }
}

fn row_visual(pane: &Pane, row_index: usize) -> RowVisual {
    if pane.selected == row_index {
        RowVisual::Cursor
    } else if pane.is_marked(row_index) {
        RowVisual::Marked
    } else {
        RowVisual::None
    }
}

fn header_cell<'a>(
    side: Side,
    label: &'a str,
    column: SortBy,
    pane: &Pane,
    width: Length,
    align: Horizontal,
    config: &Config,
    active: bool,
) -> Element<'a, Message> {
    let arrow = if pane.sort_by == column {
        match pane.sort_dir {
            SortDir::Asc => " ↑",
            SortDir::Desc => " ↓",
        }
    } else {
        ""
    };
    let label_full = format!("{}{}", label, arrow);
    button(
        text(label_full)
            .size(config.header_font_size)
            .width(Length::Fill)
            .align_x(align),
    )
    .width(width)
    .padding(Padding::from([2, 4]))
    .style(move |theme: &Theme, status: button::Status| {
        // Inherit the standard text-button look, then mute the label color
        // when this header is in an inactive pane so it matches the greyed-out
        // body rows.
        let mut s = button::text(theme, status);
        if !active {
            s.text_color = dim(s.text_color);
        }
        s
    })
    .on_press(Message::ToggleSort(side, column))
    .into()
}

fn build_row<'a>(
    row_index: usize,
    name: String,
    size: String,
    modified: String,
    msg: Message,
    visual: RowVisual,
    pane_active: bool,
    config: &Config,
    colors: RowColors,
    pad_y: u16,
    name_color: Option<Color>,
    dim_row: bool,
    git_modified: bool,
) -> Element<'a, Message> {
    let font_size = config.row_font_size;
    // Fixed-width gutter so every row's name column aligns regardless of
    // whether a git marker is present.
    let git_marker_width = Length::Fixed(font_size as f32);
    let git_marker = text(if git_modified { "●" } else { "" })
        .font(Font::MONOSPACE)
        .size(font_size)
        .width(git_marker_width)
        .style(|_theme: &Theme| iced::widget::text::Style {
            // GitHub-ish "modified" amber, picked to stand out on both light
            // and dark themes without further tuning.
            color: Some(Color::from_rgb8(0xd2, 0x99, 0x22)),
        });
    let name_widget = text(name)
        .font(Font::MONOSPACE)
        .size(font_size)
        .width(Length::Fill)
        .wrapping(iced::widget::text::Wrapping::None)
        .style(move |theme: &Theme| {
            // Resolve the final name color:
            //   - explicit override (e.g. folder_color) wins over theme default
            //   - dim_row pulls whichever color we'd use toward the background
            let resolved = match (name_color, dim_row) {
                (Some(c), true) => Some(dim(c)),
                (Some(c), false) => Some(c),
                (None, true) => Some(dim(theme.extended_palette().background.base.text)),
                (None, false) => None,
            };
            // Inactive pane → mute the name too so the row reads as part of
            // the greyed-out pane.
            let resolved = if !pane_active {
                Some(dim(
                    resolved.unwrap_or_else(|| theme.extended_palette().background.base.text),
                ))
            } else {
                resolved
            };
            iced::widget::text::Style { color: resolved }
        });

    // Keep marker and name visually adjacent — the outer row's 8px spacing
    // would otherwise push the name away from the marker.
    let name_cell = row![git_marker, name_widget].spacing(4).width(Length::Fill);

    let content = row![
        name_cell,
        text(size)
            .font(Font::MONOSPACE)
            .size(font_size)
            .width(Length::Fixed(config.size_column_px))
            .align_x(Horizontal::Right)
            .wrapping(iced::widget::text::Wrapping::None),
        text(modified)
            .font(Font::MONOSPACE)
            .size(font_size)
            .width(Length::Fixed(config.modified_column_px))
            .wrapping(iced::widget::text::Wrapping::None),
    ]
    .spacing(8);

    button(content)
        .on_press(msg)
        .width(Length::Fill)
        .height(Length::Fixed(config.row_height_px))
        .padding(Padding::from([pad_y, 8]))
        .clip(true)
        .style(move |theme: &Theme, status: button::Status| {
            compute_row_style(
                theme,
                status,
                visual,
                pane_active,
                row_index,
                colors,
                dim_row,
            )
        })
        .into()
}

fn compute_row_style(
    theme: &Theme,
    _status: button::Status,
    visual: RowVisual,
    pane_active: bool,
    row_index: usize,
    colors: RowColors,
    dim_row: bool,
) -> button::Style {
    let palette = theme.extended_palette();
    // dim_row affects the row's default text color (i.e. the Size and Modified
    // columns). The name widget handles its own dimming above so it can
    // combine with the folder-color override.
    let mut text_default = if dim_row {
        dim(palette.background.base.text)
    } else {
        palette.background.base.text
    };
    // Inactive pane → mute the row text so the whole pane visually recedes.
    // Cursor/Marked branches below only run when pane_active is true, so they
    // don't need to apply this dim themselves.
    if !pane_active {
        text_default = dim(text_default);
    }

    // Note: hover state intentionally ignored — rows shouldn't light up under
    // the cursor; the only emphasis is the active pane's selection.
    let (background, text_color) = match (visual, pane_active) {
        (RowVisual::Cursor, true) => {
            let pair = palette.primary.strong;
            let bg = colors.cursor.unwrap_or(pair.color);
            let fg = if colors.cursor.is_some() {
                Color::WHITE
            } else {
                pair.text
            };
            (Some(bg.into()), fg)
        }
        (RowVisual::Marked, true) => {
            let pair = palette.primary.weak;
            let bg = colors.mark.unwrap_or(pair.color);
            let fg = if colors.mark.is_some() {
                Color::WHITE
            } else {
                pair.text
            };
            (Some(bg.into()), fg)
        }
        _ => {
            if row_index % 2 == 1 {
                let stripe = colors.stripe.unwrap_or_else(|| {
                    blend(
                        palette.background.base.color,
                        palette.background.weak.color,
                        0.15,
                    )
                });
                (Some(stripe.into()), text_default)
            } else {
                (None, text_default)
            }
        }
    };

    button::Style {
        background,
        text_color,
        border: Border::default(),
        shadow: Shadow::default(),
    }
}

pub fn scroll_id(side: Side) -> scrollable::Id {
    match side {
        Side::Left => scrollable::Id::new("scroll-left"),
        Side::Right => scrollable::Id::new("scroll-right"),
    }
}

pub fn name_max_chars(window_width: f32, config: &Config) -> usize {
    let outer_padding = 8.0 * 2.0;
    let gap_between_panes = 8.0;
    let pane_width = (window_width - outer_padding - gap_between_panes) / 2.0;

    let pane_border_padding = 6.0 * 2.0 + 2.0;
    let row_horizontal_padding = 8.0 * 2.0;
    let row_gaps = 8.0 * 2.0;
    let scrollbar = 16.0;
    // The name column also contains a git-modified marker (width = font_size)
    // plus 4px spacing before the name text itself.
    let git_marker_with_gap = config.row_font_size as f32 + 4.0;
    // A couple of glyphs of breathing room: cosmic-text's measured glyph width
    // is rarely exactly mono_glyph_px, and we'd rather ellipsize one char too
    // early than have the name overflow into a second visual line.
    let safety_px = config.mono_glyph_px * 2.0;

    let name_px = pane_width
        - pane_border_padding
        - row_horizontal_padding
        - row_gaps
        - git_marker_with_gap
        - safety_px
        - config.size_column_px
        - config.modified_column_px
        - scrollbar;

    ((name_px / config.mono_glyph_px).max(8.0)) as usize
}

pub fn viewport_height_estimate(window_height: f32) -> f32 {
    (window_height - 110.0).max(100.0)
}

fn truncate_with_ellipsis(s: &str, max_chars: usize) -> String {
    let len = s.chars().count();
    if len <= max_chars {
        s.to_string()
    } else if max_chars == 0 {
        String::new()
    } else {
        let mut out: String = s.chars().take(max_chars - 1).collect();
        out.push('…');
        out
    }
}

pub fn format_size(bytes: u64) -> String {
    const KB: u64 = 1024;
    const MB: u64 = 1024 * 1024;
    const GB: u64 = 1024 * 1024 * 1024;
    if bytes < KB {
        format!("{} B", bytes)
    } else if bytes < MB {
        format!("{} KB", bytes / KB)
    } else if bytes < GB {
        format!("{:.1} MB", bytes as f64 / MB as f64)
    } else {
        format!("{:.1} GB", bytes as f64 / GB as f64)
    }
}

fn format_modified(t: SystemTime) -> String {
    let dt: DateTime<Local> = t.into();
    dt.format("%-m/%-d/%y %-I:%M %p").to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn approx_eq(a: f32, b: f32) -> bool {
        (a - b).abs() < 1e-3
    }

    fn test_colors() -> RowColors {
        RowColors {
            cursor: Some(Color::from_rgb(0.0, 0.0, 1.0)),
            mark: None,
            stripe: None,
            folder: Some(Color::from_rgb(0.1, 0.2, 0.3)),
            action: Some(Color::from_rgb(0.7, 0.4, 0.7)),
        }
    }

    #[test]
    fn entry_name_color_files_with_action_use_action_color() {
        let c = test_colors();
        // A plain file with a matching action gets the action color…
        assert_eq!(
            entry_name_color(false, true, RowVisual::None, true, c),
            c.action
        );
        // …a file without one gets the theme default (None)…
        assert_eq!(entry_name_color(false, false, RowVisual::None, true, c), None);
        // …and a directory always wins with the folder color.
        assert_eq!(
            entry_name_color(true, true, RowVisual::None, true, c),
            c.folder
        );
    }

    #[test]
    fn entry_name_color_suppressed_in_active_selection() {
        let c = test_colors();
        // Cursor / marked rows in the active pane drop the override so the
        // selection highlight reads cleanly — for both folders and actions.
        assert_eq!(entry_name_color(false, true, RowVisual::Cursor, true, c), None);
        assert_eq!(entry_name_color(true, false, RowVisual::Marked, true, c), None);
        // In the *inactive* pane the override stays (no selection there).
        assert_eq!(
            entry_name_color(false, true, RowVisual::Cursor, false, c),
            c.action
        );
    }

    #[test]
    fn format_size_zero() {
        assert_eq!(format_size(0), "0 B");
    }

    #[test]
    fn format_size_under_kb() {
        assert_eq!(format_size(1023), "1023 B");
    }

    #[test]
    fn format_size_exact_kb() {
        assert_eq!(format_size(1024), "1 KB");
    }

    #[test]
    fn format_size_kb_range() {
        assert_eq!(format_size(2048), "2 KB");
    }

    #[test]
    fn format_size_exact_mb() {
        assert_eq!(format_size(1024 * 1024), "1.0 MB");
    }

    #[test]
    fn format_size_mb_fractional() {
        // 1.5 MB → "1.5 MB"
        assert_eq!(format_size(1024 * 1024 + 512 * 1024), "1.5 MB");
    }

    #[test]
    fn format_size_exact_gb() {
        assert_eq!(format_size(1024u64.pow(3)), "1.0 GB");
    }

    #[test]
    fn truncate_shorter_than_max_returns_input() {
        assert_eq!(truncate_with_ellipsis("hi", 10), "hi");
    }

    #[test]
    fn truncate_exactly_at_max_returns_input() {
        assert_eq!(truncate_with_ellipsis("hello", 5), "hello");
    }

    #[test]
    fn truncate_over_max_adds_ellipsis() {
        // max_chars=5 → 4 chars + '…' = 5 chars total.
        assert_eq!(truncate_with_ellipsis("hello world", 5), "hell…");
    }

    #[test]
    fn truncate_max_zero_returns_empty() {
        assert_eq!(truncate_with_ellipsis("anything", 0), "");
    }

    #[test]
    fn truncate_unicode_safe() {
        // The 'é' is a single char but multiple bytes; truncation should work
        // on chars, not bytes.
        let s = "café-society";
        let out = truncate_with_ellipsis(s, 5);
        assert_eq!(out.chars().count(), 5);
    }

    #[test]
    fn name_max_chars_at_normal_width_is_reasonable() {
        let cfg = Config::default();
        let n = name_max_chars(1100.0, &cfg);
        // Sanity bounds: at 1100px window with the default columns the name
        // column should fit a few dozen glyphs.
        assert!(n >= 20, "expected at least 20 chars, got {}", n);
        assert!(n <= 100, "expected at most 100 chars, got {}", n);
    }

    #[test]
    fn name_max_chars_at_tiny_width_clamps_at_minimum() {
        let cfg = Config::default();
        // Even with a very narrow window, the helper clamps at 8 so we never
        // truncate filenames down to one or two glyphs.
        let n = name_max_chars(100.0, &cfg);
        assert!(n >= 8);
    }

    #[test]
    fn viewport_height_estimate_subtracts_chrome() {
        // At 700 window height, available rows ≈ 700 - 110 = 590.
        let h = viewport_height_estimate(700.0);
        assert!(approx_eq(h, 590.0));
    }

    #[test]
    fn viewport_height_estimate_clamps_at_minimum() {
        // At absurdly small heights it must still return at least 100 so the
        // scroll math doesn't divide by zero or produce negatives.
        let h = viewport_height_estimate(0.0);
        assert!(h >= 100.0);
    }
}
