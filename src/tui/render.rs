//! Drawing. Reads `App` and writes a frame; never mutates state.

use ratatui::layout::{Constraint, Direction, Layout, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Clear, List, ListItem, ListState, Paragraph, Tabs, Wrap};
use ratatui::Frame;

use crate::tui::app::{App, Draft, Field, Level, Mode, Tab};

const ACCENT: Color = Color::Cyan;
const MUTED: Color = Color::DarkGray;

/// See the matching definition in `app.rs`: works around a strictrs
/// checker bug where a literal `-> ()` is misread as an absent return type.
type Unit = ();

/// Narrow a `usize` for ratatui's u16 geometry without an `as` cast.
fn as_u16(value: usize) -> u16 {
    u16::try_from(value).unwrap_or(u16::MAX)
}

pub fn draw(frame: &mut Frame, app: &App) -> Unit {
    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(3), // tabs
            Constraint::Min(3),    // body
            Constraint::Length(1), // status
        ])
        .split(frame.area());

    let Some(header_area) = chunks.first().copied() else {
        return;
    };
    let Some(body_area) = chunks.get(1).copied() else {
        return;
    };
    let Some(status_area) = chunks.get(2).copied() else {
        return;
    };

    draw_tabs(frame, app, header_area);

    match app.mode {
        Mode::Add => draw_form(frame, app, body_area),
        Mode::Browse | Mode::Filter | Mode::Help => draw_browser(frame, app, body_area),
    }

    draw_status(frame, app, status_area);

    if matches!(app.mode, Mode::Help) {
        draw_help(frame, body_area);
    }
}

fn draw_tabs(frame: &mut Frame, app: &App, area: Rect) {
    let titles: Vec<Line> = Tab::ALL
        .iter()
        .map(|tab| {
            let count = match tab {
                Tab::Stories => app.stories.len(),
                Tab::Interviews => app.interviews.len(),
                Tab::Media => app.media.len(),
            };
            Line::from(format!(" {} {count} ", tab.title()))
        })
        .collect();

    let subject = app
        .vault
        .load_config()
        .map(|config| config.subject_name)
        .unwrap_or_else(|_unreadable| "archive".to_owned());

    let tabs = Tabs::new(titles)
        .block(
            Block::default()
                .borders(Borders::ALL)
                .title(format!(" legacy — {subject} ")),
        )
        .select(app.tab.index())
        .highlight_style(Style::default().fg(ACCENT).add_modifier(Modifier::BOLD));
    frame.render_widget(tabs, area);
}

fn draw_browser(frame: &mut Frame, app: &App, area: Rect) {
    let columns = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([Constraint::Percentage(40), Constraint::Percentage(60)])
        .split(area);

    let Some(list_area) = columns.first().copied() else {
        return;
    };
    let Some(detail_area) = columns.get(1).copied() else {
        return;
    };

    draw_list(frame, app, list_area);
    draw_detail(frame, app, detail_area);
}

fn list_label(app: &App, index: usize) -> Line<'static> {
    match app.tab {
        Tab::Stories => match app.stories.get(index) {
            Some(story) => Line::from(vec![
                Span::styled(
                    format!(
                        "{:<11}",
                        story.date.clone().unwrap_or_else(|| "undated".to_owned())
                    ),
                    Style::default().fg(MUTED),
                ),
                Span::raw(story.title.clone()),
            ]),
            None => Line::from(""),
        },
        Tab::Interviews => match app.interviews.get(index) {
            Some(row) => Line::from(vec![
                Span::styled(format!("{:<10}", row.status), Style::default().fg(MUTED)),
                Span::raw(row.session_id.clone()),
            ]),
            None => Line::from(""),
        },
        Tab::Media => match app.media.get(index) {
            Some(row) => Line::from(row.relative_path.clone()),
            None => Line::from(""),
        },
    }
}

