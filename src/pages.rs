use crate::auth::User;

pub fn esc(s: &str) -> String {
    s.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
        .replace('\'', "&#39;")
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
         <link rel=\"stylesheet\" href=\"/static/site.css\"></head>\
         <body>{}</body></html>",
        esc(title),
        body
    )
}

fn card_page(title: &str, body: &str) -> String {
    shell(
        title,
        &format!(
            "<main class=\"container min-vh-100 d-flex flex-column justify-content-center py-4\" style=\"max-width:26rem\">\
             <img src=\"/static/crest.png\" alt=\"WillCrock crest\" width=\"132\" class=\"d-block mx-auto mb-3\">\
             <div class=\"card shadow-sm\"><div class=\"card-body p-4\">\
             <h1 class=\"h4 mb-3\">{}</h1>{}</div></div></main>",
            esc(title),
            body
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
const ICON_MENU: &str = "<path d=\"M4 6h16M4 12h16M4 18h16\"/>";

fn icon(paths: &str) -> String {
    format!(
        "<svg class=\"icon\" viewBox=\"0 0 24 24\" fill=\"none\" stroke=\"currentColor\" \
         stroke-width=\"2\" stroke-linecap=\"round\" stroke-linejoin=\"round\" aria-hidden=\"true\">{paths}</svg>"
    )
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

fn app_page(title: &str, active: &str, user: &User, body: &str) -> String {
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
    }
    shell(
        title,
        &format!(
            "<input type=\"checkbox\" id=\"nav-toggle\" class=\"nav-toggle\" aria-hidden=\"true\">\
             <div class=\"app\">\
             <header class=\"topbar\">\
             <label for=\"nav-toggle\" class=\"topbar-btn\" aria-label=\"Menu\">{menu}</label>\
             <img src=\"/favicon.svg\" alt=\"\" width=\"32\" height=\"32\">\
             <span class=\"brand-word\">WillCrock</span></header>\
             <label for=\"nav-toggle\" class=\"backdrop\"></label>\
             <aside class=\"sidebar\">\
             <a class=\"brand\" href=\"/\"><img src=\"/favicon.svg\" alt=\"\" width=\"48\" height=\"48\">\
             <span class=\"brand-word\">WillCrock</span></a>\
             <nav class=\"side-nav\">{links}</nav>\
             <div class=\"side-foot\"><div class=\"version\">{version}</div>\
             <div class=\"user\"><span class=\"avatar\">{initial}</span>\
             <div class=\"user-text\"><div class=\"user-name\">{who}</div>\
             <div class=\"user-email\">{email}</div></div></div>\
             <form method=\"post\" action=\"/logout\">\
             <button class=\"signout\">{logout}Sign out</button></form></div>\
             </aside>\
             <main class=\"content\">{body}</main></div>",
            menu = icon(ICON_MENU),
            version = esc(&version_label()),
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

pub fn home(user: &User) -> String {
    let mut tiles = String::new();
    if user.is_admin {
        tiles.push_str(&tile("/admin/users", "Users", ICON_USERS));
    }
    app_page(
        "Home",
        "home",
        user,
        &format!(
            "<section class=\"hero\"><h1 class=\"hero-word\">WillCrock</h1>\
             <img src=\"/static/crest.png\" alt=\"WillCrock crest: ID IAM SCIO\" class=\"hero-crest\"></section>\
             <div class=\"tile-grid\">{tiles}</div>"
        ),
    )
}

pub fn admin_users(users: &[User], me: &User) -> String {
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
        let html = home(&user(1, "mike", false));
        assert!(html.contains("side-link active\" href=\"/\""));
        assert!(html.contains("class=\"version\""));
        assert!(html.contains("action=\"/logout\""));
        assert!(html.contains("mike@example.com"));
    }

    #[test]
    fn users_link_is_admin_only() {
        assert!(!home(&user(1, "sam", false)).contains("/admin/users"));
        assert!(home(&user(1, "mike", true)).contains("href=\"/admin/users\""));
    }

    #[test]
    fn admin_page_marks_users_active() {
        let me = user(1, "mike", true);
        let html = admin_users(std::slice::from_ref(&me), &me);
        assert!(html.contains("side-link active\" href=\"/admin/users\""));
    }

    #[test]
    fn home_is_a_crest_hero_with_admin_tile() {
        let member = home(&user(1, "sam", false));
        assert!(member.contains("class=\"hero-crest\""));
        assert!(!member.contains("Hello"));
        assert!(!member.contains("class=\"tile\""));
        assert!(home(&user(2, "mike", true)).contains("class=\"tile\" href=\"/admin/users\""));
    }

    #[test]
    fn user_text_is_escaped() {
        let html = home(&user(1, "<b>x</b>", false));
        assert!(!html.contains("<b>x</b>"));
        assert!(html.contains("&lt;b&gt;x&lt;/b&gt;"));
    }
}
