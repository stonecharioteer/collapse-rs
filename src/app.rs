use std::{
    io,
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
        mpsc::{self, Receiver},
    },
    thread,
    time::{Duration, Instant},
};

use ansi_to_tui::IntoText as _;
use crossterm::event::{
    self, Event as TerminalEvent, KeyCode, KeyEvent, KeyEventKind, KeyModifiers,
};
use ratatui::{
    DefaultTerminal, Frame,
    layout::{Alignment, Constraint, Layout, Rect},
    style::{Color, Modifier, Style, Stylize},
    text::{Line, Span, Text},
    widgets::{Block, BorderType, Borders, Clear, Gauge, Padding, Paragraph, Tabs},
};

use crate::runner::{self, ColorMode, CommandResult, Event};

const CYAN: Color = Color::Rgb(0, 229, 255);
const MAGENTA: Color = Color::Rgb(255, 73, 219);
const GREEN: Color = Color::Rgb(73, 232, 151);
const RED: Color = Color::Rgb(255, 91, 112);
const MUTED: Color = Color::Rgb(113, 121, 140);
const PANEL: Color = Color::Rgb(35, 39, 51);

pub fn run(
    terminal: &mut DefaultTerminal,
    command_lines: Vec<String>,
    color: ColorMode,
) -> io::Result<u8> {
    let Some(command_lines) = crate::editor::review(terminal, command_lines)? else {
        return Ok(0);
    };
    let mut app = App::new(command_lines.clone());
    let (sender, receiver) = mpsc::channel();
    let cancel = Arc::new(AtomicBool::new(false));
    let worker_cancel = Arc::clone(&cancel);
    let worker = thread::spawn(move || runner::run(command_lines, color, sender, worker_cancel));
    let mut interrupted = false;

    let ui_result = (|| -> io::Result<()> {
        loop {
            app.receive(&receiver);
            terminal.draw(|frame| app.render(frame))?;

            if event::poll(Duration::from_millis(50))? {
                let should_quit = match event::read()? {
                    TerminalEvent::Key(key)
                        if matches!(key.kind, KeyEventKind::Press | KeyEventKind::Repeat) =>
                    {
                        app.handle_key(key)
                    }
                    TerminalEvent::Paste(text) => {
                        app.handle_paste(&text);
                        false
                    }
                    _ => false,
                };
                if should_quit {
                    interrupted = !app.tasks.iter().all(|task| task.status.is_complete());
                    break;
                }
            }
        }
        Ok(())
    })();

    if interrupted || ui_result.is_err() {
        cancel.store(true, Ordering::Relaxed);
    }
    let _ = worker.join();
    app.receive(&receiver);
    ui_result?;

    if interrupted || app.cancelled {
        Ok(130)
    } else if app.tasks.iter().any(|task| task.status.is_failure()) {
        Ok(1)
    } else {
        Ok(0)
    }
}

struct App {
    tasks: Vec<Task>,
    selected: usize,
    selection_pinned: bool,
    all_finished: bool,
    cancelled: bool,
    started_at: Instant,
    tick: usize,
    viewport_height: usize,
    search_query: String,
    search_editing: bool,
    current_match: Option<SearchMatch>,
    quit_confirmation: bool,
}

impl App {
    fn new(commands: Vec<String>) -> Self {
        Self {
            tasks: commands.into_iter().map(Task::new).collect(),
            selected: 0,
            selection_pinned: false,
            all_finished: false,
            cancelled: false,
            started_at: Instant::now(),
            tick: 0,
            viewport_height: 1,
            search_query: String::new(),
            search_editing: false,
            current_match: None,
            quit_confirmation: false,
        }
    }