fn draw_list(frame: &mut Frame, app: &App, area: Rect) {
    let items: Vec<ListItem> = app
        .visible()
        .iter()
        .map(|index| ListItem::new(list_label(app, *index)))
        .collect();

    let title = if app.filter.is_empty() {
        format!(" {} ", app.tab.title())
    } else {
        format!(" {} — filter: {} ", app.tab.title(), app.filter)
    };

    let empty = items.is_empty();
    let list = List::new(items)
        .block(Block::default().borders(Borders::ALL).title(title))
        .highlight_style(
            Style::default()
                .bg(ACCENT)
                .fg(Color::Black)
                .add_modifier(Modifier::BOLD),
        )
        .highlight_symbol("▌");

    let mut state = ListState::default();
    if !empty {
        state.select(Some(app.selected_position()));
    }
    frame.render_stateful_widget(list, area, &mut state);
}

fn field_line(label: &str, value: &str) -> Line<'static> {
    Line::from(vec![
        Span::styled(format!("{label:<12}"), Style::default().fg(MUTED)),
        Span::raw(value.to_owned()),
    ])
}

fn draw_detail(frame: &mut Frame, app: &App, area: Rect) {
    let mut lines: Vec<Line> = Vec::new();

    if let Some(story) = app.selected_story() {
        lines.push(Line::from(Span::styled(
            story.title.clone(),
            Style::default().add_modifier(Modifier::BOLD),
        )));
        lines.push(Line::from(""));
        lines.push(field_line("id", &story.id));
        lines.push(field_line(
            "date",
            &format!(
                "{} ({})",
                story.date.clone().unwrap_or_else(|| "unknown".to_owned()),
                story.date_precision.as_str()
            ),
        ));
        lines.push(field_line("visibility", story.visibility.as_str()));
        if !story.people.is_empty() {
            lines.push(field_line("people", &story.people.join(", ")));
        }
        if !story.places.is_empty() {
            lines.push(field_line("places", &story.places.join(", ")));
        }
        if !story.tags.is_empty() {
            lines.push(field_line("tags", &story.tags.join(", ")));
        }
        if !story.media.is_empty() {
            lines.push(field_line("media", &story.media.join(", ")));
        }
        if let Some(source) = &story.source {
            lines.push(field_line("source", source));
        }
        // Machine-written tags are always visible as such, here as much as
        // in the file itself.
        match &story.tags_generated_by {
            Some(model) => lines.push(Line::from(Span::styled(
                format!("{:<12}{model}", "tags by"),
                Style::default().fg(Color::Yellow),
            ))),
            None => lines.push(field_line("tags by", "a human")),
        }
        lines.push(Line::from(""));
        for line in story.body.trim().lines() {
            lines.push(Line::from(line.to_owned()));
        }
    } else if let Some(row) = app.selected_interview() {
        lines.push(Line::from(Span::styled(
            row.session_id.clone(),
            Style::default().add_modifier(Modifier::BOLD),
        )));
        lines.push(Line::from(""));
        lines.push(field_line("bank", &row.bank));
        lines.push(field_line("status", &row.status));
        lines.push(field_line("mode", &row.mode));
        lines.push(field_line("answered", &row.answered.to_string()));
        lines.push(field_line("skipped", &row.skipped.to_string()));
        lines.push(Line::from(""));
        for line in row.transcript.trim().lines() {
            lines.push(Line::from(line.to_owned()));
        }
    } else if let Some(row) = app.selected_media() {
        lines.push(Line::from(Span::styled(
            row.relative_path.clone(),
            Style::default().add_modifier(Modifier::BOLD),
        )));
        lines.push(Line::from(""));
        match &row.sidecar {
            Some(sidecar) => {
                lines.push(field_line("original", &sidecar.original_filename));
                lines.push(field_line(
                    "date",
                    &sidecar.date.clone().unwrap_or_else(|| "unknown".to_owned()),
                ));
                lines.push(field_line("from", sidecar.date_source.as_str()));
                lines.push(field_line(
                    "type",
                    sidecar.mime_type.as_deref().unwrap_or("unknown"),
                ));
                lines.push(field_line("bytes", &sidecar.size_bytes.to_string()));
                lines.push(field_line("visibility", sidecar.visibility.as_str()));
                lines.push(field_line("sha256", &sidecar.sha256));
                if let Some(caption) = &sidecar.caption {
                    lines.push(field_line("caption", caption));
                }
            }
            None => lines.push(Line::from(Span::styled(
                "no sidecar found for this file",
                Style::default().fg(Color::Yellow),
            ))),
        }
    } else {
        lines.push(Line::from(Span::styled(
            "nothing selected",
            Style::default().fg(MUTED),
        )));
    }

    let detail = Paragraph::new(lines)
        .block(Block::default().borders(Borders::ALL).title(" Detail "))
        .wrap(Wrap { trim: false });
    frame.render_widget(detail, area);
}

