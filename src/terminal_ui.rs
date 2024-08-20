use std::io;
use std::sync::Arc;
use tokio::sync::Mutex;

use crossterm::{
    event::{self, Event, KeyCode},
    execute,
    terminal::{disable_raw_mode, enable_raw_mode, EnterAlternateScreen, LeaveAlternateScreen},
};
use tui::{
    backend::{Backend, CrosstermBackend},
    layout::{Constraint, Direction, Layout, Rect},
    style::{Color, Modifier, Style},
    text::{Span, Spans},
    widgets::{Block, Borders, List, ListItem, ListState, Widget},
    Frame, Terminal,
};

use crate::app::App;

struct ScrollableText<'a> {
    content: &'a str,
    offset: usize,
    block: Option<Block<'a>>,
    style: Style,
}

impl<'a> ScrollableText<'a> {
    pub fn new(content: &'a str) -> ScrollableText<'a> {
        ScrollableText {
            content,
            offset: 0,
            block: None,
            style: Style::default(),
        }
    }

    pub fn block(mut self, block: Block<'a>) -> ScrollableText<'a> {
        self.block = Some(block);
        self
    }

    pub fn style(mut self, style: Style) -> ScrollableText<'a> {
        self.style = style;
        self
    }

    pub fn scroll(&mut self, offset: usize) {
        self.offset = offset;
    }
}

impl<'a> Widget for ScrollableText<'a> {
    fn render(mut self, area: Rect, buf: &mut tui::buffer::Buffer) {
        let text_area = match self.block.take() {
            Some(b) => {
                let inner_area = b.inner(area);
                b.render(area, buf);
                inner_area
            }
            None => area,
        };

        let lines: Vec<&str> = self.content.lines().skip(self.offset).collect();
        for (i, line) in lines.iter().enumerate() {
            if i >= text_area.height as usize {
                break;
            }
            let trimmed_line = line.trim_end();
            buf.set_string(
                text_area.left(),
                text_area.top() + i as u16,
                trimmed_line,
                self.style,
            );
        }
    }
}

pub struct TerminalUI {
    app: Arc<Mutex<App>>,
    status_message: String,
    scroll_offset: usize,
}

impl TerminalUI {
    pub fn new(app: Arc<Mutex<App>>) -> Self {
        Self {
            app,
            status_message: String::new(),
            scroll_offset: 0,
        }
    }

    fn ui<B: Backend>(&self, f: &mut Frame<B>) {
        let chunks = Layout::default()
            .direction(Direction::Vertical)
            .constraints([Constraint::Min(3), Constraint::Length(3)].as_ref())
            .split(f.size());

        let main_chunks = Layout::default()
            .direction(Direction::Horizontal)
            .constraints([Constraint::Percentage(30), Constraint::Percentage(70)].as_ref())
            .split(chunks[0]);

        let app = self.app.try_lock().expect("Failed to acquire app lock");

        // Render email list
        let emails: Vec<ListItem> = app
            .emails
            .iter()
            .map(|email| {
                ListItem::new(vec![Spans::from(Span::raw(&email.subject))])
                    .style(Style::default().fg(Color::White))
            })
            .collect();

        let mut state = ListState::default();
        state.select(Some(app.current_index));

        let emails = List::new(emails)
            .block(Block::default().borders(Borders::ALL).title("Emails"))
            .highlight_style(Style::default().bg(Color::LightGreen).add_modifier(Modifier::BOLD))
            .highlight_symbol(">> ");

        f.render_stateful_widget(emails, main_chunks[0], &mut state);

        // Render email content
        let email_content = if let Some(email) = app.emails.get(app.current_index) {
            email.body.clone()
        } else {
            String::from("No email selected")
        };
        
        let mut scrollable_content = ScrollableText::new(&email_content)
            .block(Block::default().borders(Borders::ALL).title("Content"))
            .style(Style::default());
        scrollable_content.scroll(self.scroll_offset);
        
        f.render_widget(scrollable_content, main_chunks[1]);

        // Render status bar
        let status_bar_width = chunks[1].width as usize - 2; // Subtracting 2 for borders
        let truncated_message = self.truncate_with_ellipsis(&self.status_message, status_bar_width);
        let status_bar = tui::widgets::Paragraph::new(truncated_message)
            .style(Style::default().fg(Color::White).bg(Color::Black))
            .block(Block::default().borders(Borders::ALL))
            .wrap(tui::widgets::Wrap { trim: true });

        f.render_widget(status_bar, chunks[1]);

        // Render controls
        let controls = tui::widgets::Paragraph::new("Q: Quit | R: Mark as Read | U: Unsubscribe | ↑↓: Navigate | PgUp/PgDn: Scroll")
            .style(Style::default().fg(Color::White).bg(Color::DarkGray));

        let control_area = Rect {
            x: chunks[1].x,
            y: chunks[1].y + chunks[1].height - 1,
            width: chunks[1].width,
            height: 1,
        };

        f.render_widget(controls, control_area);
    }

