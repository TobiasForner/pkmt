use anyhow::{Context, Result};
use ratatui::crossterm::event::{self, Event, KeyCode, KeyEvent, KeyEventKind};
use ratatui::layout::{Constraint, Layout};
use ratatui::prelude::{Buffer, Rect, Widget};
use ratatui::style::Stylize;
use ratatui::symbols::border;
use ratatui::text::{Line, Text};
use ratatui::widgets::{Block, Paragraph};
use ratatui::{DefaultTerminal, Frame};
use regex::Regex;

use crate::util::{self, file_link_pattern, link_name_pattern};

struct InteractiveState {
    data_pos: usize,
    data: Vec<String>,
    choices: Vec<String>,
    done_choices: Vec<Option<usize>>,
    choices_pos: usize,
    exit: bool,
}

impl InteractiveState {
    fn new(data: Vec<String>, choices: Vec<String>) -> Self {
        Self {
            data_pos: 0,
            data,
            choices,
            choices_pos: 0,
            done_choices: Vec::new(),
            exit: false,
        }
    }
    fn run(&mut self, terminal: &mut DefaultTerminal) -> Result<()> {
        while !self.exit {
            terminal.draw(|frame| self.draw(frame))?;
            self.handle_events()?;
        }
        Ok(())
    }

    fn draw(&self, frame: &mut Frame) {
        frame.render_widget(self, frame.area());
    }

    fn handle_events(&mut self) -> Result<()> {
        match event::read()? {
            Event::Key(key_event) if key_event.kind == KeyEventKind::Press => {
                self.handle_key_event(key_event);
            }
            _ => {}
        };
        Ok(())
    }

    fn handle_key_event(&mut self, key_event: KeyEvent) {
        match key_event.code {
            KeyCode::Up => {
                self.choices_pos = self.choices_pos.saturating_sub(1);
            }
            KeyCode::Down => {
                self.choices_pos = self
                    .choices_pos
                    .saturating_add(1)
                    .min(self.choices.len().saturating_sub(1));
            }
            KeyCode::Char(' ') => {
                self.done_choices.push(Some(self.choices_pos));
                self.advance();
            }
            KeyCode::Char('s') => {
                self.done_choices.push(None);
                self.advance();
            }
            KeyCode::Char('c') => {
                (self.data_pos..self.data.len()).for_each(|_| {
                    self.done_choices.push(None);
                });
                self.exit = true;
            }
            _ => {}
        }
    }
    fn advance(&mut self) {
        if self.data_pos < self.data.len().saturating_sub(1) {
            self.data_pos += 1;
        } else {
            self.exit = true;
        }
    }
}

impl Widget for &InteractiveState {
    fn render(self, area: Rect, buf: &mut Buffer) {
        let instructions = Line::from(vec![
            " Choose ".into(),
            "<Up>/<Down>".blue(),
            " Select ".into(),
            "<Space>".blue(),
            " Skip ".into(),
            "s".blue(),
            " Cancel ".into(),
            "c".blue(),
            " ".into(),
        ]);
        let choices_block = Block::bordered()
            .title("Interactive assignment")
            .title_bottom(instructions.centered())
            .border_set(border::THICK);
        let choices_text = Text::from(
            self.choices
                .iter()
                .enumerate()
                .map(|(i, c)| {
                    if i == self.choices_pos {
                        c.as_str().blue().into()
                    } else {
                        c.as_str().into()
                    }
                })
                .collect::<Vec<_>>(),
        );
        let row_constrains = vec![Constraint::Percentage(10), Constraint::Percentage(90)];
        let vertical = Layout::vertical(row_constrains);
        let rows = vertical.split(area);
        Paragraph::new(self.data[self.data_pos].clone()).render(rows[0], buf);
        Paragraph::new(choices_text)
            .centered()
            .block(choices_block)
            .render(rows[1], buf);
    }
}

use super::{TaskData, config::Config, todoist_api::TodoistTask};
#[derive(Debug)]
pub enum Resolution {
    ToHandle,
    Skip,
}

pub fn get_full_interactive_data(
    tasks: &[TodoistTask],
    template_names: &[String],
    config: &Config,
) -> Result<Vec<(Resolution, TaskData)>> {
    use Resolution::*;
    let task_data = tasks.iter().map(|t| t.content.to_string()).collect();
    let choices = template_names.to_vec();
    let mut interactive_state = InteractiveState::new(task_data, choices);
    ratatui::run(|terminal| interactive_state.run(terminal))?;
    let res = interactive_state
        .done_choices
        .iter()
        .enumerate()
        .map(|(task_index, template_index)| {
            if let Some(template_index) = template_index {
                let template_name = &template_names[*template_index];

                let content = util::apply_substitutions(&tasks[task_index].content);
                let url_re = url_re().unwrap();
                if let Some(captures) = url_re.captures(&content) {
                    let mut tags = vec![];
                    let title = if let Some(title) = captures.get(1) {
                        let title = title.as_str().trim().to_string();
                        tags = config.get_keyword_tags(&title);
                        if title.is_empty() {
                            Some("untitled".to_string())
                        } else {
                            Some(title)
                        }
                    } else {
                        println!("No title capture: {content}");
                        None
                    };

                    let mut sources = vec![];
                    let url = if let Some(url) = captures.get(2) {
                        let url = url.as_str().to_string();
                        let url_tags = config.get_url_tags(&url);
                        url_tags.into_iter().for_each(|ut| {
                            if !tags.contains(&ut) {
                                tags.push(ut);
                            }
                        });

                        sources = config.get_url_sources(&url);
                        Some(url)
                    } else {
                        println!("No url capture: {content}");
                        None
                    };
                    (
                        ToHandle,
                        TaskData::Interactive(
                            template_name.clone(),
                            url.clone(),
                            title,
                            tags,
                            sources,
                        ),
                    )
                } else {
                    println!("No url match: {content:?} with {url_re:?}");
                    (Skip, TaskData::Unhandled)
                }
            } else {
                (Skip, TaskData::Unhandled)
            }
        })
        .collect();
    Ok(res)
}

fn url_re() -> Result<Regex> {
    let pattern = format!(r"\[{}\]\({}\)", link_name_pattern(), file_link_pattern());
    let url_re = Regex::new(&pattern);
    url_re.context("failed to construct url_re")
}
