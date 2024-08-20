use std::error::Error;
use yup_oauth2::AccessToken;
use serde::{Deserialize, Serialize};
use base64::{Engine as _, engine::general_purpose};

#[derive(Debug, Serialize, Deserialize)]
pub struct Email {
    pub id: String,
    pub subject: String,
    pub body: String,
    pub unsubscribe_link: Option<String>,
}

#[derive(Debug, Serialize, Deserialize)]
struct GmailMessage {
    id: String,
    payload: Payload,
}

#[derive(Debug, Serialize, Deserialize)]
struct Payload {
    headers: Vec<Header>,
    #[serde(default)]
    parts: Option<Vec<Part>>,
    body: Body,
}

#[derive(Debug, Serialize, Deserialize)]
struct Header {
    name: String,
    value: String,
}

#[derive(Debug, Serialize, Deserialize)]
struct Part {
    mimeType: String,
    body: Body,
}

#[derive(Debug, Serialize, Deserialize)]
struct Body {
    #[serde(default)]
    data: Option<String>,
}

pub struct GmailClient {
    client: reqwest::Client,
    token: AccessToken,
}

impl GmailClient {
    pub fn new(token: AccessToken) -> Self {
        Self {
            client: reqwest::Client::new(),
            token,
        }
    }

    pub async fn fetch_emails(&self) -> Result<Vec<Email>, Box<dyn Error>> {
        let url = "https://www.googleapis.com/gmail/v1/users/me/messages?q=is:unread&maxResults=10";
        let response: serde_json::Value = self.client.get(url)
            .bearer_auth(self.token.token().ok_or("No token available")?)
            .send()
            .await?
            .json()
            .await?;

        let messages = response["messages"].as_array()
            .ok_or("No messages found")?;

        let mut emails = Vec::new();

        for message in messages {
            let id = message["id"].as_str().ok_or("No id found")?;
            let email = self.fetch_email(id).await?;
            emails.push(email);
        }

        Ok(emails)
    }

    async fn fetch_email(&self, id: &str) -> Result<Email, Box<dyn Error>> {
        let url = format!("https://www.googleapis.com/gmail/v1/users/me/messages/{}", id);
        let response: GmailMessage = self.client.get(&url)
            .bearer_auth(self.token.token().ok_or("No token available")?)
            .send()
            .await?
            .json()
            .await?;

        self.parse_message(response)
    }

    fn parse_message(&self, msg: GmailMessage) -> Result<Email, Box<dyn Error>> {
        let subject = msg.payload.headers.iter()
            .find(|h| h.name == "Subject")
            .map(|h| h.value.clone())
            .unwrap_or_default();

        let unsubscribe_link = msg.payload.headers.iter()
            .find(|h| h.name == "List-Unsubscribe")
            .and_then(|h| Some(h.value.clone()))  // Wrap in Some
            .and_then(|v| {
                // Extract URL from <> if present
                if v.starts_with('<') && v.ends_with('>') {
                    Some(v[1..v.len()-1].to_string())
                } else {
                    // If no <>, return the whole string
                    Some(v)
                }
            });

        let body = self.extract_body(&msg.payload)?;

        Ok(Email {
            id: msg.id,
            subject,
            body,
            unsubscribe_link,
        })
    }

    
    fn extract_body(&self, payload: &Payload) -> Result<String, Box<dyn Error>> {
        if let Some(body) = &payload.body.data {
            return self.decode_body(body);
        }
    
        if let Some(parts) = &payload.parts {
            for part in parts {
                if part.mimeType.starts_with("text/") {
                    if let Some(data) = &part.body.data {
                        return self.decode_body(data);
                    }
                }
            }
        }
    
        Ok("No readable content found in the email.".to_string())
    }
    
    fn decode_body(&self, encoded_body: &str) -> Result<String, Box<dyn Error>> {
        let decoded = general_purpose::STANDARD.decode(encoded_body.replace('-', "+").replace('_', "/"))?;
        let body = String::from_utf8(decoded)
            .map_err(|e| Box::new(e) as Box<dyn Error>)?;
        Ok(body)
    }

    pub async fn mark_as_read(&self, email_id: &str) -> Result<(), Box<dyn Error>> {
        let url = format!("https://www.googleapis.com/gmail/v1/users/me/messages/{}/modify", email_id);
        let body = serde_json::json!({
            "removeLabelIds": ["UNREAD"]
        });

        self.client.post(&url)
            .bearer_auth(self.token.token().ok_or("No token available")?)
            .json(&body)
            .send()
            .await?;

        Ok(())
    }
}