//! ratatui rendering for every panel, dialog and status line.

use ratatui::layout::{Alignment, Constraint, Layout, Position, Rect};
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Clear, List, ListItem, ListState, Paragraph, Wrap};
use ratatui::Frame;

use crate::app::{
    prefix_label, same_path, App, Focus, Overlay, Row, SettingsForm, COMMITS_PER_PAGE,
    GROUP_PREVIEW,
};
use crate::diff_lines::DiffLineKind;
use crate::theme::{self, Theme};

const SPINNER: [&str; 4] = ["|", "/", "-", "\\"];

pub fn draw(f: &mut Frame, app: &mut App) {
    let theme = theme::theme_for(app.diff_settings.theme);
    let chunks = Layout::vertical([
        Constraint::Length(1),
        Constraint::Min(3),
        Constraint::Length(4),
        Constraint::Length(1),
    ])
    .split(f.area());

    draw_header(f, app, &theme, chunks[0]);

    let body = Layout::horizontal([Constraint::Length(app.sidebar_width), Constraint::Min(20)])
        .split(chunks[1]);
    draw_changes(f, app, &theme, body[0]);
    draw_diff(f, app, &theme, body[1]);

    draw_commit(f, app, &theme, chunks[2]);
    draw_status(f, app, &theme, chunks[3]);

    draw_overlay(f, app, &theme);
}

// ------------------------------------------------------------------- header

fn draw_header(f: &mut Frame, app: &App, theme: &Theme, area: Rect) {
    let repo = if app.repo_path.is_empty() {
        "no repository".to_string()
    } else {
        std::path::Path::new(&app.repo_path)
            .file_name()
            .map(|n| n.to_string_lossy().to_string())
            .unwrap_or_else(|| app.repo_path.clone())
    };

    let mut spans = vec![
        Span::styled(
            " EzConsoleGit ",
            Style::default()
                .bg(theme.accent)
                .fg(theme.text_on_accent)
                .add_modifier(Modifier::BOLD),
        ),
        Span::raw(" "),
        Span::styled(repo, Style::default().fg(theme.text)),
        Span::raw("  "),
        Span::styled(app.repo_path.clone(), Style::default().fg(theme.text_ghost)),
    ];

    if app.inspecting.is_some() {
        spans.push(Span::styled(
            "   COMMIT VIEW",
            Style::default()
                .fg(theme.warning)
                .add_modifier(Modifier::BOLD),
        ));
    }

    let right = format!(
        "depth {}  theme {}  {} ",
        app.diff_settings.tree_depth,
        theme_label(app),
        if app.show_all { "all shown" } else { "trees" },
    );

    f.render_widget(
        Paragraph::new(Line::from(spans)).style(Style::default().bg(theme.surface2)),
        area,
    );
    f.render_widget(
        Paragraph::new(Line::from(Span::styled(
            right,
            Style::default().fg(theme.text_faint),
        )))
        .alignment(Alignment::Right)
        .style(Style::default().bg(theme.surface2)),
        area,
    );
}

fn theme_label(app: &App) -> &'static str {
    match app.diff_settings.theme {
        ezgit_core::models::AppThemeMode::System => "system",
        ezgit_core::models::AppThemeMode::Dark => "dark",
        ezgit_core::models::AppThemeMode::Light => "light",
    }
}

// ------------------------------------------------------------------ changes

fn draw_changes(f: &mut Frame, app: &App, theme: &Theme, area: Rect) {
    let title = if let Some(commit) = &app.inspecting {
        format!(
            " Commit {} · {} ",
            commit.short_hash,
            truncate(&commit.message, (area.width as usize).saturating_sub(20))
        )
    } else {
        format!(" Changes ({}) ", app.unstaged.len() + app.staged.len())
    };

    let block = panel_block(theme, title, app.focus == Focus::Changes);
    let inner = block.inner(area);
    f.render_widget(block, area);

    if app.rows.is_empty() {
        return;
    }

    let items: Vec<ListItem> = app
        .rows
        .iter()
        .enumerate()
        .map(|(index, row)| row_item(row, index == app.row_sel, theme, inner.width))
        .collect();

    let mut state = ListState::default();
    state.select(Some(app.row_sel));
    f.render_stateful_widget(List::new(items), inner, &mut state);
}

