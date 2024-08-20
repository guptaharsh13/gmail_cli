use std::error::Error;
use std::process::Command;
use std::sync::Arc;
use tokio::sync::Mutex;

use crate::gmail_api::{Email, GmailClient};

pub struct App {
    pub emails: Vec<Email>,
    pub current_index: usize,
    gmail_client: Arc<Mutex<GmailClient>>,
}

impl App {
    pub async fn new() -> Result<Self, Box<dyn Error>> {
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

    pub async fn mark_as_read(&mut self) -> Result<(), Box<dyn Error>> {
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

    pub async fn unsubscribe(&self) -> Result<String, Box<dyn Error>> {
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