    fn receive(&mut self, receiver: &Receiver<Event>) {
        for event in receiver.try_iter() {
            match event {
                Event::Started { index } => {
                    if let Some(task) = self.tasks.get_mut(index) {
                        task.status = TaskStatus::Running;
                        task.started_at = Some(Instant::now());
                    }
                    if !self.selection_pinned {
                        if self.selected != index {
                            self.current_match = None;
                        }
                        self.selected = index;
                    }
                }
                Event::Output { index, bytes } => {
                    if let Some(task) = self.tasks.get_mut(index) {
                        task.raw_output.extend(bytes);
                        task.output_dirty = true;
                    }
                }
                Event::Finished {
                    index,
                    result,
                    elapsed,
                } => {
                    if let Some(task) = self.tasks.get_mut(index) {
                        task.elapsed = Some(elapsed);
                        task.started_at = None;
                        task.status = match result {
                            CommandResult::Exited(status) if status.success() => TaskStatus::Passed,
                            CommandResult::Exited(status) => TaskStatus::Failed(status.code()),
                            CommandResult::Cancelled => TaskStatus::Cancelled,
                            CommandResult::SpawnFailed(error) => {
                                task.raw_output.extend_from_slice(
                                    format!("\n\x1b[31mcollapse: {error}\x1b[0m\n").as_bytes(),
                                );
                                task.output_dirty = true;
                                TaskStatus::SpawnFailed
                            }
                        };
                    }
                }
                Event::AllFinished { cancelled } => {
                    self.all_finished = true;
                    self.cancelled = cancelled;
                }
            }
        }
        self.tick = self.tick.wrapping_add(1);
    }

    fn handle_key(&mut self, key: KeyEvent) -> bool {
        if self.quit_confirmation {
            return match key.code {
                KeyCode::Char('y' | 'Y') => true,
                KeyCode::Char('n' | 'N') | KeyCode::Esc | KeyCode::Enter => {
                    self.quit_confirmation = false;
                    false
                }
                _ => false,
            };
        }

        if self.search_editing {
            self.handle_search_key(key);
            return false;
        }

        match key.code {
            KeyCode::Char('/') => {
                self.search_query.clear();
                self.current_match = None;
                self.search_editing = true;
            }
            KeyCode::Char('n') => self.jump_to_match(SearchDirection::Next),
            KeyCode::Char('N') => self.jump_to_match(SearchDirection::Previous),
            KeyCode::Esc if !self.search_query.is_empty() => {
                self.search_query.clear();
                self.current_match = None;
            }
            KeyCode::Char('q') | KeyCode::Esc => self.quit_confirmation = true,
            KeyCode::Tab | KeyCode::Right | KeyCode::Char('l') => self.select_next(),
            KeyCode::BackTab | KeyCode::Left | KeyCode::Char('h') => self.select_previous(),
            KeyCode::Up | KeyCode::Char('k') => self.scroll_up(1),
            KeyCode::Down | KeyCode::Char('j') => self.scroll_down(1),
            KeyCode::PageUp => self.scroll_up(self.viewport_height.saturating_sub(1).max(1)),
            KeyCode::PageDown => self.scroll_down(self.viewport_height.saturating_sub(1).max(1)),
            KeyCode::Home | KeyCode::Char('g') => {
                let task = &mut self.tasks[self.selected];
                task.scroll = 0;
                task.follow = false;
            }
            KeyCode::End | KeyCode::Char('G') => {
                let height = self.viewport_height;
                let task = &mut self.tasks[self.selected];
                task.scroll = task.line_count().saturating_sub(height);
                task.follow = true;
            }
            _ => {}
        }
        false
    }

    fn handle_paste(&mut self, text: &str) {
        if self.search_editing {
            self.search_query
                .push_str(text.trim_end_matches(['\r', '\n']));
            self.select_initial_match();
        }
    }