fn row_item(row: &Row, selected: bool, theme: &Theme, width: u16) -> ListItem<'static> {
    let width = width as usize;

    match row {
        Row::Section { label, staged } => {
            let color = if *staged {
                theme.badge_added
            } else {
                theme.text_faint
            };
            let style = Style::default()
                .fg(color)
                .bg(theme.surface1)
                .add_modifier(Modifier::BOLD);
            ListItem::new(Line::from(Span::styled(format!(" {label}"), style)))
        }
        Row::Empty(message) => ListItem::new(Line::from(Span::styled(
            format!("   {message}"),
            Style::default().fg(theme.text_faint),
        ))),
        Row::Group {
            group,
            staged,
            expanded,
        } => {
            let mut spans = vec![
                Span::styled(
                    if *expanded { " v " } else { " > " },
                    Style::default().fg(theme.accent_text),
                ),
                Span::styled(
                    group.key.clone(),
                    Style::default()
                        .fg(theme.accent_text)
                        .add_modifier(Modifier::BOLD),
                ),
                Span::styled(
                    format!(" ({})", group.items.len()),
                    Style::default().fg(theme.text_faint),
                ),
            ];
            if group.has_serious_warning {
                spans.push(Span::styled(
                    "  !>800KB",
                    Style::default().fg(theme.warning),
                ));
            }
            if !*expanded && group.items.len() > GROUP_PREVIEW {
                spans.push(Span::styled(
                    format!("  +{} more", group.items.len() - GROUP_PREVIEW),
                    Style::default().fg(theme.text_ghost),
                ));
            }
            if *staged {
                spans.push(Span::styled(
                    "  [staged]",
                    Style::default().fg(theme.badge_added),
                ));
            }
            ListItem::new(Line::from(spans)).style(row_style(selected, theme))
        }
        Row::File { file, .. } => {
            let badge = file.display_status.clone();
            let mut spans = vec![
                Span::styled(
                    format!(" {badge:<2} "),
                    Style::default()
                        .fg(if selected {
                            theme.text_on_accent
                        } else {
                            badge_color(&badge, theme)
                        })
                        .add_modifier(Modifier::BOLD),
                ),
                Span::styled(
                    file.file_name.clone(),
                    Style::default().fg(if selected {
                        theme.text_on_accent
                    } else {
                        theme.text_secondary
                    }),
                ),
            ];
            if file.has_serious_warning {
                spans.push(Span::styled(" !", Style::default().fg(theme.warning)));
            }
            if !file.folder_path.is_empty() {
                let used = file.file_name.chars().count() + 6;
                let room = width.saturating_sub(used);
                if room > 4 {
                    spans.push(Span::styled(
                        format!("  {}", truncate(&file.folder_path, room)),
                        Style::default().fg(if selected {
                            theme.text_secondary
                        } else {
                            theme.text_ghost
                        }),
                    ));
                }
            }
            ListItem::new(Line::from(spans)).style(row_style(selected, theme))
        }
    }
}

fn row_style(selected: bool, theme: &Theme) -> Style {
    if selected {
        Style::default().bg(theme.selection)
    } else {
        Style::default()
    }
}

fn badge_color(badge: &str, theme: &Theme) -> ratatui::style::Color {
    match badge.chars().next() {
        Some('?') => theme.badge_untracked,
        Some('A') => theme.badge_added,
        Some('D') => theme.badge_deleted,
        Some('U') => theme.badge_conflict,
        Some('M') => theme.badge_modified,
        _ => theme.accent_text,
    }
}

// --------------------------------------------------------------------- diff

fn draw_diff(f: &mut Frame, app: &mut App, theme: &Theme, area: Rect) {
    let (title, suffix) = diff_title(app);
    let block = panel_block(theme, title, app.focus == Focus::Diff);
    let inner = block.inner(area);
    f.render_widget(block, area);

    if inner.width == 0 || inner.height == 0 {
        return;
    }

    if let Some(image) = &app.image_diff {
        draw_image_info(f, image, theme, inner);
        return;
    }

    if app.loading_diff {
        f.render_widget(
            Paragraph::new("Loading diff…").style(Style::default().fg(theme.accent_text)),
            inner,
        );
        return;
    }

    let len = app.diff_lines.len();
    if len == 0 {
        f.render_widget(
            Paragraph::new(suffix).style(Style::default().fg(theme.text_faint)),
            inner,
        );
        return;
    }

    let height = inner.height as usize;
    app.diff_viewport = height;
    let max_start = len.saturating_sub(1);
    if app.diff_scroll > max_start {
        app.diff_scroll = max_start;
    }
    let start = app.diff_scroll;
    let end = (start + height).min(len);
    let number_width = digits(len);

    let mut lines: Vec<Line> = Vec::with_capacity(end - start);
    for index in start..end {
        let line = &app.diff_lines[index];
        let gutter = format!("{:>width$} ", index + 1, width = number_width);
        let text = slice_from(&line.text, app.diff_hscroll);
        lines.push(Line::from(vec![
            Span::styled(gutter, Style::default().fg(theme.text_ghost)),
            Span::styled(text, diff_style(line.kind, theme)),
        ]));
    }

    f.render_widget(Paragraph::new(lines), inner);
}

