use std::error::Error;
use std::io;
use std::sync::Arc;
use std::process::Command;

use crossterm::{
    event::{self, Event, KeyCode},
    execute,
    terminal::{disable_raw_mode, enable_raw_mode, EnterAlternateScreen, LeaveAlternateScreen},
};
use tokio::sync::Mutex;
use tui::{
    backend::{Backend, CrosstermBackend},
    layout::{Constraint, Direction, Layout, Rect},
    style::{Color, Modifier, Style},
    text::{Span, Spans},
    widgets::{Block, Borders, List, ListItem, ListState, Paragraph, Widget},
    Frame, Terminal,
};
use unicode_width::UnicodeWidthStr;
use unicode_width::UnicodeWidthChar;

use crate::App;

pub struct ScrollableText<'a> {
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

        self.render_email_list(f, &app, main_chunks[0]);
        self.render_email_content(f, &app, main_chunks[1]);
        self.render_status_bar(f, chunks[1]);
        self.render_controls(f, chunks[1]);
    }

    fn render_email_list<B: Backend>(&self, f: &mut Frame<B>, app: &App, area: Rect) {
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
            .highlight_style(Style::
                default().bg(Color::LightGreen).add_modifier(Modifier::BOLD))
            .highlight_symbol(">> ");

        f.render_stateful_widget(emails, area, &mut state);
    }

    fn render_email_content<B: Backend>(&self, f: &mut Frame<B>, app: &App, area: Rect) {
        let email_content = if let Some(email) = app.emails.get(app.current_index) {
            email.body.clone()
        } else {
            String::from("No email selected")
        };
        
        let mut scrollable_content = ScrollableText::new(&email_content)
            .block(Block::default().borders(Borders::ALL).title("Content"))
            .style(Style::default());
        scrollable_content.scroll(self.scroll_offset);
        
        f.render_widget(scrollable_content, area);
    }

    fn render_status_bar<B: Backend>(&self, f: &mut Frame<B>, area: Rect) {
        let status_bar_width = area.width as usize - 2; // Subtracting 2 for borders
        let truncated_message = truncate_with_ellipsis(&self.status_message, status_bar_width);
        let status_bar = Paragraph::new(truncated_message)
            .style(Style::default().fg(Color::White).bg(Color::Black))
            .block(Block::default().borders(Borders::ALL))
            .wrap(tui::widgets::Wrap { trim: true });

        f.render_widget(status_bar, area);
    }

    fn render_controls<B: Backend>(&self, f: &mut Frame<B>, area: Rect) {
        let controls = Paragraph::new("Q: Quit | R: Mark as Read | U: Unsubscribe | ↑↓: Navigate | PgUp/PgDn: Scroll")
            .style(Style::default().fg(Color::White).bg(Color::DarkGray));

        let control_area = Rect {
            x: area.x,
            y: area.y + area.height - 1,
            width: area.width,
            height: 1,
        };

        f.render_widget(controls, control_area);
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

    async fn run_app<B: Backend>(&mut self, terminal: &mut Terminal<B>) -> io::Result<()> {
        loop {
            terminal.draw(|f| self.ui(f))?;

            if let Event::Key(key) = event::read()? {
                match key.code {
                    KeyCode::Char('q') => return Ok(()),
                    KeyCode::Up => self.handle_up().await,
                    KeyCode::Down => self.handle_down().await,
                    KeyCode::Char('r') => self.handle_mark_as_read().await,
                    KeyCode::Char('u') => self.handle_unsubscribe().await,
                    KeyCode::PageUp => self.handle_page_up(),
                    KeyCode::PageDown => self.handle_page_down(terminal).await,
                    _ => {}
                }
            }
        }
    }

    async fn handle_up(&mut self) {
        let mut app = self.app.lock().await;
        if app.current_index > 0 {
            app.current_index -= 1;
            self.scroll_offset = 0;  // Reset scroll when changing emails
        }
    }

    async fn handle_down(&mut self) {
        let mut app = self.app.lock().await;
        if app.current_index < app.emails.len().saturating_sub(1) {
            app.current_index += 1;
            self.scroll_offset = 0;  // Reset scroll when changing emails
        }
    }

    async fn handle_mark_as_read(&mut self) {
        let mut app = self.app.lock().await;
        match app.mark_as_read().await {
            Ok(_) => self.status_message = "Email marked as read.".to_string(),
            Err(e) => self.status_message = format!("Error marking email as read: {}", e),
        }
    }

    async fn handle_unsubscribe(&mut self) {
        let app = self.app.lock().await;
        match app.unsubscribe().await {
            Ok(message) => self.status_message = message,
            Err(e) => self.status_message = format!("Error unsubscribing: {}", e),
        }
    }

    fn handle_page_up(&mut self) {
        self.scroll_offset = self.scroll_offset.saturating_sub(10);
    }

    async fn handle_page_down<B: Backend>(&mut self, terminal: &Terminal<B>) {
        let app = self.app.lock().await;
        if let Some(email) = app.emails.get(app.current_index) {
            let content_height = email.body.lines().count();
            let visible_height = terminal.size().unwrap().height as usize - 6; // Subtracting space for borders and status bar
            let max_scroll = content_height.saturating_sub(visible_height);
            self.scroll_offset = (self.scroll_offset + 10).min(max_scroll);
        }
    }
}


pub fn open_link(link: &str) -> Result<String, Box<dyn Error>> {
    let (program, args) = if cfg!(target_os = "linux") {
        ("xdg-open", vec![link])
    } else if cfg!(target_os = "macos") {
        ("open", vec![link])
    } else if cfg!(target_os = "windows") {
        ("cmd", vec!["/C", "start", "", link])
    } else {
        return Err("Unsupported operating system".into());
    };

    let status = Command::new(program)
        .args(&args)
        .status()?;

    if status.success() {
        Ok("Unsubscribe link opened.".to_string())
    } else {
        Err(format!("Failed to open unsubscribe link: {}", link).into())
    }
}

fn truncate_with_ellipsis(s: &str, max_width: usize) -> String {
    if s.width() <= max_width {
        s.to_string()
    } else {
        let mut result = String::with_capacity(max_width);
        let mut current_width = 0;

        for ch in s.chars() {
            if current_width + ch.width().unwrap_or(0) + 3 > max_width { // +3 for "..."
                result.push_str("...");
                break;
            }
            result.push(ch);
            current_width += ch.width().unwrap_or(0);
        }

        result
    }
}