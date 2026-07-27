use std::{
    io,
    time::{Duration, Instant},
};

use crossterm::event::{
    self, Event as TerminalEvent, KeyCode, KeyEvent, KeyEventKind, KeyModifiers,
};
use ratatui::{
    DefaultTerminal, Frame,
    layout::{Alignment, Constraint, Layout, Rect},
    style::{Color, Style, Stylize},
    text::{Line, Span, Text},
    widgets::{Block, BorderType, Borders, Clear, Padding, Paragraph},
};

use crate::runner::CommandSpec;

const CYAN: Color = Color::Rgb(0, 229, 255);
const BLUE: Color = Color::Rgb(90, 140, 255);
const MAGENTA: Color = Color::Rgb(255, 73, 219);
const GREEN: Color = Color::Rgb(73, 232, 151);
const RED: Color = Color::Rgb(255, 91, 112);
const YELLOW: Color = Color::Rgb(245, 200, 66);
const MUTED: Color = Color::Rgb(113, 121, 140);
const PANEL: Color = Color::Rgb(35, 39, 51);
const BACKGROUND: Color = Color::Rgb(15, 17, 23);

const LOGO: [[&str; 5]; 8] = [
    [" ████", "█    ", "█    ", "█    ", " ████"],
    [" ███ ", "█   █", "█   █", "█   █", " ███ "],
    ["█    ", "█    ", "█    ", "█    ", "█████"],
    ["█    ", "█    ", "█    ", "█    ", "█████"],
    [" ███ ", "█   █", "█████", "█   █", "█   █"],
    ["████ ", "█   █", "████ ", "█    ", "█    "],
    [" ████", "█    ", " ███ ", "    █", "████ "],
    ["█████", "█    ", "████ ", "█    ", "█████"],
];

pub fn review(
    terminal: &mut DefaultTerminal,
    initial_commands: Vec<String>,
) -> io::Result<Option<Vec<CommandSpec>>> {
    let mut pending_event = play_intro(terminal)?;

    let starts_empty = initial_commands.is_empty();
    let mut editor = CommandEditor::new(initial_commands);
    if starts_empty {
        editor.begin_edit();
    }

    loop {
        terminal.draw(|frame| editor.render(frame))?;
        let terminal_event = if let Some(pending) = pending_event.take() {
            pending
        } else {
            if !event::poll(Duration::from_millis(50))? {
                continue;
            }
            event::read()?
        };

        let action = match terminal_event {
            TerminalEvent::Key(key)
                if matches!(key.kind, KeyEventKind::Press | KeyEventKind::Repeat) =>
            {
                editor.handle_key(key)
            }
            TerminalEvent::Paste(text) => {
                editor.handle_paste(&text);
                EditorAction::Continue
            }
            _ => EditorAction::Continue,
        };

        match action {
            EditorAction::Continue => {}
            EditorAction::Run => return Ok(Some(editor.commands_for_run())),
            EditorAction::Quit => return Ok(None),
        }
    }
}

fn play_intro(terminal: &mut DefaultTerminal) -> io::Result<Option<TerminalEvent>> {
    let started = Instant::now();
    const DURATION: Duration = Duration::from_millis(1_250);

    while started.elapsed() < DURATION {
        terminal.draw(|frame| render_intro(frame, started.elapsed(), DURATION))?;
        if event::poll(Duration::from_millis(45))? {
            match event::read()? {
                TerminalEvent::Resize(_, _) => continue,
                terminal_event => return Ok(Some(terminal_event)),
            }
        }
    }
    Ok(None)
}