fn diff_title(app: &App) -> (String, String) {
    let position = if app.diff_lines.is_empty() {
        String::new()
    } else {
        format!("  [{}/{}]", app.diff_scroll + 1, app.diff_lines.len())
    };

    if let Some(commit) = &app.inspecting {
        let path = app
            .selection
            .as_ref()
            .filter(|selection| !selection.is_group)
            .map(|selection| selection.key.clone())
            .unwrap_or_else(|| format!("commit {}", commit.short_hash));
        return (
            format!(" {path}  (commit {}){position} ", commit.short_hash),
            "Select a file to view its diff in this commit.".to_string(),
        );
    }

    match &app.selection {
        Some(selection) if !selection.is_group && !selection.key.is_empty() => (
            format!(
                " {}  ({}){position} ",
                selection.key,
                if selection.staged {
                    "Staged"
                } else {
                    "Working Tree"
                }
            ),
            String::new(),
        ),
        _ if app.diff_settings.is_single_diff_view => (
            format!(" Unified Working Tree Diff{position} "),
            "(No changes)".to_string(),
        ),
        _ => (
            " No file selected ".to_string(),
            "(No file selected)".to_string(),
        ),
    }
}

fn draw_image_info(
    f: &mut Frame,
    image: &ezgit_core::models::ImageDiffData,
    theme: &Theme,
    area: Rect,
) {
    let size = |value: &Option<usize>| match value {
        Some(bytes) => ezgit_core::git_service::format_bytes(*bytes as u64),
        None => "—".to_string(),
    };

    let lines = vec![
        Line::from(Span::styled(
            format!("  Image: {}", image.file_name),
            Style::default().fg(theme.text).add_modifier(Modifier::BOLD),
        )),
        Line::from(""),
        Line::from(vec![
            Span::styled("  BEFORE (HEAD)   ", Style::default().fg(theme.text_muted)),
            Span::styled(
                if image.has_before {
                    size(&image.before_size)
                } else {
                    "(absent)".to_string()
                },
                Style::default().fg(theme.badge_deleted),
            ),
        ]),
        Line::from(vec![
            Span::styled("  AFTER  (work)   ", Style::default().fg(theme.text_muted)),
            Span::styled(
                if image.has_after {
                    size(&image.after_size)
                } else {
                    "(absent)".to_string()
                },
                Style::default().fg(theme.badge_added),
            ),
        ]),
        Line::from(""),
        Line::from(Span::styled(
            "  Image pixels cannot be rendered in a terminal.",
            Style::default().fg(theme.text_faint),
        )),
        Line::from(Span::styled(
            "  Press 'e' to open the working-tree file in your OS viewer.",
            Style::default().fg(theme.text_faint),
        )),
        if image.is_svg {
            Line::from(Span::styled(
                "  SVG diff",
                Style::default().fg(theme.text_ghost),
            ))
        } else {
            Line::from("")
        },
    ];

    f.render_widget(Paragraph::new(lines).wrap(Wrap { trim: false }), area);
}

fn diff_style(kind: DiffLineKind, theme: &Theme) -> Style {
    match kind {
        DiffLineKind::Added => Style::default().bg(theme.diff_add_bg).fg(theme.diff_add_fg),
        DiffLineKind::Removed => Style::default().bg(theme.diff_del_bg).fg(theme.diff_del_fg),
        DiffLineKind::Hunk => Style::default()
            .bg(theme.diff_hunk_bg)
            .fg(theme.diff_hunk_fg),
        DiffLineKind::Meta => Style::default().fg(theme.diff_meta_fg),
        DiffLineKind::Normal => Style::default().fg(theme.text_dim),
    }
}

// ------------------------------------------------------------------- commit

