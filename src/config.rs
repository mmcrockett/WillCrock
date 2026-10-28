use serde::Deserialize;

#[derive(Deserialize)]
pub struct Config {
    pub db_path: String,
    pub smtp: Smtp,
}

#[derive(Deserialize)]
pub struct Smtp {
    pub host: String,
    pub username: String,
    pub password: String,
    pub from: String,
}

pub fn load() -> Result<Config, String> {
    let path = match std::env::var("WILLCROCK_CONFIG") {
        Ok(p) => p,
        Err(_) => {
            let home = std::env::var("HOME").map_err(|_| "HOME not set".to_string())?;
            format!("{home}/config/willcrock.com/config.toml")
        }
    };
    let text = std::fs::read_to_string(&path).map_err(|e| format!("read {path}: {e}"))?;
    toml::from_str(&text).map_err(|e| format!("parse {path}: {e}"))
}
