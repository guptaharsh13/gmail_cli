use std::error::Error;
use std::io;
use std::sync::Arc;
use std::process::Command;  // Add this line

use async_trait::async_trait;
use crossterm::{
    event::{self, DisableMouseCapture, EnableMouseCapture, Event, KeyCode},
    execute,
    terminal::{disable_raw_mode, enable_raw_mode, EnterAlternateScreen, LeaveAlternateScreen},
};
use tokio::sync::Mutex;
use tui::{
    backend::{Backend, CrosstermBackend},
    layout::{Constraint, Direction, Layout, Rect},
    style::{Color, Modifier, Style},
    text::{Span, Spans},
    widgets::{Block, Borders, List, ListItem, ListState, Paragraph},
    Frame, Terminal,
};

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

    async fn unsubscribe(&self) -> Result<(), Box<dyn Error>> {
        if let Some(email) = self.emails.get(self.current_index) {
            if let Some(link) = &email.unsubscribe_link {
                // Use xdg-open on Linux, open on macOS, or start on Windows
                let program = if cfg!(target_os = "linux") {
                    "xdg-open"
                } else if cfg!(target_os = "macos") {
                    "open"
                } else if cfg!(target_os = "windows") {
                    "start"
                } else {
                    return Err("Unsupported operating system".into());
                };

                Command::new(program)
                    .arg(link)
                    .output()
                    .map_err(|e| format!("Failed to open unsubscribe link: {}", e))?;
            }
        }
        Ok(())
    }
}

#[async_trait]
trait UIHandler {
    async fn run(&mut self) -> io::Result<()>;
}

struct TerminalUI {
    app: Arc<Mutex<App>>,
}

#[async_trait]
impl UIHandler for TerminalUI {
    async fn run(&mut self) -> io::Result<()> {
        enable_raw_mode()?;
        let mut stdout = io::stdout();
        execute!(stdout, EnterAlternateScreen, EnableMouseCapture)?;
        let backend = CrosstermBackend::new(stdout);
        let mut terminal = Terminal::new(backend)?;

        let res = self.run_app(&mut terminal).await;

        disable_raw_mode()?;
        execute!(
            terminal.backend_mut(),
            LeaveAlternateScreen,
            DisableMouseCapture
        )?;
        terminal.show_cursor()?;

        if let Err(err) = res {
            println!("{:?}", err)
        }

        Ok(())
    }
}

impl TerminalUI {
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
                        }
                    }
                    KeyCode::Down => {
                        if app.current_index < app.emails.len().saturating_sub(1) {
                            app.current_index += 1;
                        }
                    }
                    KeyCode::Char('r') => {
                        if let Err(e) = app.mark_as_read().await {
                            eprintln!("Error marking email as read: {}", e);
                        }
                    }
                    KeyCode::Char('u') => {
                        if let Err(e) = app.unsubscribe().await {
                            eprintln!("Error unsubscribing: {}", e);
                        }
                    }
                    _ => {}
                }
            }
        }
    }

    fn ui<B: Backend>(&self, f: &mut Frame<B>) {
        let chunks = Layout::default()
            .direction(Direction::Vertical)
            .constraints([Constraint::Min(0), Constraint::Length(3)].as_ref())
            .split(f.size());

        let main_chunks = Layout::default()
            .direction(Direction::Horizontal)
            .constraints([Constraint::Percentage(30), Constraint::Percentage(70)].as_ref())
            .split(chunks[0]);

        let app = self.app.try_lock().expect("Failed to acquire app lock");

        let emails: Vec<ListItem> = app
            .emails
            .iter()
            .map(|email| {
                ListItem::new(vec![Spans::from(Span::raw(&email.subject))])
                    .style(Style::default().fg(Color::White).bg(Color::Black))
            })
            .collect();

        let mut state = ListState::default();
        state.select(Some(app.current_index));

        let emails = List::new(emails)
            .block(Block::default().borders(Borders::ALL).title("Emails"))
            .highlight_style(
                Style::default()
                    .bg(Color::LightGreen)
                    .add_modifier(Modifier::BOLD),
            )
            .highlight_symbol(">> ");

        f.render_stateful_widget(emails, main_chunks[0], &mut state);

        let email_content = if let Some(email) = app.emails.get(app.current_index) {
            email.body.clone()
        } else {
            String::from("No email selected")
        };

        let email_content = Paragraph::new(email_content)
            .block(Block::default().borders(Borders::ALL).title("Content"));

        f.render_widget(email_content, main_chunks[1]);

        let controls = Paragraph::new("Q: Quit | R: Mark as Read | U: Unsubscribe | ↑↓: Navigate")
            .style(Style::default().fg(Color::White).bg(Color::DarkGray))
            .block(Block::default().borders(Borders::ALL));

        f.render_widget(controls, chunks[1]);
    }
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn Error>> {
    let app = Arc::new(Mutex::new(App::new().await?));
    let mut ui = TerminalUI { app };
    ui.run().await?;
    Ok(())
}