fn draw_commit(f: &mut Frame, app: &mut App, theme: &Theme, area: Rect) {
    let title = format!(" Commit  ·  prefix: {} ", prefix_label(&app.ai_settings));
    let block = panel_block(theme, title, app.focus == Focus::Commit);
    let inner = block.inner(area);
    f.render_widget(block, area);

    if inner.width == 0 || inner.height == 0 {
        return;
    }

    let input_area = Rect { height: 1, ..inner };
    let hint_area = Rect {
        y: inner.y + 1,
        height: inner.height.saturating_sub(1),
        ..inner
    };

    let value = app.commit_message.value().to_string();
    let cursor = app.commit_message.char_cursor();
    let width = inner.width as usize;
    let chars: Vec<char> = value.chars().collect();

    let start = if cursor >= width {
        cursor + 1 - width
    } else {
        0
    };
    let visible: String = chars.iter().skip(start).collect();
    let selected = app.focus == Focus::Commit;

    if value.is_empty() {
        f.render_widget(
            Paragraph::new(Line::from(Span::styled(
                " Commit message…",
                Style::default().fg(theme.text_ghost),
            ))),
            input_area,
        );
    } else {
        f.render_widget(
            Paragraph::new(Line::from(Span::styled(
                format!(" {visible}"),
                Style::default().fg(if selected {
                    theme.text
                } else {
                    theme.text_secondary
                }),
            ))),
            input_area,
        );
    }

    if selected {
        let x = input_area.x + 1 + cursor.saturating_sub(start) as u16;
        let x = x.min(input_area.x + input_area.width.saturating_sub(1));
        f.set_cursor_position(Position::new(x, input_area.y));
    }

    let hint = Line::from(vec![
        Span::styled(" I", Style::default().fg(theme.accent_text)),
        Span::styled(" AI Commit & Push", Style::default().fg(theme.text_faint)),
        Span::styled("   Enter", Style::default().fg(theme.accent_text)),
        Span::styled(" Commit & Push", Style::default().fg(theme.text_faint)),
        Span::styled("   D", Style::default().fg(theme.accent_text)),
        Span::styled(" Reset All", Style::default().fg(theme.text_faint)),
        Span::styled("   A", Style::default().fg(theme.accent_text)),
        Span::styled(" AI Commit Trees", Style::default().fg(theme.text_faint)),
        Span::styled("   Ctrl+G", Style::default().fg(theme.accent_text)),
        Span::styled(" focus input", Style::default().fg(theme.text_faint)),
    ]);
    f.render_widget(Paragraph::new(hint), hint_area);
}

// ------------------------------------------------------------------- status

fn draw_status(f: &mut Frame, app: &App, theme: &Theme, area: Rect) {
    let (glyph, color) = if app.status_error {
        ("x", theme.danger)
    } else if app.is_operating {
        (
            SPINNER[app.spinner as usize % SPINNER.len()],
            theme.accent_text,
        )
    } else {
        ("*", theme.badge_added)
    };

    let mut spans = vec![
        Span::styled(format!(" {glyph} "), Style::default().fg(color)),
        Span::styled(
            app.status.clone(),
            Style::default().fg(if app.status_error {
                theme.danger
            } else {
                theme.text_muted
            }),
        ),
    ];
    if !app.is_operating {
        spans.push(Span::styled(
            format!("   {}", app.context_hint()),
            Style::default().fg(theme.text_ghost),
        ));
    }

    f.render_widget(
        Paragraph::new(Line::from(spans)).style(Style::default().bg(theme.surface0)),
        area,
    );
    f.render_widget(
        Paragraph::new(Line::from(Span::styled(
            format!("{} ", app.repo_path),
            Style::default().fg(theme.text_ghost),
        )))
        .alignment(Alignment::Right)
        .style(Style::default().bg(theme.surface0)),
        area,
    );
}

// ----------------------------------------------------------------- overlays

