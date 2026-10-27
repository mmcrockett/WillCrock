use crate::auth::display_code;
use crate::config::Smtp;
use lettre::transport::smtp::authentication::Credentials;
use lettre::{Message, SmtpTransport, Transport};

pub fn send_code(smtp: &Smtp, to: &str, code: &str) -> Result<(), String> {
    let msg = Message::builder()
        .from(smtp.from.parse().map_err(|e| format!("from: {e}"))?)
        .to(to.parse().map_err(|e| format!("to: {e}"))?)
        .subject("Your WillCrock login code")
        .body(format!(
            "Your login code is {}\n\nIt expires in 10 minutes. If you didn't ask for it, ignore this email.\n",
            display_code(code)
        ))
        .map_err(|e| e.to_string())?;
    let mailer = SmtpTransport::relay(&smtp.host)
        .map_err(|e| e.to_string())?
        .credentials(Credentials::new(
            smtp.username.clone(),
            smtp.password.clone(),
        ))
        .build();
    mailer.send(&msg).map(|_| ()).map_err(|e| e.to_string())
}
