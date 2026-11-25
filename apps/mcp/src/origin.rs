//! 连接哪个服务：由构建决定，和桌面端是同一条规矩。
//!
//! 设置了 `ASSEMBLE_SERVER_ORIGIN` 就连它。没设的时候，调试构建连本机开发服务，发布构建
//! 拒绝启动——宁可起不来，也不连到一个猜出来的地址。
//!
//! 调试构建另认一个环境变量 `ASSEMBLE_MCP_ORIGIN`，只给本机验证与测试用：把入口指向
//! 一个临时起的服务端。发布构建里没有这条路径。理由和"不从环境变量里读地址"一样：
//! 谁能改这个进程的环境变量，谁就能让它在用户的浏览器里打开一个仿冒的登录页。

use assemble_client::{OriginError, ServerOrigin};

/// 连哪个服务没定下来，或者定得不合法。
#[derive(Debug, thiserror::Error)]
pub enum OriginChoice {
    /// 这个构建没有带服务地址。多半是忘了在构建时给出 `ASSEMBLE_SERVER_ORIGIN`。
    #[error("this build has no service address; build it with ASSEMBLE_SERVER_ORIGIN set")]
    NotConfigured,
    #[error(transparent)]
    Invalid(#[from] OriginError),
}

/// 这一次运行该连的服务。
pub fn origin() -> Result<ServerOrigin, OriginChoice> {
    choose(
        overridden().as_deref(),
        option_env!("ASSEMBLE_SERVER_ORIGIN"),
        cfg!(debug_assertions),
    )
}

/// 回退规则：`overridden` 优先（只有调试构建会给），其次是构建时定下的地址；
/// 都没有时，调试构建连本地开发服务，发布构建拒绝启动。
fn choose(
    overridden: Option<&str>,
    configured: Option<&str>,
    debug: bool,
) -> Result<ServerOrigin, OriginChoice> {
    // 编译期读取的值只可能是 `None`、空串或一段真实地址。
    if let Some(value) = overridden.filter(|value| !value.is_empty()) {
        return ServerOrigin::parse(value).map_err(OriginChoice::Invalid);
    }
    match configured.filter(|value| !value.is_empty()) {
        Some(origin) => ServerOrigin::parse(origin).map_err(OriginChoice::Invalid),
        None if debug => Ok(ServerOrigin::development()),
        None => Err(OriginChoice::NotConfigured),
    }
}

fn overridden() -> Option<String> {
    // 发布构建不认这个变量：客户端只访问构建时确定的那个来源。
    if !cfg!(debug_assertions) {
        return None;
    }
    std::env::var("ASSEMBLE_MCP_ORIGIN")
        .ok()
        .filter(|value| !value.is_empty())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_configured_address_wins_and_an_empty_one_counts_as_missing() {
        let chosen = choose(None, Some("https://as.example.com"), true).unwrap();
        assert_eq!(chosen.as_str(), "https://as.example.com");
        assert!(choose(None, Some(""), true).is_ok());
    }

    #[test]
    fn without_an_address_a_debug_build_falls_back_and_a_release_build_refuses() {
        assert_eq!(
            choose(None, None, true).unwrap(),
            ServerOrigin::development()
        );
        assert!(matches!(
            choose(None, None, false),
            Err(OriginChoice::NotConfigured)
        ));
    }

    #[test]
    fn only_the_rules_of_an_origin_are_accepted() {
        // 不加密的连接只能用在回环地址上；带路径的不算一个来源。
        assert!(choose(Some("http://example.com"), None, true).is_err());
        assert!(choose(Some("https://as.example.com/app"), None, true).is_err());
        assert_eq!(
            choose(
                Some("http://127.0.0.1:8787"),
                Some("https://as.example.com"),
                true
            )
            .unwrap()
            .as_str(),
            "http://127.0.0.1:8787"
        );
    }
}
