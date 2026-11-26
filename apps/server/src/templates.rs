//! 服务端渲染的页面：登录、注册、授权确认、账户、一句话的结果。
//!
//! 没有模板引擎，也没有前端框架：都是 Rust 里的字符串，动态内容一律经过 [`escape`]。
//! 共享组件在 Bun 构建时导出静态属性与样式；动态内容仍只在 Rust 侧转义、提交。
//! 样式的内容哈希进入 CSP，不开 `unsafe-inline`；仅邮箱验证页保留既有的本站脚本。

use std::borrow::Cow;
use std::sync::LazyLock;

use crate::web_ui::{STYLE, attrs, markup};
use base64::Engine as _;
use base64::engine::general_purpose::STANDARD as BASE64;
use sha2::{Digest, Sha256};

/// 一份内容安全策略：什么都不加载，样式只认上面那一段的哈希，不许嵌进别的页面；
/// 表单能去哪里由 `form_action` 决定。
fn policy(form_action: &str) -> String {
    let digest = BASE64.encode(Sha256::digest(STYLE.as_bytes()));
    format!(
        "default-src 'none'; style-src 'sha256-{digest}'; {form_action}; base-uri 'none'; frame-ancestors 'none'; img-src https://avatars.githubusercontent.com; font-src 'self'"
    )
}

/// 多数页面的那一份：表单只能提交到本站。
static POLICY: LazyLock<String> = LazyLock::new(|| policy("form-action 'self'"));

pub fn content_security_policy() -> &'static str {
    &POLICY
}

/// 账户绑定表单会跳到 GitHub 授权页，额外只允许这个固定来源。
pub fn account_content_security_policy() -> String {
    policy("form-action 'self' https://github.com")
}

/// 授权确认页的那一份：表单除了提交到本站，提交之后还要能被送回 `callback_origin`
/// （`http://127.0.0.1:<端口>`，这一次登录正在监听的回环来源）。
///
/// 浏览器对 `form-action` 的规矩是"提交之后的跳转目标也要落在允许之列"。确认之后服务端
/// 回的是"跳到回调地址"；按只认本站的那一份，这次跳转被悄悄拦掉，页面原地不动，用户看到
/// 的就是点了没反应——2026-11-25 正式服务上第一次真人登录正是这样。放行的是这一个端口，
/// 不是整个回环地址。
///
/// `callback_origin` 取自 [`assemble_protocol::login::AuthorizationRequest::callback_origin`]：
/// 那里已经保证它只有 `http://127.0.0.1:` 和数字，才能原样放进策略里。
pub fn authorize_content_security_policy(callback_origin: &str) -> String {
    policy(&format!("form-action 'self' {callback_origin}"))
}

/// 页面的来源策略：地址只随同站请求发回，不发给任何别的站点。
///
/// 不能用 `no-referrer`。按 Fetch 标准，在那个策略下浏览器对**同源**的 POST
/// 也会把 `Origin` 发成 `null`，写操作的 Origin 校验就会把自己的登录表单拒之门外。
/// 这一条是真实启动时才发现的：测试里的"浏览器"自己构造 `Origin`，复现不了它。
pub const REFERRER_POLICY: &str = "same-origin";

/// 把动态文字转成可以安全放进 HTML 的形式。
pub fn escape(value: &str) -> Cow<'_, str> {
    if value
        .bytes()
        .any(|byte| matches!(byte, b'&' | b'<' | b'>' | b'"' | b'\''))
    {
        let mut escaped = String::with_capacity(value.len() + 8);
        for character in value.chars() {
            match character {
                '&' => escaped.push_str("&amp;"),
                '<' => escaped.push_str("&lt;"),
                '>' => escaped.push_str("&gt;"),
                '"' => escaped.push_str("&quot;"),
                '\'' => escaped.push_str("&#39;"),
                other => escaped.push(other),
            }
        }
        Cow::Owned(escaped)
    } else {
        Cow::Borrowed(value)
    }
}

pub(crate) fn page(title: &str, heading: &str, body: &str) -> String {
    form_page(title, heading, "", body)
}