/// One row of the form, highlighted when focused.
fn form_row(draft: &Draft, field: Field, focused: bool) -> Line<'static> {
    let value = draft.value(field).to_owned();
    let shown = if focused {
        format!("{value}▏")
    } else {
        value
    };
    let label_style = if focused {
        Style::default().fg(ACCENT).add_modifier(Modifier::BOLD)
    } else {
        Style::default().fg(MUTED)
    };
    Line::from(vec![
        Span::styled(format!("{:<12}", field.label()), label_style),
        Span::raw(shown),
        Span::styled(
            format!("   {}", field.hint()),
            Style::default().fg(MUTED).add_modifier(Modifier::ITALIC),
        ),
    ])
}

fn draw_form(frame: &mut Frame, app: &App, area: Rect) {
    let single_line_fields = [
        Field::Title,
        Field::Date,
        Field::People,
        Field::Places,
        Field::Tags,
        Field::Visibility,
    ];

    let rows = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(as_u16(single_line_fields.len()).saturating_add(2)),
            Constraint::Min(3),
        ])
        .split(area);

    let Some(fields_area) = rows.first().copied() else {
        return;
    };
    let Some(body_area) = rows.get(1).copied() else {
        return;
    };

    let lines: Vec<Line> = single_line_fields
        .iter()
        .map(|field| form_row(&app.draft, *field, app.draft.field == *field))
        .collect();
    let fields = Paragraph::new(lines).block(
        Block::default()
            .borders(Borders::ALL)
            .title(" New story — ctrl-s saves, esc cancels "),
    );
    frame.render_widget(fields, fields_area);

    let focused = app.draft.field == Field::Body;
    let mut body_lines: Vec<Line> = app
        .draft
        .body
        .lines()
        .map(|line| Line::from(line.to_owned()))
        .collect();
    if focused {
        // Show the caret on its own line when the body ends in a newline.
        match body_lines.last_mut() {
            Some(last) if !app.draft.body.ends_with('\n') => last.spans.push(Span::raw("▏")),
            Some(_) | None => body_lines.push(Line::from("▏")),
        }
    }
    if !app.draft.media.is_empty() {
        body_lines.push(Line::from(""));
        body_lines.push(Line::from(Span::styled(
            format!("attached audio: {}", app.draft.media.join(", ")),
            Style::default().fg(Color::Green),
        )));
    }

    let title = if app.is_recording() {
        " Story — ● RECORDING, press ctrl-r to stop ".to_owned()
    } else {
        format!(" {} — {} ", Field::Body.label(), Field::Body.hint())
    };
    let border_style = if app.is_recording() {
        Style::default().fg(Color::Red).add_modifier(Modifier::BOLD)
    } else if focused {
        Style::default().fg(ACCENT)
    } else {
        Style::default()
    };

    let body = Paragraph::new(body_lines)
        .block(
            Block::default()
                .borders(Borders::ALL)
                .border_style(border_style)
                .title(title),
        )
        .wrap(Wrap { trim: false });
    frame.render_widget(body, body_area);
}

