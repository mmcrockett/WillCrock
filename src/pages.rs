use crate::auth::User;
use crate::config::Quackr;
use crate::sms::{self, Sms};

pub fn esc(s: &str) -> String {
    s.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
        .replace('\'', "&#39;")
}

fn asset(path: &str) -> String {
    format!("{path}?v={VERSION}")
}

fn shell(title: &str, body: &str) -> String {
    format!(
        "<!doctype html><html lang=\"en\"><head><meta charset=\"utf-8\">\
         <meta name=\"viewport\" content=\"width=device-width,initial-scale=1\">\
         <title>{} - WillCrock</title>\
         <link rel=\"icon\" href=\"/favicon.ico\" sizes=\"48x48\">\
         <link rel=\"icon\" href=\"/favicon.svg\" type=\"image/svg+xml\">\
         <link rel=\"apple-touch-icon\" href=\"/apple-touch-icon.png\">\
         <link rel=\"stylesheet\" href=\"/static/bootstrap.min.css\">\
         <link rel=\"stylesheet\" href=\"{css}\"></head>\
         <body>{}</body></html>",
        esc(title),
        body,
        css = asset("/static/site.css"),
    )
}

fn card_page(title: &str, body: &str) -> String {
    shell(
        title,
        &format!(
            "<main class=\"container min-vh-100 d-flex flex-column justify-content-center py-4\" style=\"max-width:26rem\">\
             <img src=\"{crest}\" alt=\"WillCrock crest\" width=\"132\" class=\"d-block mx-auto mb-3\">\
             <div class=\"card shadow-sm\"><div class=\"card-body p-4\">\
             <h1 class=\"h4 mb-3\">{}</h1>{}</div></div></main>",
            esc(title),
            body,
            crest = asset("/static/crest.png"),
        ),
    )
}

const VERSION: &str = match option_env!("WILLCROCK_VERSION") {
    Some(v) => v,
    None => "dev",
};

const ICON_HOME: &str =
    "<path d=\"M3 10.5 12 3l9 7.5\"/><path d=\"M5 9.5V21h14V9.5\"/><path d=\"M10 21v-6h4v6\"/>";
const ICON_USERS: &str = "<path d=\"M16 21v-2a4 4 0 0 0-4-4H6a4 4 0 0 0-4 4v2\"/>\
    <circle cx=\"9\" cy=\"7\" r=\"4\"/><path d=\"M22 21v-2a4 4 0 0 0-3-3.9\"/>\
    <path d=\"M16 3.1a4 4 0 0 1 0 7.8\"/>";
const ICON_LOGOUT: &str =
    "<path d=\"M9 21H5a2 2 0 0 1-2-2V5a2 2 0 0 1 2-2h4\"/><path d=\"m16 17 5-5-5-5\"/><path d=\"M21 12H9\"/>";
const ICON_SMS: &str =
    "<path d=\"M21 15a2 2 0 0 1-2 2H7l-4 4V5a2 2 0 0 1 2-2h14a2 2 0 0 1 2 2z\"/>";
const ICON_MENU: &str = "<path d=\"M4 6h16M4 12h16M4 18h16\"/>";

fn icon(paths: &str) -> String {
    format!(
        "<svg class=\"icon\" viewBox=\"0 0 24 24\" fill=\"none\" stroke=\"currentColor\" \
         stroke-width=\"2\" stroke-linecap=\"round\" stroke-linejoin=\"round\" aria-hidden=\"true\">{paths}</svg>"
    )
}

const COMMIT: Option<&str> = option_env!("WILLCROCK_COMMIT");
const BUILT_AT: Option<&str> = option_env!("WILLCROCK_BUILT_AT");

fn build_details(commit: Option<&str>, built_at: Option<&str>) -> String {
    match (commit, built_at) {
        (Some(c), Some(t)) => format!("Commit {c}, built {t}"),
        (Some(c), None) => format!("Commit {c}"),
        (None, Some(t)) => format!("Built {t}"),
        (None, None) => "Local build".to_string(),
    }
}

fn version_label() -> String {
    if !VERSION.is_empty() && VERSION.bytes().all(|b| b.is_ascii_digit()) {
        format!("v{VERSION}")
    } else {
        VERSION.to_string()
    }
}

fn side_link(href: &str, label: &str, paths: &str, is_active: bool) -> String {
    format!(
        "<a class=\"side-link{}\" href=\"{href}\">{}{label}</a>",
        if is_active { " active" } else { "" },
        icon(paths)
    )
}