/// login-03 的单列卡片：说明在标题旁，表单内容不重复标题与引导文案。
pub(crate) fn form_page(title: &str, heading: &str, description: &str, body: &str) -> String {
    let description = if description.is_empty() {
        String::new()
    } else {
        format!(
            "<div {}>{}</div>",
            attrs("description"),
            escape(description)
        )
    };
    document(
        title,
        &format!(
            "<div class=\"flex min-h-svh flex-col items-center justify-center gap-6 bg-muted p-6 md:p-10\">\
<div class=\"flex w-full max-w-sm flex-col gap-6\">\
<header class=\"flex items-center gap-2 self-center font-medium\"><span aria-hidden=\"true\" class=\"flex size-6 items-center justify-center rounded-md bg-primary text-primary-foreground\">{}</span>Assemble</header>\
<main {}><div {}><div {}><h1>{}</h1></div>{description}</div><div {}>{body}</div></main></div></div>",
            markup("brand"),
            attrs("card"),
            attrs("auth_header"),
            attrs("title"),
            escape(heading),
            attrs("content")
        ),
    )
}

fn document(title: &str, body: &str) -> String {
    format!(
        "<!doctype html><html lang=\"zh-CN\"><head><meta charset=\"utf-8\">\
<meta name=\"viewport\" content=\"width=device-width, initial-scale=1\">\
<meta name=\"referrer\" content=\"{}\">\
<meta name=\"color-scheme\" content=\"light dark\">\
<title>{}</title><style>{STYLE}</style></head><body>{body}</body></html>",
        REFERRER_POLICY,
        escape(title)
    )
}

/// 一句话的结果页：已登录、已退出、请求不合法……
pub fn message_page(title: &str, body: &str) -> String {
    message_with_actions(title, &escape(body), "", "info")
}

/// 没有可填写字段时使用 Empty，不把结果和确认操作再包进一张表单卡片。
/// `body` 与 `actions` 仅由服务端组合；调用方须先转义动态文字。
pub(crate) fn message_with_actions(title: &str, body: &str, actions: &str, icon: &str) -> String {
    let actions = if actions.is_empty() {
        String::new()
    } else {
        format!("<div {}>{actions}</div>", attrs("empty_content"))
    };
    document(
        title,
        &format!(
            "<main class=\"flex min-h-svh items-center justify-center bg-background p-6 md:p-10\"><div class=\"w-full max-w-md\"><div {}><div {}><div {}>{}</div><div {}><h1>{}</h1></div><div {}>{body}</div></div>{actions}</div></div></main>",
            attrs("empty"),
            attrs("empty_header"),
            attrs("empty_media"),
            markup(icon),
            attrs("empty_title"),
            escape(title),
            attrs("empty_description")
        ),
    )
}

/// 普通网页退出后仍保留浏览器内的登录入口，不再让用户寻找桌面程序。
pub fn signed_out_page() -> String {
    message_with_actions(
        "已退出",
        "你已退出当前网页账户。",
        &format!(
            "{}{}",
            link_button("重新登录账户", "/web/login?next=%2Fweb%2Faccount", "button"),
            link_button("登录管理控制台", "/web/login?next=%2Fadmin%2F", "outline")
        ),
        "logout",
    )
}

/// 一条去另一个页面的链接，把"之后要回来"的参数原样带上。
pub(crate) fn carry_next(link: &str, next: &str) -> String {
    if next.is_empty() {
        link.to_owned()
    } else {
        format!(
            "{link}?next={}",
            url::form_urlencoded::byte_serialize(next.as_bytes()).collect::<String>()
        )
    }
}

pub(crate) fn alert(error: Option<&str>) -> String {
    match error {
        Some(error) => format!(
            "<div {} tabindex=\"-1\" autofocus><div {}>{}</div></div>",
            attrs("alert"),
            attrs("alert_description"),
            escape(error)
        ),
        None => String::new(),
    }
}

pub(crate) fn field(id: &str, label: &str, attributes: &str) -> String {
    format!(
        "<div {}><label for=\"{}\" {}>{}</label><input id=\"{}\" {} {attributes}></div>",
        attrs("field"),
        escape(id),
        attrs("label"),
        escape(label),
        escape(id),
        attrs("input")
    )
}

pub(crate) fn button(label: &str, variant: &str, attributes: &str) -> String {
    format!(
        "<button {} {attributes}>{}</button>",
        attrs(variant),
        escape(label)
    )
}

pub(crate) fn link_button(label: &str, href: &str, variant: &str) -> String {
    format!(
        "<a href=\"{}\" {}>{}</a>",
        escape(href),
        attrs(variant),
        escape(label)
    )
}

pub(crate) fn fields(content: &str) -> String {
    format!("<div {}>{content}</div>", attrs("group"))
}

pub(crate) fn status(message: &str) -> String {
    format!(
        "<div {}><div {}>{}</div></div>",
        attrs("status"),
        attrs("alert_description"),
        escape(message)
    )
}

