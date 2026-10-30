use std::collections::HashMap;
use std::io::{Read, Write};

const MAX_BODY: u64 = 64 * 1024;

pub struct Req {
    pub method: String,
    pub path: String,
    pub query: HashMap<String, String>,
    pub form: HashMap<String, String>,
    cookies: String,
}

impl Req {
    pub fn from_fastcgi(req: &mut fastcgi::Request) -> Req {
        let method = req.param("REQUEST_METHOD").unwrap_or_default();
        let uri = req.param("REQUEST_URI").unwrap_or_default();
        let (path, uri_query) = split_uri(&uri);
        let query = req.param("QUERY_STRING").unwrap_or(uri_query);
        let mut body = Vec::new();
        let _ = req.stdin().take(MAX_BODY).read_to_end(&mut body);
        Req {
            method,
            path,
            query: parse_form(&query),
            form: parse_form(&String::from_utf8_lossy(&body)),
            cookies: req.param("HTTP_COOKIE").unwrap_or_default(),
        }
    }

    #[cfg(test)]
    pub fn test(method: &str, path: &str, form: &str) -> Req {
        Req {
            method: method.to_string(),
            path: path.to_string(),
            query: HashMap::new(),
            form: parse_form(form),
            cookies: String::new(),
        }
    }

    pub fn cookie(&self, name: &str) -> Option<&str> {
        self.cookies.split(';').find_map(|pair| {
            let (k, v) = pair.trim().split_once('=')?;
            (k == name).then_some(v)
        })
    }
}

pub struct Resp {
    status: u16,
    headers: Vec<(String, String)>,
    body: String,
}

impl Resp {
    pub fn html(status: u16, body: String) -> Resp {
        Resp {
            status,
            headers: vec![
                ("Content-Type".into(), "text/html; charset=utf-8".into()),
                ("Cache-Control".into(), "no-store".into()),
                ("X-Content-Type-Options".into(), "nosniff".into()),
            ],
            body,
        }
    }

    pub fn redirect(location: &str) -> Resp {
        Resp {
            status: 303,
            headers: vec![("Location".into(), location.into())],
            body: String::new(),
        }
    }

    pub fn header(mut self, name: &str, value: String) -> Resp {
        self.headers.push((name.into(), value));
        self
    }

    pub fn write_to(&self, out: &mut impl Write) -> std::io::Result<()> {
        write!(out, "Status: {}\r\n", self.status)?;
        for (k, v) in &self.headers {
            write!(out, "{k}: {v}\r\n")?;
        }
        write!(out, "\r\n{}", self.body)
    }
}

pub fn session_cookie(token: &str, max_age: i64) -> String {
    format!("session={token}; Max-Age={max_age}; Path=/; HttpOnly; Secure; SameSite=Lax")
}

fn split_uri(uri: &str) -> (String, String) {
    let (path, query) = uri.split_once('?').unwrap_or((uri, ""));
    (urldecode(path), query.to_string())
}

pub fn parse_form(s: &str) -> HashMap<String, String> {
    s.split('&')
        .filter(|p| !p.is_empty())
        .map(|p| {
            let (k, v) = p.split_once('=').unwrap_or((p, ""));
            (urldecode(k), urldecode(v))
        })
        .collect()
}

pub fn urldecode(s: &str) -> String {
    let bytes = s.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        match bytes[i] {
            b'+' => out.push(b' '),
            b'%' if i + 2 < bytes.len() && hex_pair(&bytes[i + 1..i + 3]).is_some() => {
                out.push(hex_pair(&bytes[i + 1..i + 3]).unwrap());
                i += 2;
            }
            b => out.push(b),
        }
        i += 1;
    }
    String::from_utf8_lossy(&out).into_owned()
}

fn hex_pair(pair: &[u8]) -> Option<u8> {
    let s = std::str::from_utf8(pair).ok()?;
    u8::from_str_radix(s, 16).ok()
}

pub fn urlencode(s: &str) -> String {
    s.bytes()
        .map(|b| match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                (b as char).to_string()
            }
            _ => format!("%{b:02X}"),
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn decodes_forms() {
        let f = parse_form("email=a%40b.com&name=Mike+C&flag&bad=%zz&end=%4");
        assert_eq!(f["email"], "a@b.com");
        assert_eq!(f["name"], "Mike C");
        assert_eq!(f["flag"], "");
        assert_eq!(f["bad"], "%zz");
        assert_eq!(f["end"], "%4");
    }

    #[test]
    fn encode_round_trip() {
        let s = "a+b@c.com & more/%";
        assert_eq!(urldecode(&urlencode(s)), s);
    }

    #[test]
    fn splits_uri() {
        assert_eq!(
            split_uri("/login/code?email=x"),
            ("/login/code".to_string(), "email=x".to_string())
        );
        assert_eq!(split_uri("/").0, "/");
    }

    #[test]
    fn finds_cookie() {
        let r = Req {
            method: "GET".into(),
            path: "/".into(),
            query: HashMap::new(),
            form: HashMap::new(),
            cookies: "a=1; session=tok; b=2".into(),
        };
        assert_eq!(r.cookie("session"), Some("tok"));
        assert_eq!(r.cookie("missing"), None);
    }
}
