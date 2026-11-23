//! 服务端渲染的页面：登录、授权确认、一句话的结果。
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
button.secondary{background:light-dark(#e8e8ec,#2c2c33);color:inherit}"#;

/// 内容安全策略。样式只认这一段的哈希，表单只能提交到本站。
static POLICY: LazyLock<String> = LazyLock::new(|| {
    let digest = BASE64.encode(Sha256::digest(STYLE.as_bytes()));
    format!(
        "default-src 'none'; style-src 'sha256-{digest}'; form-action 'self'; base-uri 'none'; frame-ancestors 'none'"
    )
});

pub fn content_security_policy() -> &'static str {
    &POLICY
}

/// 页面共用的安全响应头。
pub const PAGE_HEADERS: [(&str, &str); 3] = [
    ("content-type", "text/html; charset=utf-8"),
    ("cache-control", "no-store"),
    ("referrer-policy", "no-referrer"),
];

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

fn page(title: &str, body: &str) -> String {
    format!(
        "<!doctype html><html lang=\"zh-CN\"><head><meta charset=\"utf-8\">\
<meta name=\"viewport\" content=\"width=device-width, initial-scale=1\">\
<meta name=\"referrer\" content=\"no-referrer\">\
<title>{}</title><style>{STYLE}</style></head><body><main>{}</main></body></html>",
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

/// 登录页。`error` 是上一回失败的原因；`next` 是登录之后要去的地方。
pub fn login_page(error: Option<&str>, next: &str, csrf: &str) -> String {
    let error = match error {
        Some(error) => format!("<p class=\"error\" role=\"alert\">{}</p>", escape(error)),
        None => String::new(),
    };
    page(
        "登录 · Assemble",
        &format!(
            "<h1>登录 Assemble</h1>{error}\
<form method=\"post\" action=\"/web/login\">\
<input type=\"hidden\" name=\"csrf\" value=\"{}\">\
<input type=\"hidden\" name=\"next\" value=\"{}\">\
<label>用户名<input name=\"username\" autocomplete=\"username\" autocapitalize=\"none\" spellcheck=\"false\" maxlength=\"64\" required></label>\
<label>密码<input type=\"password\" name=\"password\" autocomplete=\"current-password\" maxlength=\"1024\" required></label>\
<button type=\"submit\">登录</button></form>",
            escape(csrf),
            escape(next)
        ),
    )
}

/// 桌面端的授权确认页。
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
            "<h1>登录 Assemble 桌面端</h1>\
<p>当前登录：<strong>{}</strong></p>\
<p><strong>{}</strong>（{}）想要访问你的账户：收发文件、使用云盘。</p>\
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
    fn the_login_page_carries_its_hidden_fields_and_no_scripts() {
        let html = login_page(
            Some("用户名或密码不对"),
            "/desktop/authorize?request=abc",
            "csrf-value",
        );
        assert!(html.contains("<title>登录 · Assemble</title>"));
        assert!(html.contains("action=\"/web/login\""));
        assert!(html.contains("name=\"csrf\" value=\"csrf-value\""));
        assert!(html.contains("name=\"next\" value=\"/desktop/authorize?request=abc\""));
        assert!(html.contains("用户名或密码不对"));
        assert!(!html.contains("<script"));
        assert!(!html.contains("onclick"));
        let clean = login_page(None, "", "c");
        assert!(!clean.contains("role=\"alert\""));
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
        assert!(policy.contains("frame-ancestors 'none'"));
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
    fn message_pages_and_platform_labels_read_well() {
        let html = message_page("已退出", "可以关闭此页面。");
        assert!(html.contains("<h1>已退出</h1><p>可以关闭此页面。</p>"));
        assert_eq!(platform_label(Platform::Windows), "Windows");
        assert_eq!(platform_label(Platform::Macos), "Mac");
        assert_eq!(platform_label(Platform::Linux), "Linux");
    }
}
