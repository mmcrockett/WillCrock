use crate::config::Quackr;
use serde_json::Value;
use std::time::Duration;

const TIME_FIELDS: [&str; 6] = [
    "date",
    "receivedAt",
    "received_at",
    "time",
    "timestamp",
    "created_at",
];

pub struct Sms {
    pub sender: String,
    pub time: String,
    pub text: String,
}

pub fn fetch(q: &Quackr) -> Result<Vec<Sms>, String> {
    let body: Value = ureq::get("https://api.quackr.io/receive-sms")
        .query("phoneNumber", &q.phone_number)
        .set("x-api-key", &q.api_key)
        .timeout(Duration::from_secs(10))
        .call()
        .map_err(|e| e.to_string())?
        .into_json()
        .map_err(|e| e.to_string())?;
    parse_messages(&body).ok_or_else(|| "response has no data.messages list".to_string())
}

fn parse_messages(body: &Value) -> Option<Vec<Sms>> {
    let list = body.get("data")?.get("messages")?.as_array()?;
    Some(
        list.iter()
            .map(|m| Sms {
                sender: str_field(m, "sender"),
                time: TIME_FIELDS
                    .iter()
                    .map(|f| str_field(m, f))
                    .find(|t| !t.is_empty())
                    .unwrap_or_default(),
                text: str_field(m, "message"),
            })
            .collect(),
    )
}

fn str_field(m: &Value, key: &str) -> String {
    match m.get(key) {
        Some(Value::String(s)) => s.clone(),
        Some(Value::Number(n)) => n.to_string(),
        _ => String::new(),
    }
}

pub fn format_phone(number: &str) -> String {
    let digits = match number.len() {
        10 => number,
        11 if number.starts_with('1') => &number[1..],
        _ => return number.to_string(),
    };
    if !digits.bytes().all(|b| b.is_ascii_digit()) {
        return number.to_string();
    }
    format!("({}) {}-{}", &digits[..3], &digits[3..6], &digits[6..])
}

fn is_word(b: u8) -> bool {
    b.is_ascii_alphanumeric() || b == b'_'
}

fn digits_at(b: &[u8], start: usize, len: usize) -> bool {
    b.get(start..start + len)
        .is_some_and(|s| s.iter().all(u8::is_ascii_digit))
}

fn ends_at_boundary(b: &[u8], end: usize) -> bool {
    b.get(end).is_none_or(|&c| !is_word(c))
}

// Leftmost match of `\b\d{3}[- ]\d{3}\b|\b\d{4,8}\b`.
pub fn extract_code(text: &str) -> Option<String> {
    let b = text.as_bytes();
    (0..b.len())
        .filter(|&i| b[i].is_ascii_digit() && (i == 0 || !is_word(b[i - 1])))
        .find_map(|i| {
            if digits_at(b, i, 3)
                && matches!(b.get(i + 3), Some(b'-' | b' '))
                && digits_at(b, i + 4, 3)
                && ends_at_boundary(b, i + 7)
            {
                return Some(text[i..i + 7].to_string());
            }
            let len = b[i..].iter().take_while(|c| c.is_ascii_digit()).count();
            ((4..=8).contains(&len) && ends_at_boundary(b, i + len))
                .then(|| text[i..i + len].to_string())
        })
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn formats_us_numbers() {
        assert_eq!(format_phone("5551234567"), "(555) 123-4567");
        assert_eq!(format_phone("15551234567"), "(555) 123-4567");
        assert_eq!(format_phone("25551234567"), "25551234567");
        assert_eq!(format_phone("+15551234567"), "+15551234567");
        assert_eq!(format_phone("555-123-4567"), "555-123-4567");
    }

    #[test]
    fn extracts_codes() {
        assert_eq!(
            extract_code("Your code is 482913.").as_deref(),
            Some("482913")
        );
        assert_eq!(
            extract_code("code 123-456 expires").as_deref(),
            Some("123-456")
        );
        assert_eq!(extract_code("code 123 456").as_deref(), Some("123 456"));
        assert_eq!(extract_code("use 1234").as_deref(), Some("1234"));
        assert_eq!(extract_code("12345678").as_deref(), Some("12345678"));
    }

    #[test]
    fn ignores_non_codes() {
        assert_eq!(extract_code("no digits here"), None);
        assert_eq!(extract_code("only 123 digits"), None);
        assert_eq!(extract_code("too long 123456789"), None);
        assert_eq!(extract_code("glued abc1234"), None);
        assert_eq!(extract_code("glued 1234abc"), None);
        assert_eq!(extract_code("snake_1234"), None);
    }

    #[test]
    fn first_code_wins() {
        assert_eq!(
            extract_code("12 then 5555 then 666-777").as_deref(),
            Some("5555")
        );
    }

    #[test]
    fn parses_messages_with_time_fallbacks() {
        let body = json!({"data": {"messages": [
            {"sender": "Bank", "message": "hi", "date": "Oct 1"},
            {"sender": "Shop", "message": "yo", "date": "", "received_at": 1700000000},
            {"message": "bare"},
        ]}});
        let msgs = parse_messages(&body).unwrap();
        assert_eq!(msgs.len(), 3);
        assert_eq!(
            (
                msgs[0].sender.as_str(),
                msgs[0].time.as_str(),
                msgs[0].text.as_str()
            ),
            ("Bank", "Oct 1", "hi")
        );
        assert_eq!(msgs[1].time, "1700000000");
        assert_eq!((msgs[2].sender.as_str(), msgs[2].time.as_str()), ("", ""));
    }

    #[test]
    fn rejects_unexpected_shapes() {
        assert!(parse_messages(&json!({})).is_none());
        assert!(parse_messages(&json!({"data": {}})).is_none());
        assert!(parse_messages(&json!({"data": {"messages": "x"}})).is_none());
        assert_eq!(
            parse_messages(&json!({"data": {"messages": []}}))
                .unwrap()
                .len(),
            0
        );
    }
}