fn draw_overlay(f: &mut Frame, app: &mut App, theme: &Theme) {
    let area = f.area();
    match &app.overlay {
        Overlay::None => {}
        Overlay::Help => draw_help(f, theme, centered(area, 78, 26)),
        Overlay::Repos(state) => {
            let rect = centered(area, 90, 22);
            let block = dialog_block(theme, " Repositories ");
            let inner = block.inner(rect);
            f.render_widget(Clear, rect);
            f.render_widget(block, rect);

            let rows = Layout::vertical([Constraint::Min(1), Constraint::Length(2)]).split(inner);
            let items: Vec<ListItem> = if app.recent_repos.is_empty() {
                vec![ListItem::new(Span::styled(
                    "  No repositories yet — press 'i' to type a path.",
                    Style::default().fg(theme.text_faint),
                ))]
            } else {
                app.recent_repos
                    .iter()
                    .enumerate()
                    .map(|(index, repo)| {
                        let is_current = same_path(repo, &app.repo_path);
                        let selected = index == state.sel && !state.typing;
                        let name = std::path::Path::new(repo)
                            .file_name()
                            .map(|n| n.to_string_lossy().to_string())
                            .unwrap_or_else(|| repo.clone());
                        let style = if selected {
                            Style::default().bg(theme.selection).fg(theme.text)
                        } else {
                            Style::default()
                        };
                        ListItem::new(Line::from(vec![
                            Span::styled(
                                if is_current { " * " } else { "   " },
                                Style::default().fg(theme.accent_text),
                            ),
                            Span::styled(
                                name,
                                Style::default().fg(theme.text).add_modifier(Modifier::BOLD),
                            ),
                            Span::styled(
                                format!("  {repo}"),
                                Style::default().fg(theme.text_ghost),
                            ),
                        ]))
                        .style(style)
                    })
                    .collect()
            };
            let mut list_state = ListState::default();
            if !state.typing && !app.recent_repos.is_empty() {
                list_state.select(Some(state.sel));
            }
            f.render_stateful_widget(List::new(items), rows[0], &mut list_state);

            let mut footer: Vec<Line> = Vec::new();
            if state.typing {
                footer.push(Line::from(vec![
                    Span::styled(" path> ", Style::default().fg(theme.accent_text)),
                    Span::styled(
                        state.input.value().to_string(),
                        Style::default().fg(theme.text),
                    ),
                ]));
            } else {
                footer.push(Line::from(Span::styled(
                    " Enter open · d remove · i type a path · Esc close",
                    Style::default().fg(theme.text_faint),
                )));
            }
            f.render_widget(Paragraph::new(footer), rows[1]);
        }
        Overlay::Commits(state) => {
            let rect = centered(area, 96, 20);
            let block = dialog_block(theme, " Recent Commits ");
            let inner = block.inner(rect);
            f.render_widget(Clear, rect);
            f.render_widget(block, rect);

            let pages = app.commit_page_count();
            let rows = Layout::vertical([
                Constraint::Length(2),
                Constraint::Min(1),
                Constraint::Length(1),
            ])
            .split(inner);

            f.render_widget(
                Paragraph::new(Line::from(Span::styled(
                    format!(
                        "  Page {} / {}   ({} commits)",
                        state.page + 1,
                        pages,
                        app.commits.len()
                    ),
                    Style::default().fg(theme.text_faint),
                ))),
                rows[0],
            );

            let start = state.page * COMMITS_PER_PAGE;
            let mut lines: Vec<Line> = Vec::new();
            for (offset, commit) in app
                .commits
                .iter()
                .skip(start)
                .take(COMMITS_PER_PAGE)
                .enumerate()
            {
                let selected = offset == state.sel;
                let marker = if selected { " > " } else { "   " };
                lines.push(Line::from(vec![
                    Span::styled(marker, Style::default().fg(theme.accent_text)),
                    Span::styled(
                        commit.short_hash.clone(),
                        Style::default()
                            .fg(theme.syntax_green)
                            .add_modifier(Modifier::BOLD),
                    ),
                    Span::styled(
                        format!("  {:<16}", truncate(&commit.date, 16)),
                        Style::default().fg(theme.text_faint),
                    ),
                    Span::styled(
                        format!("  {:<18}", truncate(&commit.author, 18)),
                        Style::default().fg(theme.text_ghost),
                    ),
                    Span::styled(
                        format!("  {}", commit.message_section),
                        Style::default().fg(theme.text),
                    ),
                ]));
                if !commit.tag_section.is_empty() {
                    lines.push(Line::from(vec![
                        Span::raw("      "),
                        Span::styled(
                            commit.tag_section.clone(),
                            Style::default().fg(theme.accent_text),
                        ),
                    ]));
                }
                lines.push(Line::from(""));
            }
            if app.commits.is_empty() {
                lines.push(Line::from(Span::styled(
                    "  No commits found.",
                    Style::default().fg(theme.text_faint),
                )));
            }
            f.render_widget(Paragraph::new(lines), rows[1]);

            f.render_widget(
                Paragraph::new(Span::styled(
                    " j/k move · h/l or ←/→ page · Enter inspect · Esc close",
                    Style::default().fg(theme.text_faint),
                )),
                rows[2],
            );
        }
        Overlay::Settings(form) => draw_settings(f, form, theme, centered(area, 88, 24)),
        Overlay::Confirm(action) => {
            let rect = centered(area, 68, 8);
            let block = dialog_block(theme, &format!(" {} ", action.title));
            let inner = block.inner(rect);
            f.render_widget(Clear, rect);
            f.render_widget(block, rect);

            let mut lines = vec![Line::from("")];
            for line in wrap(&action.message, inner.width.saturating_sub(4) as usize) {
                lines.push(Line::from(Span::styled(
                    format!("  {line}"),
                    Style::default().fg(theme.text_secondary),
                )));
            }
            lines.push(Line::from(""));
            lines.push(Line::from(Span::styled(
                "  Enter / y confirm        Esc / n cancel",
                Style::default().fg(theme.text_faint),
            )));
            f.render_widget(Paragraph::new(lines), inner);
        }
        Overlay::Prompt {
            title, hint, input, ..
        } => {
            let rect = centered(area, 76, 7);
            let block = dialog_block(theme, &format!(" {title} "));
            let inner = block.inner(rect);
            f.render_widget(Clear, rect);
            f.render_widget(block, rect);

            let rows =
                Layout::vertical([Constraint::Length(2), Constraint::Length(1)]).split(inner);
            f.render_widget(
                Paragraph::new(Line::from(vec![
                    Span::styled(" > ", Style::default().fg(theme.accent_text)),
                    Span::styled(input.value().to_string(), Style::default().fg(theme.text)),
                ])),
                rows[0],
            );
            f.render_widget(
                Paragraph::new(Span::styled(
                    format!(" {hint}   Esc cancel"),
                    Style::default().fg(theme.text_faint),
                )),
                rows[1],
            );

            let width = rows[0].width as usize;
            let cursor = input.char_cursor();
            let start = if cursor >= width.saturating_sub(3) {
                cursor + 4 - width
            } else {
                0
            };
            let x = rows[0].x + 3 + cursor.saturating_sub(start) as u16;
            f.set_cursor_position(Position::new(
                x.min(rows[0].x + rows[0].width.saturating_sub(1)),
                rows[0].y,
            ));
        }
    }
}