fn render_intro(frame: &mut Frame, elapsed: Duration, duration: Duration) {
    let screen = frame.area();
    frame.render_widget(Block::default().style(Style::new().bg(BACKGROUND)), screen);

    if screen.width < 55 || screen.height < 12 {
        frame.render_widget(
            Paragraph::new("COLLAPSE")
                .alignment(Alignment::Center)
                .style(Style::new().fg(CYAN).bg(BACKGROUND).bold()),
            Rect::new(screen.x, screen.y + screen.height / 2, screen.width, 1),
        );
        return;
    }

    let logo_width = 47;
    let logo_x = screen.x + (screen.width - logo_width) / 2;
    let logo_y = screen.y + (screen.height.saturating_sub(7)) / 2;
    let colors = [CYAN, BLUE, MAGENTA, CYAN, MAGENTA, BLUE, CYAN, MAGENTA];

    for row in 0..5 {
        let mut spans = Vec::new();
        for (index, letter) in LOGO.iter().enumerate() {
            spans.push(Span::styled(
                letter[row],
                Style::new().fg(colors[index]).bg(BACKGROUND).bold(),
            ));
            if index + 1 < LOGO.len() {
                spans.push(Span::raw(" "));
            }
        }
        frame.render_widget(
            Paragraph::new(Line::from(spans)).style(Style::new().bg(BACKGROUND)),
            Rect::new(logo_x, logo_y + row as u16, logo_width, 1),
        );
    }

    let fall_duration = duration.as_secs_f32() * 0.78;
    let progress = (elapsed.as_secs_f32() / fall_duration).min(1.0);
    let eased = progress * progress;
    let target_y = logo_y.saturating_sub(3);
    let distance = target_y.saturating_sub(screen.y + 1);
    let piece_y = screen.y + 1 + (distance as f32 * eased).round() as u16;
    let piece_x = screen.x + screen.width / 2 - 1;
    let landed = progress >= 1.0;
    let piece_color = if landed && (elapsed.as_millis() / 100).is_multiple_of(2) {
        Color::White
    } else {
        MAGENTA
    };
    frame.render_widget(
        Paragraph::new("  █\n███").style(Style::new().fg(piece_color).bg(BACKGROUND).bold()),
        Rect::new(piece_x, piece_y, 3, 2),
    );

    frame.render_widget(
        Paragraph::new("run  ·  stack  ·  inspect")
            .alignment(Alignment::Center)
            .style(Style::new().fg(MUTED).bg(BACKGROUND)),
        Rect::new(screen.x, logo_y + 6, screen.width, 1),
    );
}

struct CommandEditor {
    commands: Vec<CommandSpec>,
    selected: usize,
    editing: Option<EditState>,
    prompt: EditorPrompt,
    message: Option<String>,
}