    fn truncate_with_ellipsis(&self, s: &str, max_width: usize) -> String {
        if s.len() <= max_width {
            s.to_string()
        } else {
            let mut result = String::with_capacity(max_width);
            let mut char_indices = s.char_indices();
            let mut current_width = 0;

            while let Some((_idx, ch)) = char_indices.next() {
                if current_width + ch.len_utf8() + 3 > max_width { // +3 for "..."
                    result.push_str("...");
                    break;
                }
                result.push(ch);
                current_width += ch.len_utf8();
            }

            result
        }
    }

    async fn run_app<B: Backend>(&mut self, terminal: &mut Terminal<B>) -> io::Result<()> {
        loop {
            terminal.draw(|f| self.ui(f))?;

            if let Event::Key(key) = event::read()? {
                let mut app = self.app.lock().await;
                match key.code {
                    KeyCode::Char('q') => return Ok(()),
                    KeyCode::Up => {
                        if app.current_index > 0 {
                            app.current_index -= 1;
                            self.scroll_offset = 0;  // Reset scroll when changing emails
                        }
                    }
                    KeyCode::Down => {
                        if app.current_index < app.emails.len().saturating_sub(1) {
                            app.current_index += 1;
                            self.scroll_offset = 0;  // Reset scroll when changing emails
                        }
                    }
                    KeyCode::Char('r') => {
                        match app.mark_as_read().await {
                            Ok(_) => self.status_message = "Email marked as read.".to_string(),
                            Err(e) => self.status_message = format!("Error marking email as read: {}", e),
                        }
                    }
                    KeyCode::Char('u') => {
                        match app.unsubscribe().await {
                            Ok(message) => self.status_message = message,
                            Err(e) => self.status_message = format!("Error unsubscribing: {}", e),
                        }
                    }
                    KeyCode::PageUp => {
                        self.scroll_offset = self.scroll_offset.saturating_sub(10);
                    }
                    KeyCode::PageDown => {
                        if let Some(email) = app.emails.get(app.current_index) {
                            let content_height = email.body.lines().count();
                            let visible_height = terminal.size()?.height as usize - 6; // Subtracting space for borders and status bar
                            let max_scroll = content_height.saturating_sub(visible_height);
                            self.scroll_offset = (self.scroll_offset + 10).min(max_scroll);
                        }
                    }
                    _ => {}
                }
            }
        }
    }

    pub async fn run(&mut self) -> io::Result<()> {
        enable_raw_mode()?;
        let mut stdout = io::stdout();
        execute!(stdout, EnterAlternateScreen)?;

        let backend = CrosstermBackend::new(stdout);
        let mut terminal = Terminal::new(backend)?;

        let res = self.run_app(&mut terminal).await;

        disable_raw_mode()?;
        execute!(
            terminal.backend_mut(),
            LeaveAlternateScreen,
        )?;
        terminal.show_cursor()?;

        if let Err(err) = res {
            println!("{:?}", err)
        }

        Ok(())
    }
}