fn draw_status(frame: &mut Frame, app: &App, area: Rect) {
    let colour = match app.status.level {
        Level::Info => MUTED,
        Level::Success => Color::Green,
        Level::Warning => Color::Yellow,
        Level::Error => Color::Red,
    };

    let keys = match app.mode {
        Mode::Browse => "j/k move  tab switch  / filter  a add  r reload  ? help  q quit",
        Mode::Filter => "type to filter  enter accept  esc clear",
        Mode::Add => "tab/shift-tab field  ctrl-r record  ctrl-s save  esc cancel",
        Mode::Help => "any key closes help",
    };

    let line = Line::from(vec![
        Span::styled(
            format!(" {} ", app.status.message),
            Style::default().fg(colour),
        ),
        Span::styled(format!("│ {keys}"), Style::default().fg(MUTED)),
    ]);
    frame.render_widget(Paragraph::new(line), area);
}

/// A centred rectangle, sized as a percentage of `area`.
fn centred(percent_x: u16, percent_y: u16, area: Rect) -> Rect {
    let vertical = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Percentage((100_u16.saturating_sub(percent_y)) / 2),
            Constraint::Percentage(percent_y),
            Constraint::Percentage((100_u16.saturating_sub(percent_y)) / 2),
        ])
        .split(area);
    let Some(middle) = vertical.get(1).copied() else {
        return area;
    };
    let horizontal = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([
            Constraint::Percentage((100_u16.saturating_sub(percent_x)) / 2),
            Constraint::Percentage(percent_x),
            Constraint::Percentage((100_u16.saturating_sub(percent_x)) / 2),
        ])
        .split(middle);
    horizontal.get(1).copied().unwrap_or(area)
}

const HELP_LINES: [(&str, &str); 14] = [
    ("j / down", "next row"),
    ("k / up", "previous row"),
    ("g / G", "first / last row"),
    ("pgup / pgdn", "move a screenful"),
    ("tab / shift-tab", "next / previous collection"),
    ("/", "filter the current collection"),
    ("esc", "clear the filter"),
    ("a", "add a story"),
    ("ctrl-r / F2", "record audio (while adding)"),
    ("ctrl-s", "save the story being added"),
    ("r", "reload from disk"),
    ("?", "this help"),
    ("q", "quit"),
    ("", ""),
];

fn draw_help(frame: &mut Frame, area: Rect) {
    let popup = centred(60, 70, area);
    frame.render_widget(Clear, popup);

    let mut lines: Vec<Line> = HELP_LINES
        .iter()
        .map(|(key, description)| {
            Line::from(vec![
                Span::styled(format!("  {key:<16}"), Style::default().fg(ACCENT)),
                Span::raw((*description).to_owned()),
            ])
        })
        .collect();
    lines.push(Line::from(Span::styled(
        "  Recordings are saved into media/ before they are transcribed,",
        Style::default().fg(MUTED),
    )));
    lines.push(Line::from(Span::styled(
        "  so audio survives even when transcription is unavailable.",
        Style::default().fg(MUTED),
    )));

    let help = Paragraph::new(lines).block(
        Block::default()
            .borders(Borders::ALL)
            .border_style(Style::default().fg(ACCENT))
            .title(" Keys "),
    );
    frame.render_widget(help, popup);
}

#[cfg(test)]
mod tests {
    use super::{as_u16, centred, draw};
    use crate::core::story::{new_story, save_story, NewStory};
    use crate::core::vault::Vault;
    use crate::tui::app::{App, Field, Mode};
    use ratatui::backend::TestBackend;
    use ratatui::layout::Rect;
    use ratatui::Terminal;

    fn app_with_a_story() -> (tempfile::TempDir, App) {
        let temp = tempfile::TempDir::new().expect("temp dir");
        let vault = Vault::init(&temp.path().join("arch"), "Jane Doe", 2, 3).expect("init");
        let story = new_story(NewStory {
            title: "Starting university".to_owned(),
            date: Some("1994-09-15".to_owned()),
            body: "I packed one suitcase.".to_owned(),
            tags: vec!["education".to_owned()],
            ..NewStory::default()
        })
        .expect("story");
        save_story(&vault.root, &story, false).expect("save");
        let app = App::new(vault).expect("app");
        (temp, app)
    }

