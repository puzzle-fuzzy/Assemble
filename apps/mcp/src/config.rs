//! 把这台助手接进某个客户端要用的配置片段。
//!
//! 片段里只有这个可执行文件在哪、带什么参数：没有任何秘密。令牌在系统钥匙串里，
//! 客户端启动它之后由它自己去取。

use std::fmt::Write as _;

use serde_json::json;

/// 客户端认的服务器名。两个客户端里都叫它。
pub const SERVER: &str = "assemble";

/// 跑起来需要哪些参数。
fn arguments(name: &str) -> Vec<String> {
    vec!["serve".to_owned(), "--name".to_owned(), name.to_owned()]
}

/// 给 Codex 的片段：贴进 `~/.codex/config.toml`。
pub fn codex(executable: &str, name: &str) -> String {
    let mut lines = vec![format!("[mcp_servers.{SERVER}]")];
    lines.push(format!("command = {}", quote_toml(executable)));
    lines.push(format!(
        "args = [{}]",
        arguments(name)
            .iter()
            .map(|arg| quote_toml(arg))
            .collect::<Vec<_>>()
            .join(", ")
    ));
    lines.join("\n")
}

/// 给 `ZCode` 的片段：并进 `~/.zcode/cli/config.json` 的 `mcp.servers`。
pub fn zcode(executable: &str, name: &str) -> Result<String, serde_json::Error> {
    let whole = json!({
        "mcp": {
            "servers": {
                SERVER: {
                    "type": "stdio",
                    "command": executable,
                    "args": arguments(name),
                }
            }
        }
    });
    serde_json::to_string_pretty(&whole)
}

/// TOML 里的一个字符串。
///
/// 路径通常用“字面量字符串”（单引号）最省事：Windows 的反斜杠不用加倍。
/// 但字面量字符串里不能有单引号、换行和控制字符，那种只好用基本字符串并转义。
fn quote_toml(text: &str) -> String {
    if !text.contains(['\'', '\n', '\r']) && !text.chars().any(char::is_control) {
        return format!("'{text}'");
    }
    let mut quoted = String::from("\"");
    for c in text.chars() {
        match c {
            '\\' => quoted.push_str("\\\\"),
            '"' => quoted.push_str("\\\""),
            '\n' => quoted.push_str("\\n"),
            '\r' => quoted.push_str("\\r"),
            '\t' => quoted.push_str("\\t"),
            c if c.is_control() => {
                // 往字符串里写不会失败。
                let _ = write!(quoted, "\\u{:04X}", u32::from(c));
            }
            c => quoted.push(c),
        }
    }
    quoted.push('"');
    quoted
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn codex_gets_a_table_with_the_command_and_its_arguments() {
        assert_eq!(
            codex("/Users/me/bin/assemble-mcp", "ZCode 的助手"),
            "[mcp_servers.assemble]\ncommand = '/Users/me/bin/assemble-mcp'\nargs = ['serve', '--name', 'ZCode 的助手']"
        );
        // Windows 的路径原样放在字面量字符串里，反斜杠不用加倍。
        assert!(
            codex(r"C:\Tools\assemble-mcp.exe", "助手")
                .contains(r"command = 'C:\Tools\assemble-mcp.exe'")
        );
    }

    #[test]
    fn zcode_gets_strict_json_with_only_the_known_keys() {
        let text = zcode("/opt/assemble-mcp", "ZCode 的助手").unwrap();
        let value: serde_json::Value = serde_json::from_str(&text).unwrap();
        let server = &value["mcp"]["servers"]["assemble"];
        assert_eq!(server["type"], "stdio");
        assert_eq!(server["command"], "/opt/assemble-mcp");
        assert_eq!(server["args"], json!(["serve", "--name", "ZCode 的助手"]));
        // 配置文件的格式是严格的：多一个键，整台服务器都会被悄悄丢掉。
        assert_eq!(server.as_object().unwrap().len(), 3);
    }

    #[test]
    fn a_path_with_a_quote_still_comes_out_as_one_string() {
        assert_eq!(quote_toml("it's"), "\"it's\"");
        // 字面量字符串里双引号和反斜杠不用转义。
        assert_eq!(quote_toml(r#"a"b\c"#), r#"'a"b\c'"#);
        assert_eq!(quote_toml("a\tb"), "\"a\\tb\"");
        assert_eq!(quote_toml("plain"), "'plain'");
    }
}