pub(crate) fn github_option(next: &str, label: &str) -> String {
    format!(
        "<a href=\"{}\" {}>{}{}</a>{}",
        escape(&carry_next("/web/github/start", next)),
        attrs("auth_outline"),
        markup("github"),
        escape(label),
        markup("email_separator")
    )
}

/// 邮箱登录始终提供公开注册；GitHub 入口只在凭据配置完整后显示。
pub fn login_page(error: Option<&str>, next: &str, csrf: &str, github_enabled: bool) -> String {
    let admin = matches!(next, "/admin" | "/admin/");
    let (title, heading, hint) = if admin {
        (
            "登录管理控制台 · Assemble",
            "登录管理控制台",
            "请使用已获授权的超级管理员账户。登录后会直接进入网页管理控制台。",
        )
    } else {
        ("登录 · Assemble", "登录", "")
    };
    let github = if github_enabled {
        github_option(next, "使用 GitHub 登录")
    } else {
        String::new()
    };
    form_page(
        title,
        heading,
        if admin || github_enabled {
            hint
        } else {
            "使用邮箱和密码登录"
        },
        &format!(
            "{github}<form method=\"post\" action=\"/web/login\" aria-label=\"邮箱登录\" class=\"flex flex-col gap-6\"><input type=\"hidden\" name=\"csrf\" value=\"{}\"><input type=\"hidden\" name=\"next\" value=\"{}\">{}{}</form><p {}>还没有账户？ <a href=\"{}\">注册账户</a></p>",
            escape(csrf),
            escape(next),
            fields(&format!(
                "{}{}<div {}><div class=\"flex items-center justify-between gap-3\"><label for=\"password\" {}>密码</label><a class=\"text-sm\" href=\"{}\">忘记密码？</a></div><input id=\"password\" {} type=\"password\" name=\"password\" autocomplete=\"current-password\" maxlength=\"1024\" enterkeyhint=\"done\" required></div>",
                alert(error),
                field(
                    "email",
                    "邮箱",
                    "type=\"email\" name=\"email\" autocomplete=\"username\" autocapitalize=\"none\" autocorrect=\"off\" spellcheck=\"false\" maxlength=\"254\" enterkeyhint=\"next\" required"
                ),
                attrs("field"),
                attrs("label"),
                escape(&carry_next("/web/forgot", next)),
                attrs("input")
            )),
            button("登录", "submit", "type=\"submit\""),
            attrs("field_description"),
            escape(&carry_next("/web/register", next))
        ),
    )
}

/// 一台设备登录的授权确认页：桌面端与助手（`assemble-mcp`）走的是同一页。
pub fn authorize_page(
    request: &str,
    account: &str,
    device_name: &str,
    platform_label: &str,
    csrf: &str,
) -> String {
    message_with_actions(
        "确认登录",
        &format!(
            "<p><strong>{}</strong>（{}）正在请求登录。</p><p>当前账户：<strong>{}</strong>。</p><p>确认后可收发文件、使用云盘、管理日程与提醒。</p>",
            escape(device_name),
            escape(platform_label),
            escape(account)
        ),
        &format!(
            "<form method=\"post\" action=\"/desktop/authorize\" aria-label=\"设备登录授权\" class=\"flex flex-wrap justify-center gap-2\">\
<input type=\"hidden\" name=\"csrf\" value=\"{}\">\
<input type=\"hidden\" name=\"request\" value=\"{}\">\
{}{}</form><p class=\"text-xs text-muted-foreground\">不是你本人在操作的话，请点\"取消\"。</p>",
            escape(csrf),
            escape(request),
            button(
                "确认登录",
                "button",
                "type=\"submit\" name=\"choice\" value=\"allow\""
            ),
            button(
                "取消",
                "outline",
                "type=\"submit\" name=\"choice\" value=\"deny\""
            )
        ),
        "login",
    )
}

/// 平台的显示名。
pub fn platform_label(platform: assemble_protocol::primitives::Platform) -> &'static str {
    use assemble_protocol::primitives::Platform;
    match platform {
        Platform::Windows => "Windows",
        Platform::Macos => "Mac",
        Platform::Linux => "Linux",
        Platform::Iphone => "iPhone",
        Platform::Other => "未知平台",
    }
}

#[cfg(test)]
mod tests {
    use assemble_protocol::primitives::Platform;

    use super::*;