    /// Render once and return the screen as text, so assertions can be
    /// made about what a person would actually see.
    fn screen(app: &App, width: u16, height: u16) -> String {
        let backend = TestBackend::new(width, height);
        let mut terminal = Terminal::new(backend).expect("terminal");
        terminal
            .draw(|frame| draw(frame, app))
            .expect("draw succeeds");
        let buffer = terminal.backend().buffer().clone();
        buffer
            .content()
            .chunks(usize::from(width))
            .map(|row| row.iter().map(|cell| cell.symbol()).collect::<String>())
            .collect::<Vec<String>>()
            .join("\n")
    }

    #[test]
    fn narrowing_a_usize_saturates_instead_of_wrapping() {
        assert_eq!(as_u16(10), 10);
        assert_eq!(as_u16(usize::MAX), u16::MAX);
    }

    #[test]
    fn browse_shows_the_subject_tabs_and_the_selected_story() {
        let (_temp, app) = app_with_a_story();
        let text = screen(&app, 100, 30);
        assert!(text.contains("Jane Doe"), "the archive names its subject");
        assert!(text.contains("Stories"));
        assert!(text.contains("Interviews"));
        assert!(text.contains("Media"));
        assert!(text.contains("Starting university"));
        assert!(
            text.contains("I packed one suitcase."),
            "detail pane shows the body"
        );
        assert!(text.contains("education"));
    }

    #[test]
    fn the_detail_pane_says_who_wrote_the_tags() {
        let (_temp, app) = app_with_a_story();
        let text = screen(&app, 100, 30);
        assert!(
            text.contains("a human"),
            "provenance of tags is visible without opening the file"
        );
    }

    #[test]
    fn an_empty_archive_renders_without_panicking() {
        let temp = tempfile::TempDir::new().expect("temp dir");
        let vault = Vault::init(&temp.path().join("arch"), "Nobody", 2, 3).expect("init");
        let app = App::new(vault).expect("app");
        let text = screen(&app, 80, 24);
        assert!(text.contains("nothing selected"));
    }

    #[test]
    fn the_form_renders_its_fields_and_hints() {
        let (_temp, mut app) = app_with_a_story();
        app.begin_add();
        let text = screen(&app, 100, 30);
        assert!(text.contains("New story"));
        assert!(text.contains("Title"));
        assert!(text.contains("Visibility"));
        assert!(
            text.contains("YYYY-MM-DD"),
            "date formats are shown while typing"
        );
        assert!(text.contains("ctrl-s"));
    }

    #[test]
    fn the_help_overlay_lists_the_keys() {
        let (_temp, mut app) = app_with_a_story();
        app.toggle_help();
        assert_eq!(app.mode, Mode::Help);
        let text = screen(&app, 100, 30);
        assert!(text.contains("Keys"));
        assert!(text.contains("quit"));
        assert!(text.contains("record audio"));
    }

    #[test]
    fn the_filter_appears_in_the_list_title() {
        let (_temp, mut app) = app_with_a_story();
        app.begin_filter();
        for character in "suit".chars() {
            app.push_filter(character);
        }
        let text = screen(&app, 100, 30);
        assert!(text.contains("filter: suit"));
    }

    #[test]
    fn attached_audio_is_shown_in_the_form() {
        let (_temp, mut app) = app_with_a_story();
        app.begin_add();
        app.draft
            .media
            .push("media/undated/take-001.wav".to_owned());
        app.draft.field = Field::Body;
        let text = screen(&app, 100, 30);
        assert!(text.contains("attached audio"));
        assert!(text.contains("take-001.wav"));
    }

    #[test]
    fn a_very_small_terminal_still_renders() {
        // Panicking on a small window would take the whole session down
        // mid-interview, so the layout must simply cope.
        let (_temp, app) = app_with_a_story();
        for (width, height) in [(20_u16, 6_u16), (10, 4), (40, 10)] {
            let text = screen(&app, width, height);
            assert!(!text.is_empty());
        }
    }

    #[test]
    fn centred_rectangles_stay_inside_their_parent() {
        let area = Rect::new(0, 0, 100, 40);
        let popup = centred(60, 70, area);
        assert!(popup.width <= area.width);
        assert!(popup.height <= area.height);
        assert!(popup.x + popup.width <= area.x + area.width);
    }
}