fn draw_settings(f: &mut Frame, form: &SettingsForm, theme: &Theme, rect: Rect) {
    let block = dialog_block(theme, " AI & Git Settings ");
    let inner = block.inner(rect);
    f.render_widget(Clear, rect);
    f.render_widget(block, rect);

    let rows = Layout::vertical([
        Constraint::Length(2),
        Constraint::Length(2),
        Constraint::Length(2),
        Constraint::Length(3),
        Constraint::Length(2),
        Constraint::Length(2),
        Constraint::Length(1),
        Constraint::Min(2),
    ])
    .split(inner);

    render_text_field(
        f,
        rows[0],
        theme,
        form.focus == 0,
        "API Endpoint URL",
        form.api_url.value(),
    );
    let masked = "*".repeat(form.api_key.value().chars().count().min(40));
    render_text_field(
        f,
        rows[1],
        theme,
        form.focus == 1,
        "API Key (Enter reveals)",
        if form.show_key {
            form.api_key.value()
        } else {
            &masked
        },
    );
    render_text_field(
        f,
        rows[2],
        theme,
        form.focus == 2,
        "Model Name",
        form.model.value(),
    );

    let prefix_focused = form.focus == 3;
    f.render_widget(
        Paragraph::new(vec![
            Line::from(Span::styled(
                " Git Commit Prefix Mode",
                Style::default().fg(if prefix_focused {
                    theme.accent_text
                } else {
                    theme.text_muted
                }),
            )),
            Line::from(Span::styled(
                format!(" < {} >", prefix_mode_label(form.prefix_mode)),
                Style::default()
                    .bg(if prefix_focused {
                        theme.selection
                    } else {
                        theme.surface1
                    })
                    .fg(theme.text),
            )),
            Line::from(Span::styled(
                format!(" preview: {}", form.preview()),
                Style::default().fg(theme.text_ghost),
            )),
        ]),
        rows[3],
    );

    render_text_field(
        f,
        rows[4],
        theme,
        form.focus == 4,
        "Custom Prefix (custom / both modes)",
        form.custom_prefix.value(),
    );
    render_text_field(
        f,
        rows[5],
        theme,
        form.focus == 5,
        "Repository Path",
        form.repo.value(),
    );

    let test_style = if form.focus == SettingsForm::TEST {
        Style::default().bg(theme.selection).fg(theme.accent_text)
    } else {
        Style::default().fg(theme.text_muted)
    };
    f.render_widget(
        Paragraph::new(Span::styled(" [ Test Connection ]", test_style)),
        rows[6],
    );

    let mut footer: Vec<Line> = Vec::new();
    if form.testing {
        footer.push(Line::from(Span::styled(
            " Testing connection…",
            Style::default().fg(theme.accent_text),
        )));
    } else if let Some((is_error, text)) = &form.test {
        footer.push(Line::from(Span::styled(
            format!(
                " {}",
                truncate(text, inner.width.saturating_sub(4) as usize)
            ),
            Style::default().fg(if *is_error {
                theme.danger
            } else {
                theme.badge_added
            }),
        )));
    } else {
        footer.push(Line::from(Span::styled(
            " Tab/↑↓ move · Enter reveal key / test / save · ←→ prefix mode",
            Style::default().fg(theme.text_faint),
        )));
    }
    footer.push(Line::from(vec![
        Span::styled(
            " [ Save ]",
            Style::default().fg(if form.focus == SettingsForm::SAVE {
                theme.accent_text
            } else {
                theme.text_muted
            }),
        ),
        Span::styled("   Esc cancel", Style::default().fg(theme.text_faint)),
    ]));
    f.render_widget(Paragraph::new(footer), rows[7]);
}

