use std::collections::HashMap;
use std::io::{Read, Write};

const MAX_BODY: u64 = 64 * 1024;
pub const MAX_PHOTO_SIZE: usize = 25 * 1024 * 1024;
const MAX_PHOTO_BODY: u64 = MAX_PHOTO_SIZE as u64 + 64 * 1024;

fn is_image_upload(method: &str, path: &str) -> bool {
    (method == "POST" && path == "/photos/upload") || is_card_upload(method, path)
}

pub fn is_card_upload(method: &str, path: &str) -> bool {
    method == "POST"
        && (path == "/cards/upload"
            || path
                .strip_prefix("/cards/")
                .and_then(|path| path.strip_suffix("/back/upload"))
                .is_some_and(|id| {
                    !id.is_empty()
                        && id.bytes().all(|b| b.is_ascii_digit())
                        && id.parse::<i64>().is_ok_and(|id| id > 0)
                }))
}

fn boundary_byte(byte: u8) -> bool {
    byte.is_ascii_alphanumeric()
        || matches!(
            byte,
            b'\'' | b'(' | b')' | b'+' | b'_' | b',' | b'-' | b'.' | b'/' | b':' | b'=' | b'?'
        )
}

fn multipart_boundary(content_type: &str) -> Option<&str> {
    let mut parameters = content_type.split(';');
    if !parameters
        .next()?
        .trim()
        .eq_ignore_ascii_case("multipart/form-data")
    {
        return None;
    }

    let mut boundary = None;
    for parameter in parameters {
        let (name, value) = parameter.trim().split_once('=')?;
        if !name.trim().eq_ignore_ascii_case("boundary") {
            continue;
        }
        if boundary.is_some() {
            return None;
        }

        let value = value.trim();
        let value = if value.starts_with('"') || value.ends_with('"') {
            value.strip_prefix('"')?.strip_suffix('"')?
        } else {
            value
        };
        if value.is_empty() || value.len() > 70 || !value.bytes().all(boundary_byte) {
            return None;
        }
        boundary = Some(value);
    }
    boundary
}

fn split_header_parameters(value: &str) -> Option<Vec<&str>> {
    let mut parameters = Vec::new();
    let mut start = 0;
    let mut quoted = false;
    let mut escaped = false;
    for (index, byte) in value.bytes().enumerate() {
        if escaped {
            escaped = false;
            continue;
        }
        match byte {
            b'\\' if quoted => escaped = true,
            b'"' => quoted = !quoted,
            b';' if !quoted => {
                parameters.push(&value[start..index]);
                start = index + 1;
            }
            _ => {}
        }
    }
    if quoted || escaped {
        return None;
    }
    parameters.push(&value[start..]);
    Some(parameters)
}

fn is_photo_file_disposition(disposition: &str) -> bool {
    let Some(parameters) = split_header_parameters(disposition) else {
        return false;
    };
    let mut parameters = parameters.into_iter();
    if !parameters
        .next()
        .is_some_and(|value| value.trim().eq_ignore_ascii_case("form-data"))
    {
        return false;
    }

    let (mut photo_name, mut filename) = (false, false);
    for parameter in parameters {
        let Some((name, value)) = parameter.trim().split_once('=') else {
            continue;
        };
        let value = value.trim();
        if name.trim().eq_ignore_ascii_case("name") {
            if photo_name
                || value.strip_prefix('"').and_then(|v| v.strip_suffix('"')) != Some("photo")
            {
                return false;
            }
            photo_name = true;
        } else if name.trim().eq_ignore_ascii_case("filename") {
            if filename || value.is_empty() || value == "\"\"" {
                return false;
            }
            filename = true;
        }
    }
    photo_name && filename
}

pub fn multipart_upload_file<'a>(body: &'a [u8], content_type: &str) -> Option<&'a [u8]> {
    let boundary = multipart_boundary(content_type)?;
    let mut opening = format!("--{boundary}").into_bytes();
    opening.extend_from_slice(b"\r\n");
    if !body.starts_with(&opening) {
        return None;
    }

    let header_start = opening.len();
    let header_end = body[header_start..]
        .windows(4)
        .position(|window| window == b"\r\n\r\n")?
        + header_start;
    let headers = std::str::from_utf8(&body[header_start..header_end]).ok()?;
    let disposition = headers.lines().find_map(|header| {
        let (name, value) = header.split_once(':')?;
        name.eq_ignore_ascii_case("content-disposition")
            .then_some(value.trim())
    })?;
    if !is_photo_file_disposition(disposition) {
        return None;
    }

    let data_start = header_end + 4;
    let data = &body[data_start..];
    let delimiter = format!("\r\n--{boundary}");
    let mut offset = 0;
    loop {
        let marker = offset
            + data[offset..]
                .windows(delimiter.len())
                .position(|window| window == delimiter.as_bytes())?;
        let suffix_start = marker + delimiter.len();
        let suffix = &data[suffix_start..];
        if let Some(epilogue) = suffix.strip_prefix(b"--") {
            if epilogue.is_empty() || epilogue == b"\r\n" {
                return Some(&data[..marker]);
            }
            return None;
        }
        if suffix.starts_with(b"\r\n") {
            return None;
        }
        offset = suffix_start;
    }
}

