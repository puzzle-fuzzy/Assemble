//! 服务端渲染的页面：登录、注册、授权确认、账户、一句话的结果。
//!
//! 没有模板引擎，也没有前端框架：都是 Rust 里的字符串，动态内容一律经过 [`escape`]。
//! 样式只有一段固定的 CSS，用它的内容哈希放进内容安全策略，不开 `unsafe-inline`；
//! 页面里没有脚本。

use std::borrow::Cow;
use std::sync::LazyLock;

use base64::Engine as _;
use base64::engine::general_purpose::STANDARD as BASE64;
use sha2::{Digest, Sha256};

const STYLE: &str = r#":root{color-scheme:light dark;font:16px/1.6 system-ui,-apple-system,"PingFang SC","Microsoft YaHei",sans-serif}
body{margin:0;min-height:100vh;display:grid;place-items:center;background:light-dark(#f6f6f7,#141416);color:light-dark(#1b1b1f,#ededf0)}
main{box-sizing:border-box;width:min(26rem,calc(100vw - 2rem));padding:1.75rem;border:1px solid light-dark(#dcdce1,#2c2c33);border-radius:14px;background:light-dark(#fff,#1c1c21)}
h1{margin:0 0 .5rem;font-size:1.25rem;line-height:1.3}
p{margin:.5rem 0}
.error{color:light-dark(#a51f24,#ff8a8f)}
.hint{color:light-dark(#66666f,#a0a0aa);font-size:.875rem}
label{display:block;margin:.9rem 0 0;font-size:.875rem}
input:not([type=hidden]){display:block;box-sizing:border-box;width:100%;margin-top:.3rem;padding:.55rem .7rem;border:1px solid light-dark(#b9b9c2,#46464f);border-radius:8px;background:light-dark(#fff,#141416);color:inherit;font:inherit}
button{margin:1.25rem .6rem 0 0;padding:.55rem 1.1rem;border:0;border-radius:8px;background:light-dark(#1f5fd0,#5b8ff5);color:#fff;font:inherit;cursor:pointer}
button.secondary{background:light-dark(#e8e8ec,#2c2c33);color:inherit}
.avatar{display:inline-grid;place-items:center;width:48px;height:48px;border-radius:50%;object-fit:cover;background:light-dark(#e8e8ec,#2c2c33);font-size:1.25rem}
h2{font-size:1rem;margin:1.5rem 0 .5rem}
a{color:light-dark(#1f5fd0,#5b8ff5)}"#;

/// 一份内容安全策略：什么都不加载，样式只认上面那一段的哈希，不许嵌进别的页面；
/// 表单能去哪里由 `form_action` 决定。
fn policy(form_action: &str) -> String {
    let digest = BASE64.encode(Sha256::digest(STYLE.as_bytes()));
    format!(
        "default-src 'none'; style-src 'sha256-{digest}'; {form_action}; base-uri 'none'; frame-ancestors 'none'; img-src https://avatars.githubusercontent.com"
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

pub(crate) fn page(title: &str, body: &str) -> String {
    format!(
        "<!doctype html><html lang=\"zh-CN\"><head><meta charset=\"utf-8\">\
<meta name=\"viewport\" content=\"width=device-width, initial-scale=1\">\
<meta name=\"referrer\" content=\"{}\">\
<title>{}</title><style>{STYLE}</style></head><body><main>{}</main></body></html>",
        REFERRER_POLICY,
        escape(title),
        body
    )
}

/// 一句话的结果页：已登录、已退出、请求不合法……
pub fn message_page(title: &str, body: &str) -> String {
    page(
        title,
        &format!("<h1>{}</h1><p>{}</p>", escape(title), escape(body)),
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
        Some(error) => format!("<p class=\"error\" role=\"alert\">{}</p>", escape(error)),
        None => String::new(),
    }
}

/// 邮箱登录始终提供公开注册；GitHub 入口只在凭据配置完整后显示。
pub fn login_page(error: Option<&str>, next: &str, csrf: &str, github_enabled: bool) -> String {
    let github = if github_enabled {
        format!(
            "<p><a href=\"{}\">使用 GitHub 登录</a></p>",
            escape(&carry_next("/web/github/start", next))
        )
    } else {
        String::new()
    };
    page(
        "登录 · Assemble",
        &format!(
            "<h1>登录 Assemble</h1>{}<form method=\"post\" action=\"/web/login\"><input type=\"hidden\" name=\"csrf\" value=\"{}\"><input type=\"hidden\" name=\"next\" value=\"{}\"><label for=\"email\">邮箱</label><input id=\"email\" type=\"email\" name=\"email\" autocomplete=\"username\" spellcheck=\"false\" maxlength=\"254\" required><label for=\"password\">密码</label><input id=\"password\" type=\"password\" name=\"password\" autocomplete=\"current-password\" maxlength=\"1024\" required><button type=\"submit\">登录</button></form>{}<p><a href=\"{}\">注册账户</a> · <a href=\"/web/forgot\">忘记密码</a></p><p class=\"hint\">原来使用用户名登录？<a href=\"{}\">为旧账户绑定邮箱</a></p>",
            alert(error),
            escape(csrf),
            escape(next),
            github,
            escape(&carry_next("/web/register", next)),
            escape(&carry_next("/web/legacy", next))
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
    page(
        "确认登录 · Assemble",
        &format!(
            "<h1>登录 Assemble</h1>\
<p>当前登录：<strong>{}</strong></p>\
<p><strong>{}</strong>（{}）想以你账户里一台设备的身份登录：收发文件、使用云盘、管理日程与提醒。</p>\
<form method=\"post\" action=\"/desktop/authorize\">\
<input type=\"hidden\" name=\"csrf\" value=\"{}\">\
<input type=\"hidden\" name=\"request\" value=\"{}\">\
<button type=\"submit\" name=\"choice\" value=\"allow\">确认登录</button>\
<button type=\"submit\" name=\"choice\" value=\"deny\" class=\"secondary\">取消</button></form>\
<p class=\"hint\">不是你本人在操作的话，请点\"取消\"。</p>",
            escape(account),
            escape(device_name),
            escape(platform_label),
            escape(csrf),
            escape(request)
        ),
    )
}

/// 平台的显示名。
pub fn platform_label(platform: assemble_protocol::primitives::Platform) -> &'static str {
    use assemble_protocol::primitives::Platform;
    match platform {
        Platform::Windows => "Windows",
        Platform::Macos => "Mac",
        Platform::Linux => "Linux",
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
        assert!(html.contains("/web/legacy"));
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
        assert!(html.contains("<h1>已退出</h1><p>可以关闭此页面。</p>"));
        assert_eq!(platform_label(Platform::Windows), "Windows");
        assert_eq!(platform_label(Platform::Macos), "Mac");
        assert_eq!(platform_label(Platform::Linux), "Linux");
    }
}