fn render_text_field(
    f: &mut Frame,
    area: Rect,
    theme: &Theme,
    focused: bool,
    label: &str,
    value: &str,
) {
    f.render_widget(
        Paragraph::new(vec![
            Line::from(Span::styled(
                format!(" {label}"),
                Style::default().fg(if focused {
                    theme.accent_text
                } else {
                    theme.text_muted
                }),
            )),
            Line::from(Span::styled(
                format!(" {value}"),
                if focused {
                    Style::default().bg(theme.selection).fg(theme.text)
                } else {
                    Style::default().fg(theme.text_secondary)
                },
            )),
        ]),
        area,
    );
}

fn prefix_mode_label(mode: ezgit_core::models::GitPrefixMode) -> &'static str {
    use ezgit_core::models::GitPrefixMode::*;
    match mode {
        AutoFolder => "Auto Folder Tag  [Folder\\Path]",
        Custom => "Custom Prefix Only",
        Both => "Both (Folder + Custom)",
        None => "None (No prefix)",
    }
}

fn draw_help(f: &mut Frame, theme: &Theme, rect: Rect) {
    let block = dialog_block(theme, " Keyboard Shortcuts ");
    let inner = block.inner(rect);
    f.render_widget(Clear, rect);
    f.render_widget(block, rect);

    let lines: Vec<Line> = HELP
        .iter()
        .map(|(keys, description)| {
            Line::from(vec![
                Span::styled(
                    format!("  {keys:<22}"),
                    Style::default().fg(theme.accent_text),
                ),
                Span::styled(*description, Style::default().fg(theme.text_secondary)),
            ])
        })
        .collect();

    f.render_widget(Paragraph::new(lines).wrap(Wrap { trim: false }), inner);
}

const HELP: &[(&str, &str)] = &[
    ("Tab / Shift+Tab", "cycle focus: changes -> diff -> commit"),
    ("1 / 2 / 3", "jump focus to changes / diff / commit"),
    ("?", "toggle this help"),
    ("q / Ctrl+C", "quit"),
    ("r / F5", "refresh repository status"),
    ("o", "repository manager (recent repos, type a path)"),
    ("H", "browse recent commits (Enter inspects)"),
    ("S", "AI & Git settings"),
    ("P / L / Y", "git push / pull / sync"),
    (
        "I (or Ctrl+I)",
        "AI commit & push (terminals send Ctrl+I as Tab)",
    ),
    ("Enter", "commit & push (while the commit box is focused)"),
    ("A", "AI commit every folder tree, one by one"),
    ("D", "reset ALL changes (working tree + untracked)"),
    ("v", "toggle unified single diff view"),
    ("x", "expand / collapse all trees"),
    ("- / +", "decrease / increase tree grouping depth"),
    ("[ / ] / \\", "narrow / widen / reset sidebar"),
    ("t", "cycle theme (system -> dark -> light)"),
    ("", ""),
    ("--- changes panel ---", ""),
    ("j / k", "move selection"),
    ("g / G", "first / last row"),
    ("Enter / l", "open file (diff) or expand group"),
    ("h", "collapse group"),
    ("Space / s", "stage / unstage the selected file or group"),
    ("u", "unstage selected file or group"),
    ("d", "discard changes (reset / delete untracked)"),
    ("i", "instant AI commit for the selected file"),
    ("a", "AI commit the selected folder tree"),
    ("c", "commit & push the selected file / group"),
    ("n", "add selected untracked path to .gitignore"),
    ("e / E", "open file / reveal in file manager"),
    ("y", "copy the file path to the clipboard"),
    ("", ""),
    ("--- diff panel ---", ""),
    ("j / k, PgUp/PgDn", "scroll the diff"),
    ("h / l", "pan horizontally, 0 resets"),
    ("g / G", "top / bottom of the diff"),
];