struct EditState {
    original: String,
    cursor: usize,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum EditorPrompt {
    None,
    Run,
    Quit,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum EditorAction {
    Continue,
    Run,
    Quit,
}

impl CommandEditor {
    fn new(commands: Vec<String>) -> Self {
        let mut commands = commands
            .into_iter()
            .map(CommandSpec::required)
            .collect::<Vec<_>>();
        if commands.is_empty() {
            commands.push(CommandSpec::required(""));
        }
        Self {
            commands,
            selected: 0,
            editing: None,
            prompt: EditorPrompt::None,
            message: None,
        }
    }

    fn handle_key(&mut self, key: KeyEvent) -> EditorAction {
        if self.prompt != EditorPrompt::None {
            return self.handle_prompt(key);
        }
        if self.editing.is_some() {
            self.handle_edit_key(key);
            return EditorAction::Continue;
        }

        match key.code {
            KeyCode::Char('q') | KeyCode::Esc => self.prompt = EditorPrompt::Quit,
            KeyCode::Char('r') => self.request_run(),
            KeyCode::Char(' ') => self.toggle_required(),
            KeyCode::Enter | KeyCode::Char('e') => self.begin_edit(),
            KeyCode::Char('a' | 'o') => self.add_command(),
            KeyCode::Char('d') | KeyCode::Delete => self.delete_command(),
            KeyCode::Up | KeyCode::Char('k') => self.select_previous(),
            KeyCode::Down | KeyCode::Char('j') => self.select_next(),
            KeyCode::Char('K') => self.move_up(),
            KeyCode::Char('J') => self.move_down(),
            _ => {}
        }
        EditorAction::Continue
    }

    fn handle_prompt(&mut self, key: KeyEvent) -> EditorAction {
        match key.code {
            KeyCode::Char('y' | 'Y') => match self.prompt {
                EditorPrompt::Run => EditorAction::Run,
                EditorPrompt::Quit => EditorAction::Quit,
                EditorPrompt::None => EditorAction::Continue,
            },
            KeyCode::Char('n' | 'N') | KeyCode::Esc | KeyCode::Enter => {
                self.prompt = EditorPrompt::None;
                EditorAction::Continue
            }
            _ => EditorAction::Continue,
        }
    }

    fn handle_edit_key(&mut self, key: KeyEvent) {
        let edit = self.editing.as_mut().expect("editing state exists");
        match key.code {
            KeyCode::Enter => {
                self.editing = None;
                self.message = None;
            }
            KeyCode::Esc => {
                self.commands[self.selected].command = edit.original.clone();
                self.editing = None;
            }
            KeyCode::Left => edit.cursor = edit.cursor.saturating_sub(1),
            KeyCode::Right => {
                edit.cursor =
                    (edit.cursor + 1).min(self.commands[self.selected].command.chars().count());
            }
            KeyCode::Home => edit.cursor = 0,
            KeyCode::End => {
                edit.cursor = self.commands[self.selected].command.chars().count();
            }
            KeyCode::Backspace if edit.cursor > 0 => {
                let command = &mut self.commands[self.selected].command;
                let start = byte_index(command, edit.cursor - 1);
                let end = byte_index(command, edit.cursor);
                command.replace_range(start..end, "");
                edit.cursor -= 1;
            }
            KeyCode::Delete => {
                let command = &mut self.commands[self.selected].command;
                let length = command.chars().count();
                if edit.cursor < length {
                    let start = byte_index(command, edit.cursor);
                    let end = byte_index(command, edit.cursor + 1);
                    command.replace_range(start..end, "");
                }
            }
            KeyCode::Char('u') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                self.commands[self.selected].command.clear();
                edit.cursor = 0;
            }
            KeyCode::Char(character)
                if !key
                    .modifiers
                    .intersects(KeyModifiers::CONTROL | KeyModifiers::ALT) =>
            {
                let command = &mut self.commands[self.selected].command;
                let index = byte_index(command, edit.cursor);
                command.insert(index, character);
                edit.cursor += 1;
            }
            _ => {}
        }
    }

    fn handle_paste(&mut self, text: &str) {
        let Some(edit) = self.editing.as_mut() else {
            return;
        };
        let pasted = text.replace(['\r', '\n'], " ");
        let command = &mut self.commands[self.selected].command;
        let index = byte_index(command, edit.cursor);
        command.insert_str(index, &pasted);
        edit.cursor += pasted.chars().count();
    }

    fn begin_edit(&mut self) {
        let command = &self.commands[self.selected].command;
        self.editing = Some(EditState {
            original: command.clone(),
            cursor: command.chars().count(),
        });
        self.message = None;
    }

    fn add_command(&mut self) {
        self.selected += 1;
        self.commands
            .insert(self.selected, CommandSpec::required(""));
        self.begin_edit();
    }

    fn delete_command(&mut self) {
        self.commands.remove(self.selected);
        if self.commands.is_empty() {
            self.commands.push(CommandSpec::required(""));
        }
        self.selected = self.selected.min(self.commands.len() - 1);
        self.message = None;
    }

    fn toggle_required(&mut self) {
        self.commands[self.selected].required = !self.commands[self.selected].required;
        self.message = None;
    }

    fn select_previous(&mut self) {
        self.selected = self.selected.saturating_sub(1);
    }

    fn select_next(&mut self) {
        self.selected = (self.selected + 1).min(self.commands.len() - 1);
    }

    fn move_up(&mut self) {
        if self.selected > 0 {
            self.commands.swap(self.selected, self.selected - 1);
            self.selected -= 1;
        }
    }

    fn move_down(&mut self) {
        if self.selected + 1 < self.commands.len() {
            self.commands.swap(self.selected, self.selected + 1);
            self.selected += 1;
        }
    }

    fn request_run(&mut self) {
        if self.commands_for_run().is_empty() {
            self.message = Some("Add at least one command before running".into());
        } else {
            self.prompt = EditorPrompt::Run;
            self.message = None;
        }
    }

    fn commands_for_run(&self) -> Vec<CommandSpec> {
        self.commands
            .iter()
            .filter_map(|command| {
                let text = command.command.trim();
                (!text.is_empty()).then(|| CommandSpec {
                    command: text.to_owned(),
                    required: command.required,
                })
            })
            .collect()
    }

    fn render(&self, frame: &mut Frame) {
        let areas = Layout::vertical([
            Constraint::Length(3),
            Constraint::Min(5),
            Constraint::Length(2),
        ])
        .split(frame.area());
        frame.render_widget(
            Block::default().style(Style::new().bg(BACKGROUND)),
            frame.area(),
        );
        self.render_header(frame, areas[0]);
        self.render_commands(frame, areas[1]);
        self.render_footer(frame, areas[2]);
        if self.prompt != EditorPrompt::None {
            self.render_prompt(frame);
        }
    }

    fn render_header(&self, frame: &mut Frame, area: Rect) {
        frame.render_widget(
            Paragraph::new(vec![
                Line::from(vec![
                    Span::styled(
                        " COLLAPSE ",
                        Style::new().fg(Color::Black).bg(MAGENTA).bold(),
                    ),
                    Span::styled("  COMMAND QUEUE", Style::new().fg(CYAN).bold()),
                ]),
                Line::styled(
                    "Required commands stop the queue on failure; Space toggles the selection.",
                    Style::new().fg(MUTED),
                ),
            ])
            .style(Style::new().bg(BACKGROUND)),
            area,
        );
    }

    fn render_commands(&self, frame: &mut Frame, area: Rect) {
        let viewport_height = area.height.saturating_sub(2) as usize;
        let start = self
            .selected
            .saturating_sub(viewport_height.saturating_sub(1));
        let end = (start + viewport_height).min(self.commands.len());
        let lines = self.commands[start..end]
            .iter()
            .enumerate()
            .map(|(offset, command)| {
                let index = start + offset;
                self.command_line(index, command)
            })
            .collect::<Vec<_>>();

        frame.render_widget(
            Paragraph::new(Text::from(lines))
                .block(
                    Block::default()
                        .title(format!(" COMMANDS · {} ", self.commands_for_run().len()))
                        .title_style(Style::new().fg(MAGENTA).bold())
                        .borders(Borders::ALL)
                        .border_type(BorderType::Rounded)
                        .border_style(Style::new().fg(PANEL))
                        .padding(Padding::horizontal(1)),
                )
                .style(Style::new().bg(BACKGROUND)),
            area,
        );
    }

    fn command_line(&self, index: usize, command: &CommandSpec) -> Line<'static> {
        let selected = index == self.selected;
        let text = command.command.as_str();
        let row_style = if selected {
            Style::new().bg(PANEL)
        } else {
            Style::new().bg(BACKGROUND)
        };
        let mut spans = vec![
            Span::styled(
                if selected { "▶ " } else { "  " },
                row_style.fg(CYAN).bold(),
            ),
            Span::styled(format!("{:02}  ", index + 1), row_style.fg(MUTED)),
            Span::styled(
                if command.required {
                    "[required] "
                } else {
                    "[optional] "
                },
                row_style
                    .fg(if command.required { GREEN } else { YELLOW })
                    .bold(),
            ),
        ];

        if selected && let Some(edit) = &self.editing {
            let cursor_byte = byte_index(text, edit.cursor);
            spans.push(Span::styled(
                text[..cursor_byte].to_owned(),
                row_style.fg(Color::White),
            ));
            spans.push(Span::styled("█", row_style.fg(MAGENTA).bold()));
            spans.push(Span::styled(
                text[cursor_byte..].to_owned(),
                row_style.fg(Color::White),
            ));
        } else if text.is_empty() {
            spans.push(Span::styled(
                "<empty command>",
                row_style.fg(MUTED).italic(),
            ));
        } else {
            spans.push(Span::styled(text.to_owned(), row_style.fg(Color::White)));
        }
        Line::from(spans).style(row_style)
    }

