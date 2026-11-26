//! 账户身份页面沿用现有的无框架表单，敏感值不进入地址查询串。

use crate::accounts::AccountRow;
use crate::auth::WebSession;
use crate::email_identity::Action;
use crate::templates::{alert, button, carry_next, escape, field, fields, page, status};
use crate::web_ui::attrs;

pub fn hidden(name: &str, value: &str) -> String {
    format!(
        "<input type=\"hidden\" name=\"{}\" value=\"{}\">",
        escape(name),
        escape(value)
    )
}

pub fn email_input(value: &str) -> String {
    field(
        "email",
        "邮箱",
        &format!(
            "type=\"email\" name=\"email\" value=\"{}\" autocomplete=\"email\" autocapitalize=\"none\" spellcheck=\"false\" maxlength=\"254\" required",
            escape(value)
        ),
    )
}

pub fn password_input(name: &str, label: &str, autocomplete: &str) -> String {
    password_input_id(name, name, label, autocomplete)
}

fn password_input_id(name: &str, id: &str, label: &str, autocomplete: &str) -> String {
    field(
        id,
        label,
        &format!(
            "type=\"password\" name=\"{}\" autocomplete=\"{}\" maxlength=\"1024\" required",
            escape(name),
            escape(autocomplete)
        ),
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
            "{}{}",
            field(
                "username",
                "原用户名",
                "name=\"username\" autocomplete=\"username\" maxlength=\"64\" required"
            ),
            password_input("current", "原密码", "current-password")
        )
    } else {
        String::new()
    };
    page(
        title,
        title,
        &format!(
            "<p class=\"text-sm text-muted-foreground\">{explanation}</p>{}<form method=\"post\" action=\"{path}\" class=\"flex flex-col gap-6\">{}{}{}{}</form><p class=\"text-center text-sm\"><a href=\"{}\">返回登录</a></p>",
            alert(error),
            hidden("csrf", csrf),
            hidden("next", next),
            fields(&format!("{legacy}{}", email_input(""))),
            button("发送验证邮件", "submit", "type=\"submit\""),
            escape(&carry_next("/web/login", next))
        ),
    )
}

pub fn verify_entry(csrf: &str, error: Option<&str>) -> String {
    page(
        "验证邮箱",
        "验证邮箱",
        &format!(
            "{}<p class=\"text-sm text-muted-foreground\">请从邮件中复制验证码，或打开邮件里的验证链接。</p><form method=\"post\" action=\"/web/verify/prepare\" class=\"flex flex-col gap-6\">{}{}{}</form><script src=\"/web/verify.js\" defer></script>",
            alert(error),
            hidden("csrf", csrf),
            fields(&field(
                "token",
                "邮件验证码",
                "name=\"token\" autocomplete=\"one-time-code\" autocapitalize=\"none\" spellcheck=\"false\" maxlength=\"128\" required"
            )),
            button("继续验证", "submit", "type=\"submit\"")
        ),
    )
}

