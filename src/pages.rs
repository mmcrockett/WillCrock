use crate::auth::User;

pub fn esc(s: &str) -> String {
    s.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
        .replace('\'', "&#39;")
}

fn layout(title: &str, body: &str) -> String {
    format!(
        "<!doctype html><html><head><meta charset=\"utf-8\">\
         <meta name=\"viewport\" content=\"width=device-width,initial-scale=1\">\
         <title>{}</title></head><body><h1>{}</h1>{}</body></html>",
        esc(title),
        esc(title),
        body
    )
}

pub fn login(msg: Option<&str>) -> String {
    layout(
        "Sign in",
        &format!(
            "{}<form method=\"post\" action=\"/login\">\
             <label>Email <input type=\"email\" name=\"email\" required autofocus></label> \
             <button>Send code</button></form>",
            msg.map(|m| format!("<p>{}</p>", esc(m)))
                .unwrap_or_default()
        ),
    )
}

pub fn code(email: &str, error: bool) -> String {
    layout(
        "Enter code",
        &format!(
            "{}<p>If {} is on the list, a code is on its way.</p>\
             <form method=\"post\" action=\"/login/code\">\
             <input type=\"hidden\" name=\"email\" value=\"{}\">\
             <label>Code <input name=\"code\" required autofocus autocomplete=\"one-time-code\" \
             autocapitalize=\"characters\" maxlength=\"12\"></label> \
             <button>Sign in</button></form>\
             <p><a href=\"/login\">Start over</a></p>",
            if error {
                "<p>That code didn't work.</p>"
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
    layout(
        "WillCrock",
        &format!(
            "<p>Hello {}.</p>{}\
             <form method=\"post\" action=\"/logout\"><button>Sign out</button></form>",
            esc(who),
            if user.is_admin {
                "<p><a href=\"/admin/users\">Manage users</a></p>"
            } else {
                ""
            }
        ),
    )
}

pub fn admin_users(users: &[User], me: i64) -> String {
    let rows: String = users
        .iter()
        .map(|u| {
            let actions = if u.id == me {
                "(you)".to_string()
            } else {
                let (state_path, state_label) = if u.disabled {
                    ("enable", "Enable")
                } else {
                    ("disable", "Disable")
                };
                let admin_label = if u.is_admin { "Remove admin" } else { "Make admin" };
                format!(
                    "<form method=\"post\" action=\"/admin/users/{id}/{state_path}\" style=\"display:inline\"><button>{state_label}</button></form> \
                     <form method=\"post\" action=\"/admin/users/{id}/admin\" style=\"display:inline\"><button>{admin_label}</button></form>",
                    id = u.id
                )
            };
            format!(
                "<tr><td>{}</td><td>{}</td><td>{}</td><td>{}</td><td>{}</td></tr>",
                esc(&u.email),
                esc(u.name.as_deref().unwrap_or("")),
                if u.is_admin { "admin" } else { "" },
                if u.disabled { "disabled" } else { "" },
                actions
            )
        })
        .collect();
    layout(
        "Users",
        &format!(
            "<p><a href=\"/\">Home</a></p>\
             <table><tr><th>Email</th><th>Name</th><th></th><th></th><th></th></tr>{rows}</table>\
             <h2>Add user</h2>\
             <form method=\"post\" action=\"/admin/users\">\
             <label>Email <input type=\"email\" name=\"email\" required></label> \
             <label>Name <input name=\"name\"></label> \
             <label><input type=\"checkbox\" name=\"is_admin\" value=\"1\"> Admin</label> \
             <button>Add</button></form>"
        ),
    )
}

pub fn not_found() -> String {
    layout("Not found", "<p><a href=\"/\">Home</a></p>")
}

pub fn error() -> String {
    layout("Something went wrong", "<p>Try again in a moment.</p>")
}