fn app_page(title: &str, active: &str, user: &User, has_sms: bool, body: &str) -> String {
    let who = user.name.as_deref().unwrap_or(&user.email);
    let initial: String = who
        .chars()
        .next()
        .map(|c| c.to_uppercase().collect())
        .unwrap_or_default();
    let mut links = side_link("/", "Home", ICON_HOME, active == "home");
    if user.is_admin {
        links.push_str(&side_link(
            "/admin/users",
            "Users",
            ICON_USERS,
            active == "users",
        ));
        if has_sms {
            links.push_str(&side_link("/sms", "SMS", ICON_SMS, active == "sms"));
        }
    }
    shell(
        title,
        &format!(
            "<input type=\"checkbox\" id=\"nav-toggle\" class=\"nav-toggle\" aria-hidden=\"true\">\
             <div class=\"app\">\
             <header class=\"topbar\">\
             <label for=\"nav-toggle\" class=\"topbar-btn\" aria-label=\"Menu\">{menu}</label>\
             <img src=\"{mark}\" alt=\"\" width=\"32\" height=\"32\">\
             <span class=\"brand-word\">WillCrock</span></header>\
             <label for=\"nav-toggle\" class=\"backdrop\"></label>\
             <aside class=\"sidebar\">\
             <a class=\"brand\" href=\"/\"><img src=\"{mark}\" alt=\"\" width=\"48\" height=\"48\">\
             <span class=\"brand-word\">WillCrock</span></a>\
             <nav class=\"side-nav\">{links}</nav>\
             <div class=\"side-foot\"><div class=\"version\" title=\"{details}\">{version}</div>\
             <div class=\"user\"><span class=\"avatar\">{initial}</span>\
             <div class=\"user-text\"><div class=\"user-name\">{who}</div>\
             <div class=\"user-email\">{email}</div></div></div>\
             <form method=\"post\" action=\"/logout\">\
             <button class=\"signout\">{logout}Sign out</button></form></div>\
             </aside>\
             <main class=\"content\">{body}</main></div>",
            menu = icon(ICON_MENU),
            mark = asset("/favicon.svg"),
            version = esc(&version_label()),
            details = esc(&build_details(COMMIT, BUILT_AT)),
            initial = esc(&initial),
            who = esc(who),
            email = esc(&user.email),
            logout = icon(ICON_LOGOUT),
        ),
    )
}

fn page_title(title: &str) -> String {
    format!("<h1 class=\"page-title\">{}</h1>", esc(title))
}

fn tile(href: &str, label: &str, paths: &str) -> String {
    format!(
        "<a class=\"tile\" href=\"{href}\"><span class=\"tile-box\">{}</span>{label}</a>",
        icon(paths)
    )
}

pub fn login(msg: Option<&str>) -> String {
    card_page(
        "Sign in",
        &format!(
            "{}<form method=\"post\" action=\"/login\">\
             <div class=\"mb-3\"><label class=\"form-label\" for=\"email\">Email</label>\
             <input class=\"form-control form-control-lg\" id=\"email\" type=\"email\" name=\"email\" \
             autocomplete=\"email\" required autofocus></div>\
             <button class=\"btn btn-primary btn-lg w-100\">Send code</button></form>",
            msg.map(|m| format!("<div class=\"alert alert-info\">{}</div>", esc(m)))
                .unwrap_or_default()
        ),
    )
}

pub fn code(email: &str, error: bool) -> String {
    card_page(
        "Enter code",
        &format!(
            "{}<p class=\"text-body-secondary\">If {} is on the list, a code is on its way.</p>\
             <form method=\"post\" action=\"/login/code\">\
             <input type=\"hidden\" name=\"email\" value=\"{}\">\
             <div class=\"mb-3\"><label class=\"form-label\" for=\"code\">Code</label>\
             <input class=\"form-control form-control-lg text-center font-monospace\" id=\"code\" name=\"code\" \
             required autofocus autocomplete=\"one-time-code\" autocapitalize=\"characters\" \
             maxlength=\"12\" placeholder=\"XXXX-XXXX\"></div>\
             <button class=\"btn btn-primary btn-lg w-100\">Sign in</button></form>\
             <p class=\"text-center mt-3 mb-0\"><a href=\"/login\">Start over</a></p>",
            if error {
                "<div class=\"alert alert-danger\">That code didn't work.</div>"
            } else {
                ""
            },
            esc(email),
            esc(email)
        ),
    )
}

pub fn home(user: &User, has_sms: bool) -> String {
    let mut tiles = String::new();
    if user.is_admin {
        tiles.push_str(&tile("/admin/users", "Users", ICON_USERS));
        if has_sms {
            tiles.push_str(&tile("/sms", "SMS", ICON_SMS));
        }
    }
    app_page(
        "Home",
        "home",
        user,
        has_sms,
        &format!(
            "<section class=\"hero\"><h1 class=\"hero-word\">WillCrock</h1>\
             <img src=\"{crest}\" alt=\"WillCrock crest: ID IAM SCIO\" class=\"hero-crest\"></section>\
             <div class=\"tile-grid\">{tiles}</div>",
            crest = asset("/static/crest.png"),
        ),
    )
}