    fn handle_search_key(&mut self, key: KeyEvent) {
        match key.code {
            KeyCode::Enter => self.search_editing = false,
            KeyCode::Esc => {
                self.search_editing = false;
                self.search_query.clear();
                self.current_match = None;
            }
            KeyCode::Backspace => {
                self.search_query.pop();
                self.select_initial_match();
            }
            KeyCode::Char('u') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                self.search_query.clear();
                self.current_match = None;
            }
            KeyCode::Char(character)
                if !key
                    .modifiers
                    .intersects(KeyModifiers::CONTROL | KeyModifiers::ALT) =>
            {
                self.search_query.push(character);
                self.select_initial_match();
            }
            _ => {}
        }
    }

    fn select_initial_match(&mut self) {
        self.tasks[self.selected].refresh_output();
        let matches = find_matches(
            &self.tasks[self.selected].rendered.lines,
            &self.search_query,
        );
        let scroll = self.tasks[self.selected].scroll;
        self.current_match = matches
            .iter()
            .find(|found| found.line >= scroll)
            .copied()
            .or_else(|| matches.first().copied());
        self.reveal_current_match();
    }

    fn jump_to_match(&mut self, direction: SearchDirection) {
        if self.search_query.is_empty() {
            return;
        }
        self.tasks[self.selected].refresh_output();
        let matches = find_matches(
            &self.tasks[self.selected].rendered.lines,
            &self.search_query,
        );
        if matches.is_empty() {
            self.current_match = None;
            return;
        }

        let current_index = self
            .current_match
            .and_then(|current| matches.iter().position(|found| *found == current));
        let next_index = match (direction, current_index) {
            (SearchDirection::Next, Some(index)) => (index + 1) % matches.len(),
            (SearchDirection::Previous, Some(0) | None) => matches.len() - 1,
            (SearchDirection::Previous, Some(index)) => index - 1,
            (SearchDirection::Next, None) => 0,
        };
        self.current_match = Some(matches[next_index]);
        self.reveal_current_match();
    }

    fn reveal_current_match(&mut self) {
        let Some(found) = self.current_match else {
            return;
        };
        let task = &mut self.tasks[self.selected];
        task.scroll = found.line.saturating_sub(self.viewport_height / 2);
        task.follow = false;
    }

    fn select_next(&mut self) {
        self.selected = (self.selected + 1) % self.tasks.len();
        self.selection_pinned = true;
        self.current_match = None;
    }

    fn select_previous(&mut self) {
        self.selected = self.selected.checked_sub(1).unwrap_or(self.tasks.len() - 1);
        self.selection_pinned = true;
        self.current_match = None;
    }

    fn scroll_up(&mut self, amount: usize) {
        let task = &mut self.tasks[self.selected];
        task.scroll = task.scroll.saturating_sub(amount);
        task.follow = false;
    }

    fn scroll_down(&mut self, amount: usize) {
        let height = self.viewport_height;
        let task = &mut self.tasks[self.selected];
        let max = task.line_count().saturating_sub(height);
        task.scroll = (task.scroll + amount).min(max);
        task.follow = task.scroll == max;
    }

    fn render(&mut self, frame: &mut Frame) {
        self.tasks[self.selected].refresh_output();

        let areas = Layout::vertical([
            Constraint::Length(2),
            Constraint::Length(3),
            Constraint::Min(4),
            Constraint::Length(1),
        ])
        .split(frame.area());

        self.render_header(frame, areas[0]);
        self.render_tabs(frame, areas[1]);
        self.render_output(frame, areas[2]);
        self.render_footer(frame, areas[3]);
        if self.quit_confirmation {
            self.render_quit_confirmation(frame);
        }
    }

    fn render_header(&self, frame: &mut Frame, area: Rect) {
        let complete = self
            .tasks
            .iter()
            .filter(|task| task.status.is_complete())
            .count();
        let passed = self
            .tasks
            .iter()
            .filter(|task| matches!(task.status, TaskStatus::Passed))
            .count();
        let failed = self
            .tasks
            .iter()
            .filter(|task| task.status.is_failure())
            .count();
        let spinner = ["◆", "◇", "◈", "◇"][self.tick % 4];
        let state = if self.all_finished && failed == 0 {
            Span::styled("DONE", Style::new().fg(GREEN).bold())
        } else if self.all_finished {
            Span::styled("FAILED", Style::new().fg(RED).bold())
        } else {
            Span::styled(format!("{spinner} RUNNING"), Style::new().fg(CYAN).bold())
        };
        let elapsed = format_duration(self.started_at.elapsed());
        let summary = if self.all_finished {
            format!("  {passed} passed  {failed} failed  {elapsed}")
        } else {
            format!("  {complete}/{} tasks  {elapsed}", self.tasks.len())
        };

        frame.render_widget(
            Paragraph::new(Line::from(vec![
                Span::styled(
                    " COLLAPSE ",
                    Style::new().fg(Color::Black).bg(MAGENTA).bold(),
                ),
                Span::raw("  "),
                state,
                Span::styled(summary, Style::new().fg(MUTED)),
            ])),
            Rect::new(area.x, area.y, area.width, 1),
        );

        let ratio = complete as f64 / self.tasks.len() as f64;
        frame.render_widget(
            Gauge::default()
                .ratio(ratio)
                .gauge_style(Style::new().fg(CYAN).bg(PANEL))
                .label(""),
            Rect::new(area.x, area.y + 1, area.width, 1),
        );
    }

    fn render_tabs(&self, frame: &mut Frame, area: Rect) {
        let titles: Vec<Line<'_>> = self
            .tasks
            .iter()
            .enumerate()
            .map(|(index, task)| {
                let marker = task.status.marker(self.tick);
                Line::from(format!(" {marker} {} ", short_name(&task.command, index)))
            })
            .collect();

        let tabs = Tabs::new(titles)
            .block(
                Block::default()
                    .borders(Borders::BOTTOM)
                    .border_style(Style::new().fg(PANEL)),
            )
            .select(self.selected)
            .style(Style::new().fg(MUTED))
            .highlight_style(
                Style::new()
                    .fg(Color::Black)
                    .bg(CYAN)
                    .add_modifier(Modifier::BOLD),
            )
            .divider(Span::styled(" │ ", Style::new().fg(PANEL)))
            .padding("", "");
        frame.render_widget(tabs, area);
    }

    fn render_output(&mut self, frame: &mut Frame, area: Rect) {
        let task_count = self.tasks.len();
        let search_query = self.search_query.clone();
        let current_match = self.current_match;
        let search_count =
            find_matches(&self.tasks[self.selected].rendered.lines, &search_query).len();
        let task = &mut self.tasks[self.selected];
        let inner_height = area.height.saturating_sub(2) as usize;
        self.viewport_height = inner_height.max(1);
        let max_scroll = task.line_count().saturating_sub(self.viewport_height);
        if task.follow {
            task.scroll = max_scroll;
        } else {
            task.scroll = task.scroll.min(max_scroll);
        }

        let status = task.status.label(task.elapsed_or_running());
        let title = Line::from(vec![
            Span::styled(
                format!(" TASK {}/{} ", self.selected + 1, task_count),
                Style::new().fg(Color::Black).bg(MAGENTA).bold(),
            ),
            Span::styled(
                format!(" {} ", task.command),
                Style::new().fg(Color::White).bold(),
            ),
            Span::styled(format!(" · {status} "), task.status.style()),
            if search_query.is_empty() {
                Span::raw("")
            } else {
                Span::styled(
                    format!(" · {search_count} matches "),
                    Style::new().fg(MAGENTA).bold(),
                )
            },
        ]);

        let block = Block::default()
            .title(title)
            .borders(Borders::ALL)
            .border_type(BorderType::Rounded)
            .border_style(Style::new().fg(if task.status.is_failure() { RED } else { PANEL }))
            .padding(Padding::horizontal(1));

        let content = if task.rendered.lines.is_empty() {
            let message = match task.status {
                TaskStatus::Queued => "Waiting for earlier tasks…",
                TaskStatus::Running => "Running… no output yet",
                _ => "Command produced no output",
            };
            vec![Line::styled(message, Style::new().fg(MUTED).italic())]
        } else {
            let end = (task.scroll + self.viewport_height).min(task.rendered.lines.len());
            task.rendered.lines[task.scroll..end]
                .iter()
                .enumerate()
                .map(|(offset, line)| {
                    highlight_matches(line, task.scroll + offset, &search_query, current_match)
                })
                .collect()
        };

        frame.render_widget(Paragraph::new(Text::from(content)).block(block), area);
    }

    fn render_footer(&self, frame: &mut Frame, area: Rect) {
        if self.search_editing {
            let matches = find_matches(
                &self.tasks[self.selected].rendered.lines,
                &self.search_query,
            );
            let position = self
                .current_match
                .and_then(|current| matches.iter().position(|found| *found == current))
                .map(|index| index + 1)
                .unwrap_or(0);
            frame.render_widget(
                Paragraph::new(Line::from(vec![
                    " / ".black().on_magenta().bold(),
                    Span::styled(&self.search_query, Style::new().fg(Color::White).bold()),
                    Span::styled("█", Style::new().fg(CYAN)),
                    Span::styled(
                        format!("  {position}/{}  Enter accept · Esc cancel", matches.len()),
                        Style::new().fg(MUTED),
                    ),
                ])),
                area,
            );
            return;
        }

        let follow = if self.tasks[self.selected].follow {
            "following"
        } else {
            "paused"
        };
        frame.render_widget(
            Paragraph::new(Line::from(vec![
                " Tab ".black().on_cyan().bold(),
                " next  ".fg(MUTED),
                "/ ".fg(MAGENTA).bold(),
                " search  ".fg(MUTED),
                "n/N ".fg(MAGENTA).bold(),
                " match  ".fg(MUTED),
                "↑↓/jk ".fg(CYAN).bold(),
                " scroll  ".fg(MUTED),
                "q ".fg(RED).bold(),
                " quit  ".fg(MUTED),
                Span::styled(format!("· {follow}"), Style::new().fg(MUTED)),
            ])),
            area,
        );
    }

    fn render_quit_confirmation(&self, frame: &mut Frame) {
        let screen = frame.area();
        let width = screen.width.min(62);
        let height = screen.height.min(5);
        let area = Rect::new(
            screen.x + screen.width.saturating_sub(width) / 2,
            screen.y + screen.height.saturating_sub(height) / 2,
            width,
            height,
        );
        let work_remaining = self.tasks.iter().any(|task| !task.status.is_complete());
        let question = if work_remaining {
            "Cancel the remaining commands and quit?"
        } else {
            "Close collapse?"
        };

        frame.render_widget(Clear, area);
        frame.render_widget(
            Paragraph::new(Text::from(vec![
                Line::styled(question, Style::new().fg(Color::White).bold()),
                Line::from(vec![
                    Span::styled("y", Style::new().fg(GREEN).bold()),
                    Span::styled(" yes   ", Style::new().fg(MUTED)),
                    Span::styled("n / Esc / Enter", Style::new().fg(RED).bold()),
                    Span::styled(" keep running", Style::new().fg(MUTED)),
                ]),
            ]))
            .alignment(Alignment::Center)
            .style(Style::new().bg(PANEL))
            .block(
                Block::default()
                    .title(" CONFIRM ")
                    .title_style(Style::new().fg(MAGENTA).bold())
                    .borders(Borders::ALL)
                    .border_type(BorderType::Rounded)
                    .border_style(Style::new().fg(RED))
                    .style(Style::new().bg(PANEL)),
            ),
            area,
        );
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct SearchMatch {
    line: usize,
    start: usize,
    end: usize,
}

#[derive(Clone, Copy)]
enum SearchDirection {
    Next,
    Previous,
}

fn find_matches(lines: &[Line<'_>], query: &str) -> Vec<SearchMatch> {
    if query.is_empty() {
        return Vec::new();
    }

    lines
        .iter()
        .enumerate()
        .flat_map(|(line_index, line)| {
            let plain = line
                .spans
                .iter()
                .map(|span| span.content.as_ref())
                .collect::<String>();
            plain
                .match_indices(query)
                .map(move |(start, found)| SearchMatch {
                    line: line_index,
                    start,
                    end: start + found.len(),
                })
                .collect::<Vec<_>>()
        })
        .collect()
}

fn highlight_matches(
    line: &Line<'static>,
    line_index: usize,
    query: &str,
    current: Option<SearchMatch>,
) -> Line<'static> {
    if query.is_empty() {
        return line.clone();
    }

    let plain = line
        .spans
        .iter()
        .map(|span| span.content.as_ref())
        .collect::<String>();
    let ranges: Vec<SearchMatch> = plain
        .match_indices(query)
        .map(|(start, found)| SearchMatch {
            line: line_index,
            start,
            end: start + found.len(),
        })
        .collect();
    if ranges.is_empty() {
        return line.clone();
    }

    let mut output_spans = Vec::new();
    let mut span_start = 0;
    for span in &line.spans {
        let span_end = span_start + span.content.len();
        let mut boundaries = vec![0, span.content.len()];
        for found in &ranges {
            let overlap_start = found.start.max(span_start);
            let overlap_end = found.end.min(span_end);
            if overlap_start < overlap_end {
                boundaries.push(overlap_start - span_start);
                boundaries.push(overlap_end - span_start);
            }
        }
        boundaries.sort_unstable();
        boundaries.dedup();

        for window in boundaries.windows(2) {
            let local_start = window[0];
            let local_end = window[1];
            if local_start == local_end {
                continue;
            }
            let global_start = span_start + local_start;
            let matched = ranges
                .iter()
                .find(|found| found.start <= global_start && global_start < found.end);
            let style = if let Some(found) = matched {
                let color = if Some(*found) == current {
                    MAGENTA
                } else {
                    Color::Rgb(245, 200, 66)
                };
                span.style
                    .patch(Style::new().bg(color).add_modifier(Modifier::BOLD))
            } else {
                span.style
            };
            output_spans.push(Span::styled(
                span.content[local_start..local_end].to_owned(),
                style,
            ));
        }
        span_start = span_end;
    }

    let mut highlighted = line.clone();
    highlighted.spans = output_spans;
    highlighted
}

struct Task {
    command: String,
    status: TaskStatus,
    started_at: Option<Instant>,
    elapsed: Option<Duration>,
    raw_output: Vec<u8>,
    rendered: Text<'static>,
    output_dirty: bool,
    scroll: usize,
    follow: bool,
}

impl Task {
    fn new(command: String) -> Self {
        Self {
            command,
            status: TaskStatus::Queued,
            started_at: None,
            elapsed: None,
            raw_output: Vec::new(),
            rendered: Text::default(),
            output_dirty: false,
            scroll: 0,
            follow: true,
        }
    }

    fn refresh_output(&mut self) {
        if !self.output_dirty {
            return;
        }
        self.rendered = self
            .raw_output
            .into_text()
            .unwrap_or_else(|_| Text::raw(String::from_utf8_lossy(&self.raw_output).into_owned()));
        self.output_dirty = false;
    }

    fn line_count(&self) -> usize {
        self.rendered.lines.len()
    }

    fn elapsed_or_running(&self) -> Option<Duration> {
        self.elapsed
            .or_else(|| self.started_at.map(|start| start.elapsed()))
    }
}

#[derive(Clone, Copy)]
enum TaskStatus {
    Queued,
    Running,
    Passed,
    Failed(Option<i32>),
    SpawnFailed,
    Cancelled,
}

impl TaskStatus {
    fn is_complete(self) -> bool {
        !matches!(self, Self::Queued | Self::Running)
    }

    fn is_failure(self) -> bool {
        matches!(self, Self::Failed(_) | Self::SpawnFailed)
    }

    fn marker(self, tick: usize) -> &'static str {
        match self {
            Self::Queued => "○",
            Self::Running => ["◐", "◓", "◑", "◒"][tick % 4],
            Self::Passed => "✓",
            Self::Failed(_) | Self::SpawnFailed => "✕",
            Self::Cancelled => "■",
        }
    }

    fn style(self) -> Style {
        match self {
            Self::Queued => Style::new().fg(MUTED),
            Self::Running => Style::new().fg(CYAN).bold(),
            Self::Passed => Style::new().fg(GREEN).bold(),
            Self::Failed(_) | Self::SpawnFailed => Style::new().fg(RED).bold(),
            Self::Cancelled => Style::new().fg(MAGENTA).bold(),
        }
    }

    fn label(self, elapsed: Option<Duration>) -> String {
        let duration = elapsed.map(format_duration).unwrap_or_default();
        match self {
            Self::Queued => "QUEUED".into(),
            Self::Running => format!("RUNNING {duration}"),
            Self::Passed => format!("PASSED {duration}"),
            Self::Failed(Some(code)) => format!("FAILED ({code}) {duration}"),
            Self::Failed(None) => format!("FAILED (signal) {duration}"),
            Self::SpawnFailed => format!("SPAWN FAILED {duration}"),
            Self::Cancelled => format!("CANCELLED {duration}"),
        }
    }
}