    fn render_footer(&self, frame: &mut Frame, area: Rect) {
        let lines = if self.editing.is_some() {
            vec![
                Line::from(vec![
                    " Enter ".black().on_cyan().bold(),
                    " save  ".fg(MUTED),
                    "Esc ".fg(RED).bold(),
                    " cancel  ".fg(MUTED),
                    "←→ ".fg(CYAN).bold(),
                    " move cursor  ".fg(MUTED),
                    "Ctrl+U ".fg(MAGENTA).bold(),
                    " clear".fg(MUTED),
                ]),
                Line::styled(
                    self.message.as_deref().unwrap_or("Editing shell command"),
                    Style::new().fg(MUTED),
                ),
            ]
        } else {
            vec![
                Line::from(vec![
                    " Space ".black().on_magenta().bold(),
                    " required  ".fg(MUTED),
                    "Enter/e ".black().on_cyan().bold(),
                    " edit  ".fg(MUTED),
                    "a ".fg(CYAN).bold(),
                    " add  ".fg(MUTED),
                    "d ".fg(RED).bold(),
                    " delete  ".fg(MUTED),
                    "J/K ".fg(MAGENTA).bold(),
                    " reorder  ".fg(MUTED),
                    "r ".fg(GREEN).bold(),
                    " run  ".fg(MUTED),
                    "q ".fg(RED).bold(),
                    " quit".fg(MUTED),
                ]),
                Line::styled(
                    self.message
                        .as_deref()
                        .unwrap_or("Nothing runs until you confirm."),
                    Style::new().fg(if self.message.is_some() { RED } else { MUTED }),
                ),
            ]
        };
        frame.render_widget(
            Paragraph::new(lines).style(Style::new().bg(BACKGROUND)),
            area,
        );
    }

