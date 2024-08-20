mod app;
mod gmail_api;
mod terminal_ui;

use std::error::Error;
use std::sync::Arc;
use tokio::sync::Mutex;

use app::App;
use terminal_ui::TerminalUI;

#[tokio::main]
async fn main() -> Result<(), Box<dyn Error>> {
    let app = Arc::new(Mutex::new(App::new().await?));
    let mut ui = TerminalUI::new(app);
    ui.run().await?;
    Ok(())
}