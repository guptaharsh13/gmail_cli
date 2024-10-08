use std::error::Error;
use yup_oauth2::AccessToken;
use serde::{Deserialize, Serialize};
use base64::{Engine as _, engine::general_purpose};
use html2text::from_read;
use pulldown_cmark::{Parser, html};

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
    mimeType: Option<String>,
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
    parts: Option<Vec<Part>>,
}

#[derive(Debug, Serialize, Deserialize)]
struct Body {
    #[serde(default)]
    data: Option<String>,
    size: Option<i32>,
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

        let unsubscribe_link = self.extract_unsubscribe_link(&msg.payload.headers);

        let body = self.extract_body(&msg.payload)?;

        Ok(Email {
            id: msg.id,
            subject,
            body,
            unsubscribe_link,
        })
    }

    fn extract_unsubscribe_link(&self, headers: &[Header]) -> Option<String> {
        headers.iter()
            .find(|h| h.name == "List-Unsubscribe")
            .and_then(|h| {
                // First, try to find a URL in angle brackets
                let urls: Vec<&str> = h.value
                    .split(',')
                    .filter_map(|part| {
                        let trimmed = part.trim();
                        if trimmed.starts_with('<') && trimmed.ends_with('>') {
                            Some(&trimmed[1..trimmed.len()-1])
                        } else {
                            None
                        }
                    })
                    .collect();

                // Prioritize http/https URLs over mailto
                urls.iter()
                    .find(|&&url| url.starts_with("http"))
                    .or_else(|| urls.first())
                    .map(|&url| url.to_string())
            })
    }

    fn extract_body(&self, payload: &Payload) -> Result<String, Box<dyn Error>> {
        // First, try to get content from the main body
        if let Some(content) = self.get_content_from_body(&payload.body) {
            return Ok(content);
        }

        // If main body is empty, try to get content from parts
        if let Some(parts) = &payload.parts {
            return self.get_content_from_parts(parts);
        }

        Ok("No readable content found in the email.".to_string())
    }

    fn get_content_from_body(&self, body: &Body) -> Option<String> {
        body.data.as_ref().and_then(|data| self.decode_and_render_body(data).ok())
    }

    fn get_content_from_parts(&self, parts: &[Part]) -> Result<String, Box<dyn Error>> {
        let mut text_plain = String::new();
        let mut text_html = String::new();

        for part in parts {
            match part.mimeType.as_str() {
                "text/plain" => {
                    if let Some(content) = self.get_content_from_body(&part.body) {
                        text_plain.push_str(&content);
                    }
                }
                "text/html" => {
                    if let Some(content) = self.get_content_from_body(&part.body) {
                        text_html.push_str(&content);
                    }
                }
                _ => {
                    // For multipart types, recursively check their parts
                    if let Some(subparts) = &part.parts {
                        let content = self.get_content_from_parts(subparts)?;
                        if !content.is_empty() {
                            return Ok(content);
                        }
                    }
                }
            }
        }

        // Prefer HTML content if available, otherwise use plain text
        if !text_html.is_empty() {
            Ok(from_read(text_html.as_bytes(), 80))
        } else if !text_plain.is_empty() {
            Ok(text_plain)
        } else {
            Ok("No readable content found in the email.".to_string())
        }
    }

    fn decode_and_render_body(&self, encoded_body: &str) -> Result<String, Box<dyn Error>> {
        let decoded = general_purpose::STANDARD.decode(encoded_body.replace('-', "+").replace('_', "/"))?;
        let body = String::from_utf8(decoded)
            .map_err(|e| Box::new(e) as Box<dyn Error>)?;
        
        // Determine if the content is HTML or Markdown
        if body.contains("&lt;") || body.contains("&gt;") || body.contains("&amp;") {
            // Likely HTML content
            Ok(from_read(body.as_bytes(), 80))
        } else {
            // Likely Markdown content
            let parser = Parser::new(&body);
            let mut html_output = String::new();
            html::push_html(&mut html_output, parser);
            Ok(from_read(html_output.as_bytes(), 80))
        }
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