pub fn admin_users(users: &[User], me: &User, has_sms: bool) -> String {
    let rows: String = users
        .iter()
        .map(|u| {
            let actions = if u.id == me.id {
                "<span class=\"text-body-secondary\">you</span>".to_string()
            } else {
                let (state_path, state_label) = if u.disabled {
                    ("enable", "Enable")
                } else {
                    ("disable", "Disable")
                };
                format!(
                    "<form method=\"post\" action=\"/admin/users/{id}/{state_path}\" class=\"d-inline\">\
                     <button class=\"btn btn-sm btn-outline-secondary\">{state_label}</button></form>",
                    id = u.id
                )
            };
            format!(
                "<tr><td>{}<div class=\"text-body-secondary small\">{}</div></td>\
                 <td>{}{}</td><td class=\"text-end text-nowrap\">{}</td></tr>",
                esc(&u.email),
                esc(u.name.as_deref().unwrap_or("")),
                if u.is_admin {
                    "<span class=\"badge text-bg-primary\">admin</span> "
                } else {
                    ""
                },
                if u.disabled {
                    "<span class=\"badge text-bg-secondary\">disabled</span>"
                } else {
                    ""
                },
                actions
            )
        })
        .collect();
    app_page(
        "Users",
        "users",
        me,
        has_sms,
        &format!(
            "{}<div class=\"card shadow-sm mb-4\"><div class=\"table-responsive\">\
             <table class=\"table table-hover align-middle mb-0\"><tbody>{rows}</tbody></table>\
             </div></div>\
             <div class=\"card shadow-sm\"><div class=\"card-body\">\
             <h2 class=\"h5 mb-3\">Add user</h2>\
             <form method=\"post\" action=\"/admin/users\" class=\"row g-2 align-items-center\">\
             <div class=\"col-12 col-md\"><input class=\"form-control\" type=\"email\" name=\"email\" placeholder=\"Email\" required></div>\
             <div class=\"col-12 col-md\"><input class=\"form-control\" name=\"name\" placeholder=\"Name\"></div>\
             <div class=\"col-auto\"><button class=\"btn btn-primary\">Add</button></div>\
             </form></div></div>",
            page_title("Users")
        ),
    )
}

pub fn sms(user: &User, quackr: &Quackr, messages: Option<&[Sms]>) -> String {
    let notice = |text: &str| {
        format!("<div class=\"card shadow-sm\"><div class=\"card-body\">{text}</div></div>")
    };
    let cards = match messages {
        None => notice("Could not load messages."),
        Some([]) => notice("No messages."),
        Some(list) => list.iter().map(sms_card).collect(),
    };
    app_page(
        "SMS",
        "sms",
        user,
        true,
        &format!(
            "<div class=\"d-flex align-items-baseline justify-content-between gap-3\">{title}\
             <a href=\"/sms\">Refresh</a></div>\
             <p><button type=\"button\" class=\"btn btn-link p-0\" data-copy=\"{raw}\">{shown}</button></p>\
             {cards}\
             <script src=\"{js}\" defer></script>",
            title = page_title("SMS"),
            raw = esc(&quackr.phone_number),
            shown = esc(&sms::format_phone(&quackr.phone_number)),
            js = asset("/static/copy.js"),
        ),
    )
}

fn sms_card(m: &Sms) -> String {
    let code = sms::extract_code(&m.text)
        .map(|c| {
            format!(
                "<button type=\"button\" class=\"btn btn-primary btn-lg w-100 mt-2 sms-code\" \
                 data-copy=\"{}\">{}</button>",
                esc(&c.chars().filter(char::is_ascii_digit).collect::<String>()),
                esc(&c)
            )
        })
        .unwrap_or_default();
    format!(
        "<div class=\"card shadow-sm mb-3\"><div class=\"card-body\">\
         <div class=\"d-flex justify-content-between gap-2 small text-body-secondary mb-1\">\
         <span>{}</span><span>{}</span></div>\
         <div class=\"sms-body\">{}</div>{code}</div></div>",
        esc(&m.sender),
        esc(&m.time),
        esc(&m.text),
    )
}

pub fn not_found() -> String {
    card_page("Not found", "<a href=\"/\">Home</a>")
}

