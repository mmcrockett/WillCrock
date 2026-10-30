use crate::auth::{Feature, User};
use crate::config::Quackr;
use crate::pickle::dashboard::{self, Dashboard};
use crate::sms::{self, Sms};

#[derive(Clone, Copy, Default)]
pub struct Features {
    pub sms: bool,
    pub pickle: bool,
}

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
const ICON_PADDLE: &str = "<g transform=\"rotate(45 12 12)\">\
    <rect x=\"6.5\" y=\"1.5\" width=\"11\" height=\"13\" rx=\"5.5\"/>\
    <path d=\"M10.5 14.5V21a1 1 0 0 0 1 1h1a1 1 0 0 0 1-1v-6.5\"/>\
    <circle cx=\"10\" cy=\"6\" r=\"0.6\"/><circle cx=\"14\" cy=\"6\" r=\"0.6\"/>\
    <circle cx=\"12\" cy=\"9\" r=\"0.6\"/></g>";
const ICON_RECIPES: &str =
    "<path d=\"M6 13.9A4 4 0 0 1 7 6a5 5 0 0 1 10 0 4 4 0 0 1 1 7.9V20H6z\"/>\
    <path d=\"M6 17h12\"/>";
const ICON_PHOTOS: &str = "<rect x=\"3\" y=\"3\" width=\"18\" height=\"18\" rx=\"2\"/>\
    <circle cx=\"9\" cy=\"9\" r=\"2\"/><path d=\"m21 15-3.1-3.1a2 2 0 0 0-2.8 0L6 21\"/>";
const ICON_TRIPS: &str = "<path d=\"M20 10c0 6-8 12-8 12S4 16 4 10a8 8 0 0 1 16 0z\"/>\
    <circle cx=\"12\" cy=\"10\" r=\"3\"/>";
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

const LOCKED_HINT: &str = "Ask an admin for access";

struct NavItem {
    key: &'static str,
    href: &'static str,
    label: &'static str,
    icon: &'static str,
    locked: bool,
}

fn nav_items(user: &User, features: Features) -> Vec<NavItem> {
    let mut items = Vec::new();
    if user.is_admin {
        items.push(NavItem {
            key: "users",
            href: "/admin/users",
            label: "Users",
            icon: ICON_USERS,
            locked: false,
        });
    }
    if features.sms {
        items.push(NavItem {
            key: "sms",
            href: "/sms",
            label: "SMS",
            icon: ICON_SMS,
            locked: !user.can(Feature::Sms),
        });
    }
    if features.pickle {
        items.push(NavItem {
            key: "pickle",
            href: "/pickle",
            label: "Pickleball",
            icon: ICON_PADDLE,
            locked: !user.can(Feature::Pickle),
        });
    }
    for (feature, key, href, icon) in [
        (Feature::Recipes, "recipes", "/recipes", ICON_RECIPES),
        (Feature::Photos, "photos", "/photos", ICON_PHOTOS),
        (Feature::Trips, "trips", "/trips", ICON_TRIPS),
    ] {
        items.push(NavItem {
            key,
            href,
            label: feature.label(),
            icon,
            locked: !user.can(feature),
        });
    }
    items.sort_by_key(|i| i.label.to_lowercase());
    items
}

fn side_link(href: &str, label: &str, paths: &str, is_active: bool) -> String {
    format!(
        "<a class=\"side-link{}\" href=\"{href}\">{}{label}</a>",
        if is_active { " active" } else { "" },
        icon(paths)
    )
}

fn side_item(item: &NavItem, active: &str) -> String {
    if item.locked {
        format!(
            "<span class=\"side-link locked\" aria-disabled=\"true\" title=\"{LOCKED_HINT}\">{}{}</span>",
            icon(item.icon),
            item.label
        )
    } else {
        side_link(item.href, item.label, item.icon, active == item.key)
    }
}

