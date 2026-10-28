use serde::Deserialize;
use std::os::unix::fs::MetadataExt;

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
        Err(_) => format!("{}/config/willcrock.com/config.toml", home_dir()?),
    };
    let text = std::fs::read_to_string(&path).map_err(|e| format!("read {path}: {e}"))?;
    toml::from_str(&text).map_err(|e| format!("parse {path}: {e}"))
}

// Apache starts FastCGI processes without HOME, so fall back to the passwd entry.
pub fn home_dir() -> Result<String, String> {
    if let Ok(home) = std::env::var("HOME") {
        return Ok(home);
    }
    let uid = std::fs::metadata("/proc/self")
        .map_err(|e| format!("stat /proc/self: {e}"))?
        .uid();
    let passwd =
        std::fs::read_to_string("/etc/passwd").map_err(|e| format!("read /etc/passwd: {e}"))?;
    passwd_home(&passwd, uid).ok_or_else(|| format!("no passwd entry for uid {uid}"))
}

fn passwd_home(passwd: &str, uid: u32) -> Option<String> {
    passwd.lines().find_map(|line| {
        let fields: Vec<&str> = line.split(':').collect();
        let matches = fields.get(2).and_then(|f| f.parse::<u32>().ok()) == Some(uid);
        fields.get(5).filter(|_| matches).map(|h| h.to_string())
    })
}

#[cfg(test)]
mod tests {
    use super::passwd_home;

    #[test]
    fn finds_home_by_uid() {
        let passwd = "root:x:0:0:root:/root:/bin/bash\nwc:x:1001:1001::/home/wc:/bin/bash\n";
        assert_eq!(passwd_home(passwd, 1001).as_deref(), Some("/home/wc"));
        assert_eq!(passwd_home(passwd, 0).as_deref(), Some("/root"));
        assert_eq!(passwd_home(passwd, 42), None);
    }
}