pub fn verify_confirm(action: &Action, token: &str, csrf: &str, error: Option<&str>) -> String {
    let (title, button_label) = match action.kind.as_str() {
        "register" => ("验证邮箱并注册", "验证并注册"),
        "reset" => ("设置新密码", "保存新密码"),
        "change_old" => ("确认更换邮箱", "向新邮箱发送验证邮件"),
        _ => ("确认邮箱", "确认验证"),
    };
    let inputs = if matches!(action.kind.as_str(), "register" | "reset") {
        format!(
            "{}{}{}",
            if action.kind == "register" {
                field(
                    "display_name",
                    "显示名（可不填）",
                    "name=\"display_name\" autocomplete=\"nickname\" maxlength=\"60\"",
                )
            } else {
                String::new()
            },
            password_input("password", "密码（至少 8 个字符）", "new-password"),
            password_input("password2", "再输一次密码", "new-password")
        )
    } else {
        String::new()
    };
    page(
        title,
        title,
        &format!(
            "<p class=\"break-words text-sm text-muted-foreground\">邮箱：{}</p>{}<form method=\"post\" action=\"/web/verify\" class=\"flex flex-col gap-6\">{}{}{}{}</form>",
            escape(&action.email),
            alert(error),
            hidden("csrf", csrf),
            hidden("token", token),
            fields(&inputs),
            button(button_label, "submit", "type=\"submit\"")
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
            "<span {}><img class=\"size-full object-cover\" src=\"{}\" width=\"40\" height=\"40\" alt=\"账户头像\" referrerpolicy=\"no-referrer\"></span>",
            attrs("avatar"),
            escape(url)
        ),
        None => format!(
            "<span {}><span {} role=\"img\" aria-label=\"默认头像\">{}</span></span>",
            attrs("avatar"),
            attrs("avatar_fallback"),
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

fn account_section(id: &str, heading: &str, content: &str) -> String {
    format!(
        "<div {}></div><section class=\"flex flex-col gap-4\" aria-labelledby=\"{}\"><h2 id=\"{}\" class=\"text-sm font-medium\">{}</h2>{content}</section>",
        attrs("separator"),
        escape(id),
        escape(id),
        escape(heading)
    )
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
        account_section(
            "email-title",
            "更换主邮箱",
            &format!(
                "<p class=\"text-sm text-muted-foreground\">先向原邮箱发送确认邮件，再验证新邮箱。GitHub 身份保持绑定，不自动跟随 GitHub 邮箱变化。</p><form method=\"post\" action=\"/web/account/email\" aria-labelledby=\"email-title\" class=\"flex flex-col gap-6\">{csrf}{}{}</form>",
                fields(&email_input("")),
                button("更换邮箱", "submit", "type=\"submit\"")
            ),
        )
    } else {
        "<p><a href=\"/web/legacy\">验证原密码并绑定邮箱</a></p>".to_owned()
    };
    let github_form = match github {
        Some(login) => account_section(
            "github-title",
            "GitHub",
            &format!(
                "<p class=\"break-words text-sm text-muted-foreground\">GitHub：{}（已绑定）</p>",
                escape(login)
            ),
        ),
        None if github_enabled => account_section(
            "github-title",
            "GitHub",
            &format!(
                "<form method=\"post\" action=\"/web/account/github\" aria-labelledby=\"github-title\" class=\"flex flex-col gap-6\">{csrf}{}{}</form>",
                fields(&password_input_id(
                    "current",
                    "github-current",
                    "当前密码（绑定 GitHub 前核对）",
                    "current-password"
                )),
                button("绑定 GitHub", "submit", "type=\"submit\"")
            ),
        ),
        None => String::new(),
    };
    let password = if account.password_enabled {
        account_section(
            "password-title",
            "修改密码",
            &format!(
                "<form method=\"post\" action=\"/web/account/password\" aria-labelledby=\"password-title\" class=\"flex flex-col gap-6\">{csrf}{}{}</form>",
                fields(&format!(
                    "{}{}{}",
                    password_input("current", "现在的密码", "current-password"),
                    password_input("password", "新密码（至少 8 个字符）", "new-password"),
                    password_input("password2", "再输一次新密码", "new-password")
                )),
                button("改密码", "submit", "type=\"submit\"")
            ),
        )
    } else {
        "<p>尚未设置密码。<a href=\"/web/forgot\">通过邮箱设置密码</a></p>".to_owned()
    };
    page(
        "账户 · Assemble",
        "账户",
        &format!(
            "<div class=\"flex min-w-0 items-center gap-3\">{}<div class=\"flex min-w-0 flex-col gap-1\"><p class=\"break-words font-medium\">{}</p><p class=\"break-words text-sm text-muted-foreground\">主邮箱：{}</p></div></div>{}{}{}{}{}<div {}></div><p class=\"text-sm text-muted-foreground\">修改密码只在网页进行。电脑与助手使用各自的设备凭据。</p><form method=\"post\" action=\"/web/logout\" aria-label=\"退出登录\">{}</form>",
            avatar(account),
            escape(&account.display_name),
            escape(email),
            alert(error),
            if changed {
                status("密码已改好。别处的网页登录状态已退出；电脑与助手不受影响。")
            } else {
                String::new()
            },
            github_form,
            password,
            email_form,
            attrs("separator"),
            button("退出登录", "outline_full", "type=\"submit\"")
        ),
    )
}

pub fn github_profile(csrf: &str, email: &str, name: &str, error: Option<&str>) -> String {
    page(
        "完成 GitHub 注册",
        "选择账户主邮箱",
        &format!(
            "<p class=\"text-sm text-muted-foreground\">GitHub 已验证的邮箱可直接使用；填写其他邮箱时，我们会发邮件验证。邮箱相同的已有账户不会自动合并，请先登录原账户，再绑定 GitHub。</p>{}<form method=\"post\" action=\"/web/github/profile\" class=\"flex flex-col gap-6\">{}{}{}</form><p class=\"flex flex-wrap justify-center gap-x-4 gap-y-2 text-sm\"><a href=\"/web/login\">登录已有账户</a><a href=\"/web/legacy\">旧用户名账户先绑定邮箱</a></p>",
            alert(error),
            hidden("csrf", csrf),
            fields(&format!(
                "{}{}",
                email_input(email),
                field(
                    "display_name",
                    "显示名",
                    &format!(
                        "name=\"display_name\" value=\"{}\" autocomplete=\"nickname\" maxlength=\"60\" required",
                        escape(name)
                    )
                )
            )),
            button("完成注册", "submit", "type=\"submit\"")
        ),
    )
}