pub fn error() -> String {
    card_page(
        "Something went wrong",
        "<p class=\"mb-0\">Try again in a moment.</p>",
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn user(id: i64, name: &str, is_admin: bool) -> User {
        User {
            id,
            email: format!("{name}@example.com"),
            name: Some(name.to_string()),
            is_admin,
            disabled: false,
        }
    }

    #[test]
    fn home_marks_home_active_and_shows_footer() {
        let html = home(&user(1, "mike", false), false);
        assert!(html.contains("side-link active\" href=\"/\""));
        assert!(html.contains("class=\"version\""));
        assert!(html.contains("action=\"/logout\""));
        assert!(html.contains("mike@example.com"));
    }

    #[test]
    fn users_link_is_admin_only() {
        assert!(!home(&user(1, "sam", false), false).contains("/admin/users"));
        assert!(home(&user(1, "mike", true), false).contains("href=\"/admin/users\""));
    }

    #[test]
    fn admin_page_marks_users_active() {
        let me = user(1, "mike", true);
        let html = admin_users(std::slice::from_ref(&me), &me, false);
        assert!(html.contains("side-link active\" href=\"/admin/users\""));
    }

    #[test]
    fn home_is_a_crest_hero_with_admin_tile() {
        let member = home(&user(1, "sam", false), false);
        assert!(member.contains("class=\"hero-crest\""));
        assert!(!member.contains("Hello"));
        assert!(!member.contains("class=\"tile\""));
        assert!(
            home(&user(2, "mike", true), false).contains("class=\"tile\" href=\"/admin/users\"")
        );
    }

    #[test]
    fn changeable_assets_are_versioned() {
        let signed_in = home(&user(1, "sam", false), false);
        let signed_out = login(None);
        for html in [&signed_in, &signed_out] {
            assert!(html.contains("/static/site.css?v="));
            assert!(html.contains("/static/crest.png?v="));
        }
        assert!(signed_in.contains("/favicon.svg?v="));
    }

    #[test]
    fn build_details_cover_every_combination() {
        let t = "Oct 28, 2026 3:05 PM CDT";
        assert_eq!(
            build_details(Some("abc1234"), Some(t)),
            "Commit abc1234, built Oct 28, 2026 3:05 PM CDT"
        );
        assert_eq!(build_details(Some("abc1234"), None), "Commit abc1234");
        assert_eq!(
            build_details(None, Some(t)),
            "Built Oct 28, 2026 3:05 PM CDT"
        );
        assert_eq!(build_details(None, None), "Local build");
    }

    #[test]
    fn version_label_has_a_tooltip() {
        assert!(home(&user(1, "sam", false), false).contains("class=\"version\" title=\""));
    }

    fn quackr() -> Quackr {
        Quackr {
            phone_number: "15551234567".into(),
            api_key: "secret".into(),
        }
    }

    fn msg(text: &str) -> Sms {
        Sms {
            sender: "Bank".into(),
            time: "Oct 1".into(),
            text: text.into(),
        }
    }

    #[test]
    fn sms_link_needs_admin_and_config() {
        let admin = user(1, "mike", true);
        assert!(!home(&admin, false).contains("/sms"));
        assert!(!home(&user(2, "sam", false), true).contains("/sms"));
        let html = home(&admin, true);
        assert!(html.contains("side-link\" href=\"/sms\""));
        assert!(html.contains("class=\"tile\" href=\"/sms\""));
    }

    #[test]
    fn sms_page_marks_sms_active_and_shows_number() {
        let html = sms(&user(1, "mike", true), &quackr(), Some(&[]));
        assert!(html.contains("side-link active\" href=\"/sms\""));
        assert!(html.contains("(555) 123-4567"));
        assert!(html.contains("data-copy=\"15551234567\""));
        assert!(html.contains("/static/copy.js?v="));
        assert!(html.contains("No messages."));
        assert!(!html.contains("secret"));
    }

    #[test]
    fn sms_page_shows_error_state() {
        let html = sms(&user(1, "mike", true), &quackr(), None);
        assert!(html.contains("Could not load messages."));
    }

    #[test]
    fn sms_card_has_code_button_only_with_a_code() {
        let list = [msg("Your code is 123-456"), msg("see you soon")];
        let html = sms(&user(1, "mike", true), &quackr(), Some(&list));
        assert_eq!(html.matches("sms-code").count(), 1);
        assert!(html.contains("data-copy=\"123456\">123-456</button>"));
    }

    #[test]
    fn sms_text_is_escaped() {
        let list = [msg("<script>alert(1)</script>")];
        let html = sms(&user(1, "mike", true), &quackr(), Some(&list));
        assert!(!html.contains("<script>alert"));
        assert!(html.contains("&lt;script&gt;alert(1)"));
    }

    #[test]
    fn user_text_is_escaped() {
        let html = home(&user(1, "<b>x</b>", false), false);
        assert!(!html.contains("<b>x</b>"));
        assert!(html.contains("&lt;b&gt;x&lt;/b&gt;"));
    }
}