    fn render_prompt(&self, frame: &mut Frame) {
        let screen = frame.area();
        let width = screen.width.min(62);
        let height = screen.height.min(5);
        let area = Rect::new(
            screen.x + screen.width.saturating_sub(width) / 2,
            screen.y + screen.height.saturating_sub(height) / 2,
            width,
            height,
        );
        let (title, question, yes) = match self.prompt {
            EditorPrompt::Run => {
                let commands = self.commands_for_run();
                let required = commands.iter().filter(|command| command.required).count();
                (
                    " RUN COMMANDS ",
                    format!(
                        "Run {} commands sequentially ({required} required)?",
                        commands.len()
                    ),
                    " run",
                )
            }
            EditorPrompt::Quit => (
                " DISCARD QUEUE ",
                "Quit without running these commands?".into(),
                " quit",
            ),
            EditorPrompt::None => return,
        };

        frame.render_widget(Clear, area);
        frame.render_widget(
            Paragraph::new(Text::from(vec![
                Line::styled(question, Style::new().fg(Color::White).bold()),
                Line::from(vec![
                    Span::styled("y", Style::new().fg(GREEN).bold()),
                    Span::styled(yes, Style::new().fg(MUTED)),
                    Span::styled("   n / Esc / Enter", Style::new().fg(RED).bold()),
                    Span::styled(" go back", Style::new().fg(MUTED)),
                ]),
            ]))
            .alignment(Alignment::Center)
            .style(Style::new().bg(PANEL))
            .block(
                Block::default()
                    .title(title)
                    .title_style(Style::new().fg(MAGENTA).bold())
                    .borders(Borders::ALL)
                    .border_type(BorderType::Rounded)
                    .border_style(Style::new().fg(if self.prompt == EditorPrompt::Run {
                        GREEN
                    } else {
                        RED
                    }))
                    .style(Style::new().bg(PANEL)),
            ),
            area,
        );
    }
}

fn byte_index(value: &str, char_index: usize) -> usize {
    value
        .char_indices()
        .nth(char_index)
        .map(|(index, _)| index)
        .unwrap_or(value.len())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn command_texts(editor: &CommandEditor) -> Vec<&str> {
        editor
            .commands
            .iter()
            .map(|command| command.command.as_str())
            .collect()
    }

    #[test]
    fn empty_editor_starts_with_one_editable_row() {
        let editor = CommandEditor::new(Vec::new());
        assert_eq!(command_texts(&editor), [""]);
        assert!(editor.commands[0].required);
        assert!(editor.commands_for_run().is_empty());
    }

    #[test]
    fn editor_can_add_edit_reorder_and_delete_commands() {
        let mut editor = CommandEditor::new(vec!["first".into(), "second".into()]);
        editor.add_command();
        editor.handle_paste("third");
        editor.handle_edit_key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE));
        assert_eq!(command_texts(&editor), ["first", "third", "second"]);

        editor.move_down();
        assert_eq!(command_texts(&editor), ["first", "second", "third"]);
        editor.delete_command();
        assert_eq!(command_texts(&editor), ["first", "second"]);
    }

    #[test]
    fn unicode_cursor_edits_on_character_boundaries() {
        let mut editor = CommandEditor::new(vec!["echo 🦀".into()]);
        editor.begin_edit();
        editor.handle_edit_key(KeyEvent::new(KeyCode::Backspace, KeyModifiers::NONE));
        assert_eq!(command_texts(&editor), ["echo "]);
    }

    #[test]
    fn commands_are_required_by_default_and_can_be_toggled() {
        let mut editor = CommandEditor::new(vec!["build".into()]);
        assert!(editor.commands[0].required);

        editor.toggle_required();
        assert!(!editor.commands[0].required);
        assert!(!editor.commands_for_run()[0].required);
    }

    #[test]
    fn run_always_requires_confirmation() {
        let mut editor = CommandEditor::new(vec!["cargo test".into()]);
        editor.request_run();
        assert_eq!(editor.prompt, EditorPrompt::Run);
        assert_eq!(
            editor.handle_prompt(KeyEvent::new(KeyCode::Char('n'), KeyModifiers::NONE)),
            EditorAction::Continue
        );
        assert_eq!(editor.prompt, EditorPrompt::None);
    }
}
