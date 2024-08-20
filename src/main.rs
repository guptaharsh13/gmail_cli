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
    widgets::{Block, Borders, List, ListItem, ListState, Widget},
    Frame, Terminal,
};
use unicode_width::UnicodeWidthStr;

mod gmail_api;
use gmail_api::{Email, GmailClient};

struct App {
    emails: Vec<Email>,
    current_index: usize,
    gmail_client: Arc<Mutex<GmailClient>>,
}

impl App {
    async fn new() -> Result<Self, Box<dyn Error>> {
        let secret = yup_oauth2::read_application_secret("client_secret.json").await?;
        let auth = yup_oauth2::InstalledFlowAuthenticator::builder(secret, yup_oauth2::InstalledFlowReturnMethod::HTTPRedirect)
            .persist_tokens_to_disk("token_cache.json")
            .build()
            .await?;

        let scopes = &["https://www.googleapis.com/auth/gmail.modify"];
        let token = auth.token(scopes).await?;
        let gmail_client = Arc::new(Mutex::new(GmailClient::new(token)));
        
        let emails = match gmail_client.lock().await.fetch_emails().await {
            Ok(emails) => emails,
            Err(e) => {
                eprintln!("Error fetching emails: {}", e);
                Vec::new()
            }
        };

        Ok(Self {
            emails,
            current_index: 0,
            gmail_client,
        })
    }

    async fn mark_as_read(&mut self) -> Result<(), Box<dyn Error>> {
        if let Some(email) = self.emails.get(self.current_index) {
            self.gmail_client
                .lock()
                .await
                .mark_as_read(&email.id)
                .await?;
            self.emails.remove(self.current_index);
            if self.current_index >= self.emails.len() {
                self.current_index = self.emails.len().saturating_sub(1);
            }
        }
        Ok(())
    }

    async fn unsubscribe(&self) -> Result<String, Box<dyn Error>> {
        if let Some(email) = self.emails.get(self.current_index) {
            if let Some(link) = &email.unsubscribe_link {
                if link.starts_with("http") {
                    let (program, args) = if cfg!(target_os = "linux") {
                        ("xdg-open", vec![link.as_str()])
                    } else if cfg!(target_os = "macos") {
                        ("open", vec![link.as_str()])
                    } else if cfg!(target_os = "windows") {
                        ("cmd", vec!["/C", "start", "", link.as_str()])
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
                } else if link.starts_with("mailto:") {
                    Ok(format!("This email uses a mailto link for unsubscribing. Please send an email to {}", &link[7..]))
                } else {
                    Ok(format!("Unsupported unsubscribe method: {}", link))
                }
            } else {
                Ok("No unsubscribe link found for this email.".to_string())
            }
        } else {
            Ok("No email selected.".to_string())
        }
    }
}

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

struct TerminalUI {
    app: Arc<Mutex<App>>,
    status_message: String,
    scroll_offset: usize,
}

impl TerminalUI {
    fn new(app: Arc<Mutex<App>>) -> Self {
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

            while let Some((idx, ch)) = char_indices.next() {
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
                        if self.scroll_offset > 0 {
                            self.scroll_offset = self.scroll_offset.saturating_sub(10);
                        }
                    }
                    KeyCode::PageDown => {
                        self.scroll_offset += 10;
                    }
                    _ => {}
                }
            }
        }
    }

    async fn run(&mut self) -> io::Result<()> {
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

#[tokio::main]
async fn main() -> Result<(), Box<dyn Error>> {
    let app = Arc::new(Mutex::new(App::new().await?));
    let mut ui = TerminalUI::new(app);
    ui.run().await?;
    Ok(())
}