fn read_body(reader: impl Read, limit: u64) -> (Vec<u8>, Option<u16>) {
    let mut body = Vec::new();
    let error = match reader.take(limit + 1).read_to_end(&mut body) {
        Err(_) => Some(400),
        Ok(_) if body.len() as u64 > limit => Some(413),
        Ok(_) => None,
    };
    (body, error)
}

pub struct Req {
    pub method: String,
    pub path: String,
    pub query: HashMap<String, String>,
    pub form: HashMap<String, String>,
    pub body: Vec<u8>,
    pub body_error: Option<u16>,
    pub content_type: String,
    pub requested_with: String,
    cookies: String,
}

impl Req {
    pub fn from_fastcgi(req: &mut fastcgi::Request) -> Req {
        let method = req.param("REQUEST_METHOD").unwrap_or_default();
        let uri = req.param("REQUEST_URI").unwrap_or_default();
        let (path, uri_query) = split_uri(&uri);
        let query = req.param("QUERY_STRING").unwrap_or(uri_query);
        let is_upload = is_image_upload(&method, &path);
        let limit = if is_upload { MAX_PHOTO_BODY } else { MAX_BODY };
        let (body, body_error) = read_body(req.stdin(), limit);
        let form = if is_upload || body_error.is_some() {
            HashMap::new()
        } else {
            parse_form(&String::from_utf8_lossy(&body))
        };
        Req {
            method,
            path,
            query: parse_form(&query),
            form,
            body: if is_upload { body } else { Vec::new() },
            body_error,
            content_type: req.param("CONTENT_TYPE").unwrap_or_default(),
            requested_with: req.param("HTTP_X_WILLCROCK_REQUEST").unwrap_or_default(),
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
            body: Vec::new(),
            body_error: None,
            content_type: String::new(),
            requested_with: String::new(),
            cookies: String::new(),
        }
    }

    #[cfg(test)]
    pub fn with_cookie(mut self, cookie: &str) -> Self {
        self.cookies = cookie.into();
        self
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
    body: Body,
}

enum Body {
    Text(String),
    File(std::fs::File),
}

impl Resp {
    pub fn json(status: u16, body: &impl serde::Serialize) -> Resp {
        Resp {
            status,
            headers: vec![
                (
                    "Content-Type".into(),
                    "application/json; charset=utf-8".into(),
                ),
                ("Cache-Control".into(), "no-store".into()),
                ("X-Content-Type-Options".into(), "nosniff".into()),
            ],
            body: Body::Text(serde_json::to_string(body).expect("serializing response")),
        }
    }

    pub fn html(status: u16, body: String) -> Resp {
        Resp {
            status,
            headers: vec![
                ("Content-Type".into(), "text/html; charset=utf-8".into()),
                ("Cache-Control".into(), "no-store".into()),
                ("X-Content-Type-Options".into(), "nosniff".into()),
            ],
            body: Body::Text(body),
        }
    }

    pub fn redirect(location: &str) -> Resp {
        Resp {
            status: 303,
            headers: vec![("Location".into(), location.into())],
            body: Body::Text(String::new()),
        }
    }

