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

fn app_page(title: &str, user: &User, body: &str) -> String {
    let who = user.name.as_deref().unwrap_or(&user.email);
    let users_link = if user.is_admin {
        "<li class=\"nav-item\"><a class=\"nav-link\" href=\"/admin/users\">Users</a></li>"
    } else {
        ""
    };
    shell(
        title,
        &format!(
            "<nav class=\"navbar navbar-expand navbar-crest\" data-bs-theme=\"dark\"><div class=\"container\">\
             <a class=\"navbar-brand fw-semibold d-flex align-items-center gap-2\" href=\"/\">\
             <img src=\"/favicon.svg\" alt=\"\" width=\"34\" height=\"34\">WillCrock</a>\
             <ul class=\"navbar-nav me-auto\">{users_link}</ul>\
             <span class=\"navbar-text me-3 d-none d-sm-inline\">{}</span>\
             <form method=\"post\" action=\"/logout\" class=\"m-0\">\
             <button class=\"btn btn-sm btn-outline-light\">Sign out</button></form>\
             </div></nav>\
             <main class=\"container py-4\"><h1 class=\"h3 mb-4\">{}</h1>{}</main>",
            esc(who),
            esc(title),
            body
        ),
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
    let who = user.name.as_deref().unwrap_or(&user.email);
    app_page(
        "Home",
        user,
        &format!(
            "<div class=\"card shadow-sm\"><div class=\"card-body\">Hello {}.</div></div>",
            esc(who)
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
        me,
        &format!(
            "<div class=\"card shadow-sm mb-4\"><div class=\"table-responsive\">\
             <table class=\"table table-hover align-middle mb-0\"><tbody>{rows}</tbody></table>\
             </div></div>\
             <div class=\"card shadow-sm\"><div class=\"card-body\">\
             <h2 class=\"h5 mb-3\">Add user</h2>\
             <form method=\"post\" action=\"/admin/users\" class=\"row g-2 align-items-center\">\
             <div class=\"col-12 col-md\"><input class=\"form-control\" type=\"email\" name=\"email\" placeholder=\"Email\" required></div>\
             <div class=\"col-12 col-md\"><input class=\"form-control\" name=\"name\" placeholder=\"Name\"></div>\
             <div class=\"col-auto\"><button class=\"btn btn-primary\">Add</button></div>\
             </form></div></div>"
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