fn short_name(command: &str, index: usize) -> String {
    const MAX_CHARS: usize = 22;
    let trimmed = command.trim();
    if trimmed.is_empty() {
        return format!("task {}", index + 1);
    }
    let mut chars = trimmed.chars();
    let short: String = chars.by_ref().take(MAX_CHARS).collect();
    if chars.next().is_some() {
        format!("{short}…")
    } else {
        short
    }
}

fn format_duration(duration: Duration) -> String {
    if duration.as_secs() >= 60 {
        format!(
            "{}m {:02}s",
            duration.as_secs() / 60,
            duration.as_secs() % 60
        )
    } else if duration.as_secs() > 0 {
        format!("{:.1}s", duration.as_secs_f64())
    } else {
        format!("{}ms", duration.as_millis())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use ratatui::style::Color;

    #[test]
    fn ansi_output_keeps_truecolor() {
        let mut task = Task::new("colors".into());
        task.raw_output = b"\x1b[38;2;12;34;56mhello\x1b[0m\n".to_vec();
        task.output_dirty = true;
        task.refresh_output();

        assert_eq!(task.rendered.lines[0].spans[0].content, "hello");
        assert_eq!(
            task.rendered.lines[0].spans[0].style.fg,
            Some(Color::Rgb(12, 34, 56))
        );
    }

    #[test]
    fn search_matches_across_ansi_style_boundaries() {
        let line = Line::from(vec![
            Span::styled("er", Style::new().fg(Color::Red)),
            Span::styled("ror", Style::new().fg(Color::Blue)),
        ]);
        let matches = find_matches(std::slice::from_ref(&line), "error");
        assert_eq!(matches.len(), 1);

        let highlighted = highlight_matches(&line, 0, "error", None);
        assert_eq!(highlighted.spans[0].style.fg, Some(Color::Red));
        assert_eq!(highlighted.spans[1].style.fg, Some(Color::Blue));
        assert!(
            highlighted
                .spans
                .iter()
                .all(|span| span.style.bg == Some(Color::Rgb(245, 200, 66)))
        );
    }

    #[test]
    fn next_match_wraps_around() {
        let mut app = App::new(vec!["search".into()]);
        app.tasks[0].rendered = Text::from("needle\nother\nneedle");
        app.search_query = "needle".into();
        app.select_initial_match();
        assert_eq!(app.current_match.unwrap().line, 0);

        app.jump_to_match(SearchDirection::Next);
        assert_eq!(app.current_match.unwrap().line, 2);
        app.jump_to_match(SearchDirection::Next);
        assert_eq!(app.current_match.unwrap().line, 0);
    }

    #[test]
    fn quitting_requires_confirmation() {
        let mut app = App::new(vec!["work".into()]);
        assert!(!app.handle_key(KeyEvent::new(KeyCode::Char('q'), KeyModifiers::NONE,)));
        assert!(app.quit_confirmation);

        assert!(!app.handle_key(KeyEvent::new(KeyCode::Char('n'), KeyModifiers::NONE,)));
        assert!(!app.quit_confirmation);

        app.handle_key(KeyEvent::new(KeyCode::Char('q'), KeyModifiers::NONE));
        assert!(app.handle_key(KeyEvent::new(KeyCode::Char('y'), KeyModifiers::NONE,)));
    }

    #[test]
    fn command_names_are_unicode_safe() {
        assert!(short_name("🦀".repeat(30).as_str(), 0).ends_with('…'));
    }
}
