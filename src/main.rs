use std::io::Write;

fn main() {
    fastcgi::run(|mut req| {
        let _ = write!(
            req.stdout(),
            "Content-Type: text/html; charset=utf-8\r\n\r\n<h1>hello world</h1>"
        );
    });
}
