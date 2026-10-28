use std::io::Write;

pub fn error(msg: &str) {
    eprintln!("{msg}");
    let Ok(home) = crate::config::home_dir() else {
        return;
    };
    let dir = format!("{home}/logs/willcrock.com");
    let _ = std::fs::create_dir_all(&dir);
    let file = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(format!("{dir}/app.log"));
    if let Ok(mut f) = file {
        let _ = writeln!(f, "{} {msg}", crate::cli::now());
    }
}