    #[test]
    fn dynamic_text_is_escaped_before_it_reaches_the_page() {
        assert_eq!(escape("plain"), "plain");
        assert_eq!(escape("a<b>&\"c\"'"), "a&lt;b&gt;&amp;&quot;c&quot;&#39;");
        assert_eq!(
            escape("<script>alert(1)</script>"),
            "&lt;script&gt;alert(1)&lt;/script&gt;"
        );
        // 借得到就不复制。
        assert!(matches!(escape("plain"), Cow::Borrowed(_)));
    }

    #[test]
    fn login_uses_email_and_registration_is_always_visible() {
        let html = login_page(None, "/desktop/authorize?request=abc", "csrf-value", false);
        assert!(html.contains("type=\"email\" name=\"email\" autocomplete=\"username\""));
        assert!(html.contains("name=\"csrf\" value=\"csrf-value\""));
        assert!(html.contains("/web/register?next=%2Fdesktop%2Fauthorize%3Frequest%3Dabc"));
        assert!(html.contains("/web/forgot"));
        assert!(!html.contains("/web/legacy"));
        assert!(!html.contains("/web/github/start"));
        assert!(login_page(None, "", "c", true).contains("/web/github/start"));
        assert!(!html.contains("<script"));
    }

    #[test]
    fn values_from_the_network_cannot_break_out_of_a_form_field() {
        let hostile = "\"><script>alert(1)</script>";
        let html = authorize_page(hostile, "用户\"><b>x</b>", hostile, "Mac", "c\"s");
        assert!(!html.contains("<script>alert"));
        assert!(html.contains("&quot;&gt;&lt;script&gt;alert(1)&lt;/script&gt;"));
        assert!(html.contains("value=\"c&quot;s\""));
        // 两个提交按钮：确认与取消。
        assert!(html.contains("name=\"choice\" value=\"allow\""));
        assert!(html.contains("name=\"choice\" value=\"deny\""));
    }

    #[test]
    fn the_policy_trusts_only_our_style_and_our_forms() {
        let policy = content_security_policy();
        assert!(policy.starts_with("default-src 'none'; style-src 'sha256-"));
        assert!(policy.contains("form-action 'self'"));
        assert!(
            policy
                .contains("frame-ancestors 'none'; img-src https://avatars.githubusercontent.com")
        );
        assert!(!policy.contains("unsafe-inline"));
        assert!(!policy.contains("script-src"));
        // 哈希确实来自页面里那一段样式。
        let html = message_page("t", "b");
        let style = html
            .split("<style>")
            .nth(1)
            .and_then(|rest| rest.split("</style>").next())
            .unwrap();
        assert_eq!(style, STYLE);
        let expected = format!(
            "'sha256-{}'",
            BASE64.encode(Sha256::digest(style.as_bytes()))
        );
        assert!(policy.contains(&expected));
    }

    #[test]
    fn the_authorize_page_lets_its_form_be_sent_back_to_the_loopback_address() {
        let policy = authorize_content_security_policy("http://127.0.0.1:61159");
        assert!(
            policy.contains("form-action 'self' http://127.0.0.1:61159;"),
            "{policy}"
        );
        // 只放行这一个端口，不是整个回环地址；回环来源在整份策略里只出现这一次。
        assert!(!policy.contains("http://127.0.0.1:*"), "{policy}");
        assert_eq!(policy.matches("http://").count(), 1, "{policy}");
        // 其余几条和别的页面一样严。
        for kept in [
            "default-src 'none'",
            "style-src 'sha256-",
            "base-uri 'none'",
            "frame-ancestors 'none'; img-src https://avatars.githubusercontent.com",
        ] {
            assert!(policy.contains(kept), "{policy}");
        }
        assert!(!policy.contains("unsafe-inline"), "{policy}");
        assert!(!policy.contains("script-src"), "{policy}");
    }

    #[test]
    fn message_pages_and_platform_labels_read_well() {
        let html = message_page("已退出", "可以关闭此页面。");
        assert!(html.contains("<h1>已退出</h1>"));
        assert!(html.contains("可以关闭此页面。"));
        assert!(html.contains("data-slot=\"empty\""));
        assert!(!html.contains("data-slot=\"card\""));
        assert_eq!(platform_label(Platform::Windows), "Windows");
        assert_eq!(platform_label(Platform::Macos), "Mac");
        assert_eq!(platform_label(Platform::Linux), "Linux");
        assert_eq!(platform_label(Platform::Iphone), "iPhone");
        assert_eq!(platform_label(Platform::Other), "未知平台");
    }
}
