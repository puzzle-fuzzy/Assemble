//! 账户身份页面沿用现有的无框架表单，敏感值不进入地址查询串。

use crate::accounts::AccountRow;
use crate::auth::WebSession;
use crate::email_identity::Action;
use crate::templates::{
    alert, button, carry_next, escape, field, fields, form_page, github_option,
    message_with_actions, page, status,
};
use crate::web_ui::attrs;

pub fn hidden(name: &str, value: &str) -> String {
    format!(
        "<input type=\"hidden\" name=\"{}\" value=\"{}\">",
        escape(name),
        escape(value)
    )
}

pub fn email_input(value: &str) -> String {
    email_input_labeled(value, "邮箱")
}

fn email_input_labeled(value: &str, label: &str) -> String {
    field(
        "email",
        label,
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

pub fn request_page(
    kind: &str,
    error: Option<&str>,
    next: &str,
    csrf: &str,
    github_enabled: bool,
) -> String {
    let (title, path) = match kind {
        "register" => ("创建账户", "/web/register"),
        "legacy" => ("旧账户绑定邮箱", "/web/legacy"),
        _ => ("重置密码", "/web/forgot"),
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
    let github = if kind == "register" && github_enabled {
        github_option(next, "GitHub 注册")
    } else {
        String::new()
    };
    form_page(
        title,
        title,
        "",
        &format!(
            "{github}{}<form method=\"post\" action=\"{path}\" aria-label=\"{title}\" class=\"flex flex-col gap-6\">{}{}{}{}</form><p {}><a href=\"{}\">返回登录</a></p>",
            alert(error),
            hidden("csrf", csrf),
            hidden("next", next),
            fields(&format!("{legacy}{}", email_input(""))),
            button("发送验证邮件", "submit", "type=\"submit\""),
            attrs("field_description"),
            escape(&carry_next("/web/login", next))
        ),
    )
}

pub fn verify_entry(csrf: &str, error: Option<&str>) -> String {
    form_page(
        "验证邮箱",
        "验证邮箱",
        "",
        &format!(
            "{}<form method=\"post\" action=\"/web/verify/prepare\" aria-label=\"验证邮箱\" class=\"flex flex-col gap-6\">{}{}{}</form><script src=\"/web/verify.js\" defer></script>",
            alert(error),
            hidden("csrf", csrf),
            fields(&field(
                "token",
                "验证码",
                "name=\"token\" autocomplete=\"one-time-code\" autocapitalize=\"none\" autocorrect=\"off\" spellcheck=\"false\" maxlength=\"128\" enterkeyhint=\"done\" required"
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
                    "显示名（选填）",
                    "name=\"display_name\" autocomplete=\"nickname\" maxlength=\"60\"",
                )
            } else {
                String::new()
            },
            password_input("password", "密码（至少 8 个字符）", "new-password"),
            password_input("password2", "确认密码", "new-password")
        )
    } else {
        String::new()
    };
    let has_inputs = !inputs.is_empty();
    let form = format!(
        "{}<form method=\"post\" action=\"/web/verify\" aria-label=\"{title}\" class=\"flex flex-col gap-6\">{}{}{}{}</form>",
        alert(error),
        hidden("csrf", csrf),
        hidden("token", token),
        if has_inputs {
            fields(&inputs)
        } else {
            String::new()
        },
        button(
            button_label,
            if has_inputs { "submit" } else { "button" },
            "type=\"submit\""
        )
    );
    let email = &action.email;
    if has_inputs {
        form_page(title, title, email, &form)
    } else {
        message_with_actions(title, &escape(email), &form, "verified")
    }
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
                "<p class=\"text-sm text-muted-foreground\">需验证当前邮箱和新邮箱。</p><form method=\"post\" action=\"/web/account/email\" aria-labelledby=\"email-title\" class=\"flex flex-col gap-6\">{csrf}{}{}</form>",
                fields(&email_input_labeled("", "新邮箱")),
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
                "<p class=\"break-words text-sm text-muted-foreground\">{}</p>",
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
                    "当前密码",
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
                    password_input("current", "当前密码", "current-password"),
                    password_input("password", "新密码（至少 8 个字符）", "new-password"),
                    password_input("password2", "确认新密码", "new-password")
                )),
                button("改密码", "submit", "type=\"submit\"")
            ),
        )
    } else {
        "<p><a href=\"/web/forgot\">设置密码</a></p>".to_owned()
    };
    page(
        "账户 · Assemble",
        "账户",
        &format!(
            "<div class=\"flex min-w-0 items-center gap-3\">{}<div class=\"flex min-w-0 flex-col gap-1\"><p class=\"break-words font-medium\">{}</p><p class=\"break-words text-sm text-muted-foreground\">主邮箱：{}</p></div></div>{}{}{}{}{}<div {}></div><form method=\"post\" action=\"/web/logout\" aria-label=\"退出登录\">{}</form>",
            avatar(account),
            escape(&account.display_name),
            escape(email),
            alert(error),
            if changed {
                status("密码已改好，其他网页登录已退出。")
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
    form_page(
        "完成 GitHub 注册",
        "完成注册",
        "",
        &format!(
            "{}<form method=\"post\" action=\"/web/github/profile\" aria-label=\"完成 GitHub 注册\" class=\"flex flex-col gap-6\">{}{}{}</form><p {}><a href=\"/web/login\">登录已有账户</a></p>",
            alert(error),
            hidden("csrf", csrf),
            fields(&format!(
                "{}{}",
                email_input_labeled(email, "主邮箱"),
                field(
                    "display_name",
                    "显示名",
                    &format!(
                        "name=\"display_name\" value=\"{}\" autocomplete=\"nickname\" maxlength=\"60\" required",
                        escape(name)
                    )
                )
            )),
            button("完成注册", "submit", "type=\"submit\""),
            attrs("field_description")
        ),
    )
}
