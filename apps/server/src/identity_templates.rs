//! 账户身份页面沿用现有的无框架表单，敏感值不进入地址查询串。

use crate::accounts::AccountRow;
use crate::auth::WebSession;
use crate::email_identity::Action;
use crate::templates::{alert, carry_next, escape, page};

pub fn hidden(name: &str, value: &str) -> String {
    format!(
        "<input type=\"hidden\" name=\"{}\" value=\"{}\">",
        escape(name),
        escape(value)
    )
}

pub fn email_input(value: &str) -> String {
    format!(
        "<label for=\"email\">邮箱</label><input id=\"email\" type=\"email\" name=\"email\" value=\"{}\" autocomplete=\"email\" spellcheck=\"false\" maxlength=\"254\" required>",
        escape(value)
    )
}

pub fn password_input(name: &str, label: &str, autocomplete: &str) -> String {
    password_input_id(name, name, label, autocomplete)
}

fn password_input_id(name: &str, id: &str, label: &str, autocomplete: &str) -> String {
    format!(
        "<label for=\"{id}\">{}</label><input id=\"{id}\" type=\"password\" name=\"{name}\" autocomplete=\"{autocomplete}\" maxlength=\"1024\" required>",
        escape(label)
    )
}

pub fn request_page(kind: &str, error: Option<&str>, next: &str, csrf: &str) -> String {
    let (title, path, explanation) = match kind {
        "register" => (
            "注册 Assemble",
            "/web/register",
            "所有人都可以注册。先验证邮箱，再设置密码。",
        ),
        "legacy" => (
            "旧账户绑定邮箱",
            "/web/legacy",
            "使用原用户名和密码验证旧账户，再验证邮箱。原来的设备、文件与日程都会保留。",
        ),
        _ => (
            "找回或设置密码",
            "/web/forgot",
            "填写账户已验证的邮箱。GitHub 注册的账户也可以在这里设置密码。",
        ),
    };
    let legacy = if kind == "legacy" {
        format!(
            "<label for=\"username\">原用户名</label><input id=\"username\" name=\"username\" autocomplete=\"username\" maxlength=\"64\" required>{}",
            password_input("current", "原密码", "current-password")
        )
    } else {
        String::new()
    };
    page(
        title,
        &format!(
            "<h1>{title}</h1><p class=\"hint\">{explanation}</p>{}<form method=\"post\" action=\"{path}\">{}{}{}{}<button type=\"submit\">发送验证邮件</button></form><p><a href=\"{}\">返回登录</a></p>",
            alert(error),
            hidden("csrf", csrf),
            hidden("next", next),
            legacy,
            email_input(""),
            escape(&carry_next("/web/login", next))
        ),
    )
}

pub fn verify_entry(csrf: &str, error: Option<&str>) -> String {
    page(
        "验证邮箱",
        &format!(
            "<h1>验证邮箱</h1>{}<p class=\"hint\">请从邮件中复制验证码，或打开邮件里的验证链接。</p><form method=\"post\" action=\"/web/verify/prepare\">{}<label for=\"token\">邮件验证码</label><input id=\"token\" name=\"token\" autocomplete=\"one-time-code\" maxlength=\"128\" required><button type=\"submit\">继续验证</button></form><script src=\"/web/verify.js\" defer></script>",
            alert(error),
            hidden("csrf", csrf)
        ),
    )
}

pub fn verify_confirm(action: &Action, token: &str, csrf: &str, error: Option<&str>) -> String {
    let (title, button) = match action.kind.as_str() {
        "register" => ("验证邮箱并注册", "验证并注册"),
        "reset" => ("设置新密码", "保存新密码"),
        "change_old" => ("确认更换邮箱", "向新邮箱发送验证邮件"),
        _ => ("确认邮箱", "确认验证"),
    };
    let fields = if matches!(action.kind.as_str(), "register" | "reset") {
        format!(
            "{}{}{}",
            if action.kind == "register" {
                "<label for=\"display_name\">显示名（可不填）</label><input id=\"display_name\" name=\"display_name\" autocomplete=\"nickname\" maxlength=\"60\">"
            } else {
                ""
            },
            password_input("password", "密码（至少 8 个字符）", "new-password"),
            password_input("password2", "再输一次密码", "new-password")
        )
    } else {
        String::new()
    };
    page(
        title,
        &format!(
            "<h1>{title}</h1><p>邮箱：{}</p>{}<form method=\"post\" action=\"/web/verify\">{}{}{}<button type=\"submit\">{button}</button></form>",
            escape(&action.email),
            alert(error),
            hidden("csrf", csrf),
            hidden("token", token),
            fields
        ),
    )
}