    pub fn file(file: std::fs::File, content_type: &str) -> std::io::Result<Resp> {
        let length = file.metadata()?.len();
        Ok(Resp {
            status: 200,
            headers: vec![
                ("Content-Type".into(), content_type.into()),
                ("Content-Length".into(), length.to_string()),
                ("Cache-Control".into(), "private, no-store".into()),
                ("X-Content-Type-Options".into(), "nosniff".into()),
            ],
            body: Body::File(file),
        })
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
        write!(out, "\r\n")?;
        match &self.body {
            Body::Text(body) => out.write_all(body.as_bytes()),
            Body::File(file) => std::io::copy(&mut &*file, out).map(|_| ()),
        }
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
    fn body_limit_reports_truncation() {
        assert_eq!(read_body(&b"abc"[..], 3), (b"abc".to_vec(), None));
        assert_eq!(read_body(&b"abcd"[..], 3).1, Some(413));
    }

    #[test]
    fn only_exact_image_upload_routes_get_the_large_body_allowance() {
        for path in ["/photos/upload", "/cards/upload", "/cards/1/back/upload"] {
            assert!(is_image_upload("POST", path));
            assert!(!is_image_upload("GET", path));
        }
        for path in [
            "/cards",
            "/cards/1/name",
            "/cards/upload/extra",
            "/cards/1/back/upload/extra",
            "/cards/1/back/upload/",
            "/cards/no/back/upload",
            "/cards//back/upload",
            "/cards/1//back/upload",
            "/cards/0/back/upload",
            "/cards/-1/back/upload",
            "/cards/9223372036854775808/back/upload",
            "/cards/1/back/original",
            "/other/upload",
        ] {
            assert!(!is_image_upload("POST", path));
        }
        let body = vec![0; MAX_BODY as usize + 1];
        assert_eq!(read_body(body.as_slice(), MAX_BODY).1, Some(413));
        assert_eq!(read_body(body.as_slice(), MAX_PHOTO_BODY).1, None);
    }

    #[test]
    fn multipart_upload_extracts_binary_photo() {
        let boundary = "photo-boundary";
        let image = [0xff, 0, b'a', b'\r', b'\n', 0x80];
        let mut body = format!(
            "--{boundary}\r\nContent-Disposition: form-data; name=\"photo\"; filename=\"holiday; name=beach.jpg\"\r\nContent-Type: image/jpeg\r\n\r\n"
        )
        .into_bytes();
        body.extend_from_slice(&image);
        body.extend_from_slice(format!("\r\n--{boundary}--\r\n").as_bytes());

        assert_eq!(
            multipart_upload_file(&body, &format!("multipart/form-data; boundary={boundary}")),
            Some(image.as_slice())
        );
    }

    #[test]
    fn multipart_upload_rejects_non_photo_and_bad_content_type() {
        let boundary = "photo-boundary";
        let body = format!(
            "--{boundary}\r\nContent-Disposition: form-data; name=\"other\"; filename=\"photo.jpg\"\r\n\r\nphoto\r\n--{boundary}--\r\n"
        );
        assert_eq!(
            multipart_upload_file(
                body.as_bytes(),
                &format!("multipart/form-data; boundary={boundary}")
            ),
            None
        );
        assert_eq!(
            multipart_upload_file(body.as_bytes(), "application/octet-stream"),
            None
        );

        let body = format!(
            "--{boundary}\r\nContent-Disposition: form-data; name=\"photo\"; filename=\"photo.jpg\"\r\n\r\nphoto\r\n--{boundary}\r\nContent-Disposition: form-data; name=\"extra\"\r\n\r\nvalue\r\n--{boundary}--\r\n"
        );
        assert_eq!(
            multipart_upload_file(
                body.as_bytes(),
                &format!("multipart/form-data; boundary={boundary}")
            ),
            None
        );

        let body = format!(
            "--{boundary}\r\nContent-Disposition: form-data; name=\"photo\"; filename=\"photo.jpg\"\r\n\r\nphoto\r\n--{boundary}--\r\nepilogue\r\n--{boundary}--\r\n"
        );
        assert_eq!(
            multipart_upload_file(
                body.as_bytes(),
                &format!("multipart/form-data; boundary={boundary}")
            ),
            None
        );
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
            body: Vec::new(),
            body_error: None,
            content_type: String::new(),
            requested_with: String::new(),
            cookies: "a=1; session=tok; b=2".into(),
        };
        assert_eq!(r.cookie("session"), Some("tok"));
        assert_eq!(r.cookie("missing"), None);
    }

    #[test]
    fn writes_binary_response_without_utf8_conversion() {
        let path =
            std::env::temp_dir().join(format!("willcrock-response-{}", rand::random::<u64>()));
        let bytes = [0xff, 0xd8, 0, 0x80, 0xff];
        std::fs::write(&path, bytes).unwrap();
        let file = std::fs::File::open(&path).unwrap();
        let mut out = Vec::new();
        Resp::file(file, "image/jpeg")
            .unwrap()
            .write_to(&mut out)
            .unwrap();
        let split = out.windows(4).position(|w| w == b"\r\n\r\n").unwrap();
        let headers = std::str::from_utf8(&out[..split]).unwrap();
        assert!(headers.contains("Content-Type: image/jpeg"));
        assert!(headers.contains("Content-Length: 5"));
        assert!(headers.contains("Cache-Control: private, no-store"));
        assert_eq!(&out[split + 4..], &bytes);
        std::fs::remove_file(path).unwrap();
    }
}
