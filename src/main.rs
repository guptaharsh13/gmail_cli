mod gmail_api;
mod ui;

use std::error::Error;
use std::sync::Arc;
use tokio::sync::Mutex;

use gmail_api::{Email, GmailClient};
use ui::TerminalUI;

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
        
        let emails = gmail_client.lock().await.fetch_emails().await?;

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
                ui::open_link(link)
            } else {
                Ok("No unsubscribe link found for this email.".to_string())
            }
        } else {
            Ok("No email selected.".to_string())
        }
    }
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn Error>> {
    let app = Arc::new(Mutex::new(App::new().await?));
    let mut ui = TerminalUI::new(app);
    ui.run().await?;
    Ok(())
}