pub fn avatar(account: &AccountRow) -> String {
    match account
        .avatar_url
        .as_deref()
        .filter(|url| crate::github::valid_avatar(url))
    {
        Some(url) => format!(
            "<img class=\"avatar\" src=\"{}\" width=\"48\" height=\"48\" alt=\"账户头像\" referrerpolicy=\"no-referrer\">",
            escape(url)
        ),
        None => format!(
            "<span class=\"avatar\" role=\"img\" aria-label=\"默认头像\">{}</span>",
            escape(
                &account
                    .display_name
                    .chars()
                    .next()
                    .unwrap_or('A')
                    .to_string()
            )
        ),
    }
}

pub fn account_page(
    account: &AccountRow,
    session: &WebSession,
    github: Option<&str>,
    github_enabled: bool,
    error: Option<&str>,
    changed: bool,
) -> String {
    let csrf = hidden("csrf", &session.csrf_token);
    let email = account.email.as_deref().unwrap_or("尚未绑定，请先验证邮箱");
    let email_form = if account.email.is_some() {
        format!(
            "<h2>更换主邮箱</h2><p class=\"hint\">先向原邮箱发送确认邮件，再验证新邮箱。GitHub 身份保持绑定，不自动跟随 GitHub 邮箱变化。</p><form method=\"post\" action=\"/web/account/email\">{csrf}{}<button type=\"submit\">更换邮箱</button></form>",
            email_input("")
        )
    } else {
        "<p><a href=\"/web/legacy\">验证原密码并绑定邮箱</a></p>".to_owned()
    };
    let github_form = match github {
        Some(login) => format!("<p>GitHub：{}（已绑定）</p>", escape(login)),
        None if github_enabled => format!(
            "<form method=\"post\" action=\"/web/account/github\">{csrf}{}<button type=\"submit\">绑定 GitHub</button></form>",
            password_input_id(
                "current",
                "github-current",
                "当前密码（绑定 GitHub 前核对）",
                "current-password"
            )
        ),
        None => String::new(),
    };
    let password = if account.password_enabled {
        format!(
            "<h2>修改密码</h2><form method=\"post\" action=\"/web/account/password\">{csrf}{}{}{}<button type=\"submit\">改密码</button></form>",
            password_input("current", "现在的密码", "current-password"),
            password_input("password", "新密码（至少 8 个字符）", "new-password"),
            password_input("password2", "再输一次新密码", "new-password")
        )
    } else {
        "<p>尚未设置密码。<a href=\"/web/forgot\">通过邮箱设置密码</a></p>".to_owned()
    };
    page(
        "账户 · Assemble",
        &format!(
            "<h1>账户</h1>{}<p>{}</p><p>主邮箱：{}</p>{}{}{}{}{}<p class=\"hint\">修改密码只在网页进行。电脑与助手使用各自的设备凭据。</p><form method=\"post\" action=\"/web/logout\"><button type=\"submit\" class=\"secondary\">退出登录</button></form>",
            avatar(account),
            escape(&account.display_name),
            escape(email),
            alert(error),
            if changed {
                "<p role=\"status\">密码已改好。别处的网页登录状态已退出；电脑与助手不受影响。</p>"
            } else {
                ""
            },
            github_form,
            password,
            email_form
        ),
    )
}

pub fn github_profile(csrf: &str, email: &str, name: &str, error: Option<&str>) -> String {
    page(
        "完成 GitHub 注册",
        &format!(
            "<h1>选择账户主邮箱</h1><p class=\"hint\">GitHub 已验证的邮箱可直接使用；填写其他邮箱时，我们会发邮件验证。邮箱相同的已有账户不会自动合并，请先登录原账户，再绑定 GitHub。</p>{}<form method=\"post\" action=\"/web/github/profile\">{}{}<label for=\"display_name\">显示名</label><input id=\"display_name\" name=\"display_name\" value=\"{}\" autocomplete=\"nickname\" maxlength=\"60\" required><button type=\"submit\">完成注册</button></form><p><a href=\"/web/login\">登录已有账户</a> · <a href=\"/web/legacy\">旧用户名账户先绑定邮箱</a></p>",
            alert(error),
            hidden("csrf", csrf),
            email_input(email),
            escape(name)
        ),
    )
}