// ------------------------------------------------------------------- helpers

fn panel_block(theme: &Theme, title: String, focused: bool) -> Block<'static> {
    Block::default()
        .borders(Borders::ALL)
        .border_style(Style::default().fg(if focused { theme.accent } else { theme.border }))
        .style(Style::default().bg(theme.surface1))
        .title(Span::styled(
            title,
            Style::default()
                .fg(if focused {
                    theme.accent_text
                } else {
                    theme.text_muted
                })
                .add_modifier(Modifier::BOLD),
        ))
}

fn dialog_block(theme: &Theme, title: &str) -> Block<'static> {
    Block::default()
        .borders(Borders::ALL)
        .border_style(Style::default().fg(theme.border_strong))
        .style(Style::default().bg(theme.surface2))
        .title(Span::styled(
            title.to_string(),
            Style::default()
                .fg(theme.accent_text)
                .add_modifier(Modifier::BOLD),
        ))
}

fn centered(area: Rect, width: u16, height: u16) -> Rect {
    let width = width.min(area.width);
    let height = height.min(area.height);
    Rect {
        x: area.x + (area.width - width) / 2,
        y: area.y + (area.height - height) / 2,
        width,
        height,
    }
}

pub fn truncate(text: &str, max: usize) -> String {
    if max == 0 {
        return String::new();
    }
    if text.chars().count() <= max {
        return text.to_string();
    }
    let head: String = text.chars().take(max.saturating_sub(1)).collect();
    format!("{head}…")
}

fn digits(value: usize) -> usize {
    value.max(1).to_string().len()
}

/// Character-wise horizontal slice (approximate for double-width glyphs).
fn slice_from(text: &str, skip: usize) -> String {
    if skip == 0 {
        return text.to_string();
    }
    text.chars().skip(skip).collect()
}

fn wrap(text: &str, width: usize) -> Vec<String> {
    if width == 0 {
        return vec![text.to_string()];
    }
    let mut lines = Vec::new();
    let mut current = String::new();
    for word in text.split_whitespace() {
        if current.is_empty() {
            current.push_str(word);
        } else if current.chars().count() + 1 + word.chars().count() <= width {
            current.push(' ');
            current.push_str(word);
        } else {
            lines.push(current);
            current = word.to_string();
        }
    }
    if !current.is_empty() {
        lines.push(current);
    }
    lines
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn truncate_keeps_short_text_and_adds_ellipsis() {
        assert_eq!(truncate("abc", 5), "abc");
        assert_eq!(truncate("abcdef", 4), "abc…");
        assert_eq!(truncate("anything", 0), "");
    }

    #[test]
    fn digits_counts_decimal_places() {
        assert_eq!(digits(0), 1);
        assert_eq!(digits(9), 1);
        assert_eq!(digits(10), 2);
        assert_eq!(digits(1234), 4);
    }

    #[test]
    fn slice_from_skips_characters_safely() {
        assert_eq!(slice_from("hello", 0), "hello");
        assert_eq!(slice_from("hello", 2), "llo");
        assert_eq!(slice_from("hello", 99), "");
        // Multi-byte characters must not panic when panning.
        assert_eq!(slice_from("héllo", 2), "llo");
    }

    #[test]
    fn wrap_breaks_on_word_boundaries() {
        assert_eq!(wrap("one two three", 7), vec!["one two", "three"]);
        assert_eq!(wrap("single", 20), vec!["single"]);
    }
}
