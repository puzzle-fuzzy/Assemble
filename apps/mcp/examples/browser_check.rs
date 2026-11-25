//! 在真实浏览器里核对登录流程用的替身：替 `assemble-mcp login` 等回调，但不打开系统浏览器，
//! 登录也只存在内存里——不碰钥匙串，也不往屏幕上弹任何东西。
//!
//! 用法：`cargo run -p assemble-mcp --example browser_check <服务的地址>`。它把授权页的地址印出来，
//! 由一个真实的浏览器去走完；回调到了就换码，把结果印出来。最多等 5 分钟，和真的登录一样。
//! 为什么需要它：测试里的"浏览器"是 HTTP 客户端，不看内容安全策略。2026-11-25 正式服务上
//! 第一次真人登录，就是被授权页自己的策略拦在了最后一步，而自动化测试全是绿的。

use std::sync::Arc;

use assemble_mcp::browser::BrowserError;
use assemble_mcp::session::{self, Session};
use assemble_protocol::device::NewDevice;
use assemble_protocol::primitives::Platform;
use assemble_store::secret::MemorySecretStore;
use tokio_util::sync::CancellationToken;

/// 这台替身在账户里的名字，授权页上和"设备"列表里都看得到。
const NAME: &str = "浏览器核对";

#[tokio::main]
async fn main() {
    let service = std::env::args()
        .nth(1)
        .expect("用法：browser_check <服务的地址>");
    let Ok(origin) = assemble_client::ServerOrigin::parse(&service) else {
        eprintln!("服务地址不合法：{service}");
        std::process::exit(2);
    };
    let session = Session::new(origin, NAME, Arc::new(MemorySecretStore::new()));
    let device = NewDevice {
        name: NAME.to_owned(),
        platform: Platform::current(),
    };
    let outcome = session::login(
        &session,
        device,
        |url: String| async move {
            println!("授权页：{url}");
            Ok::<(), BrowserError>(())
        },
        &CancellationToken::new(),
    )
    .await;
    match outcome {
        Ok(stored) => println!(
            "登录成功：{}，这台替身在账户里叫“{}”",
            stored.account.username, stored.device.name
        ),
        Err(error) => {
            eprintln!("登录失败：{error:?}");
            std::process::exit(1);
        }
    }
}