fn app_page(title: &str, active: &str, user: &User, features: Features, body: &str) -> String {
    let who = user.name.as_deref().unwrap_or(&user.email);
    let initial: String = who
        .chars()
        .next()
        .map(|c| c.to_uppercase().collect())
        .unwrap_or_default();
    let mut links = side_link("/", "Home", ICON_HOME, active == "home");
    for item in nav_items(user, features) {
        links.push_str(&side_item(&item, active));
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

fn tile(item: &NavItem) -> String {
    if item.locked {
        format!(
            "<span class=\"tile locked\" aria-disabled=\"true\" title=\"{LOCKED_HINT}\">\
             <span class=\"tile-box\">{}</span>{}</span>",
            icon(item.icon),
            item.label
        )
    } else {
        format!(
            "<a class=\"tile\" href=\"{}\"><span class=\"tile-box\">{}</span>{}</a>",
            item.href,
            icon(item.icon),
            item.label
        )
    }
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

pub fn home(user: &User, features: Features) -> String {
    let tiles: String = nav_items(user, features).iter().map(tile).collect();
    app_page(
        "Home",
        "home",
        user,
        features,
        &format!(
            "<section class=\"hero\"><h1 class=\"hero-word\">WillCrock</h1>\
             <img src=\"{crest}\" alt=\"WillCrock crest: ID IAM SCIO\" class=\"hero-crest\"></section>\
             <div class=\"tile-grid\">{tiles}</div>",
            crest = asset("/static/crest.png"),
        ),
    )
}

pub fn admin_users(users: &[User], me: &User, features: Features) -> String {
    let rows: String = users
        .iter()
        .map(|u| {
            let grants: String = if u.is_admin {
                String::new()
            } else {
                Feature::ALL
                    .into_iter()
                    .map(|f| {
                        let (verb, style) = if u.access.contains(&f) {
                            ("revoke", "btn-primary")
                        } else {
                            ("grant", "btn-outline-secondary")
                        };
                        format!(
                            "<form method=\"post\" action=\"/admin/users/{id}/{verb}/{feature}\" class=\"d-inline me-1\">\
                             <button class=\"btn btn-sm {style}\">{label}</button></form>",
                            id = u.id,
                            feature = f.as_str(),
                            label = f.label()
                        )
                    })
                    .collect()
            };
            let actions = if u.id == me.id {
                "<span class=\"text-body-secondary\">you</span>".to_string()
            } else {
                let (state_path, state_label) = if u.disabled {
                    ("enable", "Enable")
                } else {
                    ("disable", "Disable")
                };
                format!(
                    "{grants}<form method=\"post\" action=\"/admin/users/{id}/{state_path}\" class=\"d-inline\">\
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
        features,
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

pub fn sms(user: &User, features: Features, quackr: &Quackr, messages: Option<&[Sms]>) -> String {
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
        features,
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

fn pre(label: &str, body: &str) -> String {
    format!(
        "<div class=\"text-body-secondary\">{label}</div>\
         <pre class=\"border rounded p-2 mb-1 pickle-body\">{}</pre>",
        esc(&dashboard::pretty_body(body))
    )
}

fn request_html(r: &dashboard::Request) -> String {
    let ok = r.status.is_some_and(|s| s < 400);
    let mut meta = r.context.clone().unwrap_or_default();
    if r.attempt > 1 {
        meta.push_str(&format!(", attempt {}", r.attempt));
    }
    if let Some(ms) = r.duration_ms {
        meta.push_str(&format!(", {ms}ms"));
    }
    let mut html = format!(
        "<div class=\"ms-3 mb-3 small\"><div>{} <strong>{}</strong> {} \
         <span class=\"badge {}\">{}</span> <span class=\"text-body-secondary\">{}</span></div>",
        esc(&dashboard::local(&r.created_at, "%-I:%M:%S%P")),
        esc(&r.method),
        esc(if r.url.is_empty() { "/" } else { &r.url }),
        if ok {
            "text-bg-success"
        } else {
            "text-bg-danger"
        },
        r.status
            .map(|s| s.to_string())
            .unwrap_or_else(|| "no response".to_string()),
        esc(&meta),
    );
    if let Some(e) = r.error.as_deref().filter(|e| !e.is_empty()) {
        html.push_str(&format!("<pre class=\"text-danger mb-1\">{}</pre>", esc(e)));
    }
    if let Some(b) = r.request_body.as_deref().filter(|b| !b.is_empty()) {
        html.push_str(&pre("Request", b));
    }
    if let Some(b) = r.response_body.as_deref().filter(|b| !b.is_empty()) {
        html.push_str(&pre("Response", b));
    }
    html.push_str("</div>");
    html
}

fn run_html(run: &dashboard::Run) -> String {
    let badge = match run.status.as_str() {
        "ok" => "text-bg-success",
        "error" => "text-bg-danger",
        _ => "text-bg-secondary",
    };
    let n = run.requests.len();
    let count = if n > 1 {
        format!(" <span class=\"text-body-secondary small\">{n} requests</span>")
    } else {
        String::new()
    };
    let text_class = if run.status == "error" {
        "text-danger"
    } else {
        "text-body-secondary"
    };
    let mut body = String::new();
    for text in [run.summary.as_deref(), run.detail.as_deref()]
        .into_iter()
        .flatten()
        .filter(|t| !t.is_empty() && *t != run.command)
    {
        body.push_str(&format!(
            "<pre class=\"ms-3 small mb-1 pickle-body {text_class}\">{}</pre>",
            esc(&dashboard::pretty_message(text))
        ));
    }
    let requests: String = run.requests.iter().map(request_html).collect();
    format!(
        "<details class=\"mb-2\"><summary class=\"h6\">\
         <span class=\"badge {badge}\">{status}</span> {cmd} {when}{count}</summary>{body}{requests}</details>",
        when = esc(&dashboard::local(&run.started_at, "%a %-I:%M:%S%P")),
        status = esc(&run.status),
        cmd = esc(&run.command),
    )
}

fn booking_row(b: &dashboard::Booking, now: i64) -> String {
    let date = dashboard::local(&b.start_time, "%A - %b %d");
    let time = format!(
        "{} - {}",
        dashboard::local(&b.start_time, "%-I:%M%P"),
        dashboard::local(&b.end_time, "%-I:%M%P")
    );
    let cancel = if dashboard::is_future(&b.start_time, now) {
        format!(
            "<form method=\"post\" action=\"/pickle/bookings/{id}/cancel\" class=\"d-inline\" \
             onsubmit=\"return confirm(this.dataset.msg)\" data-msg=\"{msg}\">\
             <button class=\"btn btn-sm btn-outline-danger\">Cancel</button></form>",
            id = esc(&b.id),
            msg = esc(&format!("Are you sure you want to cancel {date}, {time}?")),
        )
    } else {
        String::new()
    };
    format!(
        "<tr><td>{}</td><td>{}</td><td>{}</td><td>{}</td><td class=\"text-end\">{}</td></tr>",
        esc(&date),
        esc(&time),
        esc(b.court.as_deref().unwrap_or("")),
        if b.checked_in { "&#9989;" } else { "&#10134;" },
        cancel
    )
}

pub fn pickle(user: &User, features: Features, data: &Dashboard, now: i64) -> String {
    let rows: String = data.bookings.iter().map(|b| booking_row(b, now)).collect();
    let refreshed = data
        .bookings
        .first()
        .map(|b| {
            format!(
                "Refreshed {}",
                dashboard::local(&b.refreshed_at, "%A %B %d %Y %-I:%M%P")
            )
        })
        .unwrap_or_else(|| "No upcoming bookings.".to_string());
    let history: String = if data.runs.is_empty() {
        "<p class=\"text-body-secondary small\">Nothing recorded.</p>".to_string()
    } else {
        data.runs.iter().map(run_html).collect()
    };
    app_page(
        "Pickleball",
        "pickle",
        user,
        features,
        &format!(
            "{}<div class=\"card shadow-sm mb-2\"><div class=\"table-responsive\">\
             <table class=\"table table-hover align-middle mb-0\">\
             <thead><tr><th>Date</th><th>Time</th><th>Court</th><th>Checked in?</th><th></th></tr></thead>\
             <tbody>{rows}</tbody></table></div></div>\
             <p class=\"text-body-secondary small\">{}</p>\
             <details class=\"pt-3\"><summary class=\"btn btn-sm btn-outline-secondary\">History</summary>\
             <h2 class=\"h6 pt-3\">Last {} days</h2>{history}</details>",
            page_title("Pickleball"),
            esc(&refreshed),
            dashboard::HISTORY_DAYS,
        ),
    )
}

pub fn pickle_cancel_failed(user: &User, features: Features, message: &str) -> String {
    app_page(
        "Pickleball",
        "pickle",
        user,
        features,
        &format!(
            "{}<div class=\"alert alert-danger\">Cancel did not go through.</div>\
             <pre class=\"border rounded p-2 pickle-body\">{}</pre>\
             <a href=\"/pickle\">Back to schedule</a>",
            page_title("Pickleball"),
            esc(message)
        ),
    )
}

pub fn placeholder(user: &User, features: Features, feature: Feature) -> String {
    app_page(
        feature.label(),
        feature.as_str(),
        user,
        features,
        &format!(
            "{}<div class=\"card shadow-sm\"><div class=\"card-body\">Coming soon.</div></div>",
            page_title(feature.label())
        ),
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
            access: vec![],
        }
    }

    fn member_with(access: Vec<Feature>) -> User {
        User {
            access,
            ..user(2, "sam", false)
        }
    }

    #[test]
    fn home_marks_home_active_and_shows_footer() {
        let html = home(&user(1, "mike", false), Features::default());
        assert!(html.contains("side-link active\" href=\"/\""));
        assert!(html.contains("class=\"version\""));
        assert!(html.contains("action=\"/logout\""));
        assert!(html.contains("mike@example.com"));
    }

    #[test]
    fn users_link_is_admin_only() {
        assert!(!home(&user(1, "sam", false), Features::default()).contains("/admin/users"));
        assert!(home(&user(1, "mike", true), Features::default()).contains("href=\"/admin/users\""));
    }

    #[test]
    fn admin_page_marks_users_active() {
        let me = user(1, "mike", true);
        let html = admin_users(std::slice::from_ref(&me), &me, Features::default());
        assert!(html.contains("side-link active\" href=\"/admin/users\""));
    }

    #[test]
    fn home_is_a_crest_hero_with_admin_tile() {
        let member = home(&user(1, "sam", false), Features::default());
        assert!(member.contains("class=\"hero-crest\""));
        assert!(!member.contains("Hello"));
        assert!(!member.contains("class=\"tile\""));
        assert!(home(&user(2, "mike", true), Features::default())
            .contains("class=\"tile\" href=\"/admin/users\""));
    }

    #[test]
    fn changeable_assets_are_versioned() {
        let signed_in = home(&user(1, "sam", false), Features::default());
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
        assert!(home(&user(1, "sam", false), Features::default())
            .contains("class=\"version\" title=\""));
    }

    fn sms_on() -> Features {
        Features {
            sms: true,
            ..Features::default()
        }
    }

    fn pickle_on() -> Features {
        Features {
            pickle: true,
            ..Features::default()
        }
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
    fn sms_link_needs_config() {
        let admin = user(1, "mike", true);
        let member = member_with(vec![Feature::Sms]);
        assert!(!home(&admin, Features::default()).contains("/sms"));
        assert!(!home(&member, Features::default()).contains("/sms"));
        for u in [&admin, &member] {
            let html = home(u, sms_on());
            assert!(html.contains("side-link\" href=\"/sms\""));
            assert!(html.contains("class=\"tile\" href=\"/sms\""));
        }
    }

    #[test]
    fn sms_link_needs_the_grant() {
        let html = home(&member_with(vec![Feature::Pickle]), sms_on());
        assert!(!html.contains("href=\"/sms\""));
        assert!(html.contains("side-link locked"));
        assert!(html.contains("tile locked"));
    }

    #[test]
    fn users_page_shows_grant_toggles_for_members_only() {
        let admin = user(1, "mike", true);
        let sam = member_with(vec![Feature::Sms]);
        let html = admin_users(&[user(1, "mike", true), sam], &admin, Features::default());
        assert!(html.contains("/admin/users/2/revoke/sms"));
        assert!(html.contains("/admin/users/2/grant/pickle"));
        assert!(!html.contains("/admin/users/1/grant"));
        assert!(!html.contains("/admin/users/1/revoke"));
    }

    #[test]
    fn sms_page_marks_sms_active_and_shows_number() {
        let html = sms(&user(1, "mike", true), sms_on(), &quackr(), Some(&[]));
        assert!(html.contains("side-link active\" href=\"/sms\""));
        assert!(html.contains("(555) 123-4567"));
        assert!(html.contains("data-copy=\"15551234567\""));
        assert!(html.contains("/static/copy.js?v="));
        assert!(html.contains("No messages."));
        assert!(!html.contains("secret"));
    }

    #[test]
    fn sms_page_shows_error_state() {
        let html = sms(&user(1, "mike", true), sms_on(), &quackr(), None);
        assert!(html.contains("Could not load messages."));
    }

    #[test]
    fn sms_card_has_code_button_only_with_a_code() {
        let list = [msg("Your code is 123-456"), msg("see you soon")];
        let html = sms(&user(1, "mike", true), sms_on(), &quackr(), Some(&list));
        assert_eq!(html.matches("sms-code").count(), 1);
        assert!(html.contains("data-copy=\"123456\">123-456</button>"));
    }

    #[test]
    fn sms_text_is_escaped() {
        let list = [msg("<script>alert(1)</script>")];
        let html = sms(&user(1, "mike", true), sms_on(), &quackr(), Some(&list));
        assert!(!html.contains("<script>alert"));
        assert!(html.contains("&lt;script&gt;alert(1)"));
    }

    #[test]
    fn user_text_is_escaped() {
        let html = home(&user(1, "<b>x</b>", false), Features::default());
        assert!(!html.contains("<b>x</b>"));
        assert!(html.contains("&lt;b&gt;x&lt;/b&gt;"));
    }

    fn booking(id: &str, start: &str, court: &str) -> dashboard::Booking {
        dashboard::Booking {
            id: id.to_string(),
            start_time: start.to_string(),
            end_time: "2099-01-01T16:30:00.000Z".to_string(),
            court: Some(court.to_string()),
            checked_in: false,
            refreshed_at: "2026-10-18T13:30:00.000Z".to_string(),
        }
    }

    #[test]
    fn pickle_link_needs_config_and_the_grant() {
        let admin = user(1, "mike", true);
        let member = member_with(vec![Feature::Pickle]);
        let data = Dashboard {
            bookings: vec![],
            runs: vec![],
        };
        for u in [&admin, &member] {
            assert!(!home(u, Features::default()).contains("/pickle"));
            let html = home(u, pickle_on());
            assert!(html.contains("side-link\" href=\"/pickle\""));
            assert!(html.contains("class=\"tile\" href=\"/pickle\""));
            assert!(
                pickle(u, pickle_on(), &data, 0).contains("side-link active\" href=\"/pickle\"")
            );
        }
    }

    #[test]
    fn pickle_link_hidden_without_the_grant() {
        let html = home(&member_with(vec![Feature::Sms]), pickle_on());
        assert!(!html.contains("href=\"/pickle\""));
        assert!(html.contains("side-link locked"));
        assert!(html.contains("tile locked"));
    }

    #[test]
    fn nav_is_alphabetical_after_home() {
        let both = Features {
            sms: true,
            pickle: true,
        };
        let html = home(&user(1, "mike", true), both);
        let at = |needle: &str| html.find(needle).unwrap();
        let order = [
            at("href=\"/\""),
            at("href=\"/pickle\""),
            at("href=\"/sms\""),
            at("href=\"/admin/users\""),
        ];
        assert!(order.windows(2).all(|w| w[0] < w[1]));
        let tiles = &html[at("tile-grid")..];
        assert!(tiles.find("/pickle").unwrap() < tiles.find("/sms").unwrap());
        assert!(tiles.find("/sms").unwrap() < tiles.find("/admin/users").unwrap());
    }

    #[test]
    fn pickle_cancel_form_only_for_future_bookings_and_asks_first() {
        let data = Dashboard {
            bookings: vec![
                booking("11", "2000-01-01T15:00:00.000Z", "5C"),
                booking("22", "2099-01-01T15:00:00.000Z", "6B"),
            ],
            runs: vec![],
        };
        let html = pickle(&user(2, "sam", false), pickle_on(), &data, 1_000_000_000);
        assert!(!html.contains("/pickle/bookings/11/cancel"));
        assert!(html.contains("/pickle/bookings/22/cancel"));
        assert!(html.contains("data-msg=\"Are you sure you want to cancel"));
        assert!(html.contains("return confirm(this.dataset.msg)"));
        assert!(html.contains("Nothing recorded."));
    }

    #[test]
    fn pickle_empty_state_and_refreshed_caption() {
        let empty = Dashboard {
            bookings: vec![],
            runs: vec![],
        };
        assert!(pickle(&user(1, "mike", true), pickle_on(), &empty, 0)
            .contains("No upcoming bookings."));
        let some = Dashboard {
            bookings: vec![booking("1", "2099-01-01T15:00:00.000Z", "6B")],
            runs: vec![],
        };
        assert!(pickle(&user(1, "mike", true), pickle_on(), &some, 0)
            .contains("Refreshed Sunday October 18 2026"));
    }

    #[test]
    fn pickle_text_is_escaped() {
        let data = Dashboard {
            bookings: vec![booking("1", "2099-01-01T15:00:00.000Z", "<i>x</i>")],
            runs: vec![dashboard::Run {
                started_at: "2026-10-18T13:30:00Z".to_string(),
                command: "sync".to_string(),
                status: "error".to_string(),
                summary: Some("<script>".to_string()),
                detail: None,
                requests: vec![dashboard::Request {
                    created_at: "2026-10-18T13:30:00Z".to_string(),
                    context: None,
                    method: "GET".to_string(),
                    url: "/a".to_string(),
                    attempt: 1,
                    status: Some(500),
                    duration_ms: None,
                    request_body: None,
                    response_body: Some("<b>504</b>".to_string()),
                    error: Some("<e>".to_string()),
                }],
            }],
        };
        let html = pickle(&user(1, "mike", true), pickle_on(), &data, 0);
        for raw in ["<i>x</i>", "<script>", "<b>504</b>", "<e>"] {
            assert!(!html.contains(raw), "{raw}");
        }
        assert!(html.contains("&lt;b&gt;504&lt;/b&gt;"));
        assert!(html.contains("text-bg-danger"));
    }

    #[test]
    fn pickle_history_is_collapsed_and_counts_only_multiple_requests() {
        let req = || dashboard::Request {
            created_at: "2026-10-18T13:30:00Z".to_string(),
            context: None,
            method: "GET".to_string(),
            url: "/a".to_string(),
            attempt: 1,
            status: Some(200),
            duration_ms: None,
            request_body: None,
            response_body: None,
            error: None,
        };
        let run = |n| dashboard::Run {
            started_at: "2026-10-18T13:30:00Z".to_string(),
            command: "sync".to_string(),
            status: "ok".to_string(),
            summary: None,
            detail: None,
            requests: (0..n).map(|_| req()).collect(),
        };
        let one = Dashboard {
            bookings: vec![],
            runs: vec![run(1)],
        };
        let html = pickle(&user(1, "mike", true), pickle_on(), &one, 0);
        assert!(html.contains(">History</summary>"));
        assert!(html.contains("Last 7 days"));
        assert!(!html.contains("1 request"));
        let many = Dashboard {
            bookings: vec![],
            runs: vec![run(6)],
        };
        assert!(pickle(&user(1, "mike", true), pickle_on(), &many, 0).contains("6 requests"));
    }

    #[test]
    fn pickle_cancel_failure_page_shows_the_message_escaped() {
        let html = pickle_cancel_failed(&user(1, "mike", true), pickle_on(), "<nope>");
        assert!(html.contains("Cancel did not go through."));
        assert!(html.contains("&lt;nope&gt;"));
        assert!(!html.contains("<nope>"));
    }

    #[test]
    fn placeholder_links_follow_the_grant() {
        let member = member_with(vec![Feature::Recipes]);
        let html = home(&member, Features::default());
        assert!(html.contains("href=\"/recipes\""));
        assert!(!html.contains("href=\"/photos\""));
        assert!(!html.contains("href=\"/trips\""));
        let admin = home(&user(1, "mike", true), Features::default());
        for path in ["/recipes", "/photos", "/trips"] {
            assert!(admin.contains(&format!("href=\"{path}\"")), "{path}");
        }
        let none = home(&user(2, "sam", false), Features::default());
        assert!(!none.contains("href=\"/recipes\""));
        assert!(none.contains("tile locked"));
    }

    #[test]
    fn placeholder_page_marks_its_link_active() {
        let html = placeholder(&user(1, "mike", true), Features::default(), Feature::Trips);
        assert!(html.contains("Coming soon."));
        assert!(html.contains("side-link active\" href=\"/trips\""));
    }
}
