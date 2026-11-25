//! 助手的登录：系统钥匙串里的一条设备令牌。
//!
//! 和桌面端走的是同一套浏览器授权（PKCE、回环回调、单次授权码），实现直接复用
//! `assemble-client` 的。钥匙串里的条目名和桌面应用分开，所以助手登录不会顶掉这台电脑上
//! 桌面应用自己的登录。
//!
//! 两条和桌面端一样的规矩：
//!
//! - **服务端没有确认撤销的会话，不在本机忘掉。** 退出时连不上服务端，本机的登录原样留着。
//! - **令牌只存在于这里和系统钥匙串。** 不进日志，不进工具的返回值；落盘时整份登录
//!   （连同令牌）是钥匙串里的一个秘密值。

use std::future::Future;
use std::sync::{Arc, Mutex, PoisonError};
use std::time::Duration;

use assemble_client::{
    Client, ClientBuildError, ClientError, LoginError, LoginListener, LoginRequest, RequestOptions,
    ServerOrigin, WaitOptions,
};
use assemble_protocol::account::Account;
use assemble_protocol::device::{Device, NewDevice, is_valid_device_name};
use assemble_protocol::login::{ExchangeResponse, LOGIN_DEADLINE};
use assemble_store::secret::{SecretError, SecretKey, SecretStore, SecretString};
use serde::{Deserialize, Serialize};
use tokio_util::sync::CancellationToken;

use crate::browser::BrowserError;

/// 登录时没说自己是哪一个助手，就用这个名字。
pub const DEFAULT_NAME: &str = "助手";

/// 换码与撤销请求的时限。
const REQUEST_TIMEOUT: Duration = Duration::from_secs(15);

/// 助手的名字不合服务端对设备名的规矩：去掉首尾空白后不能为空，最多 60 个字，不能有控制字符。
#[derive(Debug, thiserror::Error)]
#[error("the assistant name is not a valid device name")]
pub struct BadName;

/// 检查并整理助手的名字。没给就用默认的。
pub fn name_of(given: Option<&str>) -> Result<String, BadName> {
    let name = given.unwrap_or(DEFAULT_NAME).trim();
    if is_valid_device_name(name) {
        Ok(name.to_owned())
    } else {
        Err(BadName)
    }
}

/// 钥匙串里的条目：按服务来源区分，再按助手的名字区分。
///
/// 桌面应用用的是 `assemble.device-session:<服务来源>`，两边碰不到。
fn key_for(origin: &ServerOrigin, name: &str) -> SecretKey {
    SecretKey::new(format!("assemble.mcp-session:{origin}"), name)
}

/// 保存在钥匙串里的登录：令牌与它对应的公开信息。
#[derive(Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct StoredSession {
    token: String,
    account: Account,
    device: Device,
}

/// 从钥匙串里取出来的一整套登录。令牌只以 [`SecretString`] 的形式存在。
#[derive(Debug, Clone)]
pub struct Stored {
    pub token: SecretString,
    pub account: Account,
    pub device: Device,
}

impl Stored {
    fn from_exchange(response: ExchangeResponse, token: SecretString) -> Self {
        Self {
            token,
            account: response.account,
            device: response.device,
        }
    }

    /// 整份登录编码成一个值。它含有令牌，所以结果只能直接进钥匙串。
    fn encode(&self) -> Result<SecretString, serde_json::Error> {
        let plain = StoredSession {
            token: self.token.expose().to_owned(),
            account: self.account.clone(),
            device: self.device.clone(),
        };
        serde_json::to_string(&plain).map(SecretString::new)
    }

    /// 读不回来就是读不回来：错误里可能带着那段值的一部分，所以不往外传错误详情。
    fn decode(secret: &SecretString) -> Option<Self> {
        let plain: StoredSession = serde_json::from_str(secret.expose()).ok()?;
        Some(Self {
            token: SecretString::new(plain.token),
            account: plain.account,
            device: plain.device,
        })
    }
}

/// 本机的登录状态出了问题。
#[derive(Debug, thiserror::Error)]
pub enum SessionError {
    #[error("the credential store is unavailable: {0}")]
    Store(#[from] SecretError),
    /// 钥匙串里有东西，但不是这里存进去的样子。
    #[error("the stored session has an unknown shape")]
    Corrupt,
    #[error("not logged in")]
    NotLoggedIn,
    #[error("the client could not be built: {0}")]
    Build(#[from] ClientBuildError),
}

/// 一个助手在这台电脑上的登录。
pub struct Session {
    origin: ServerOrigin,
    key: SecretKey,
    store: Arc<dyn SecretStore>,
}

impl Session {
    pub fn new(origin: ServerOrigin, name: &str, store: Arc<dyn SecretStore>) -> Self {
        Self {
            key: key_for(&origin, name),
            origin,
            store,
        }
    }

    pub fn origin(&self) -> &ServerOrigin {
        &self.origin
    }

    /// 本机保存的登录。没登录过是 `None`；有东西但读不懂才是错。
    ///
    /// 退出、查看状态要用这个：没登录过是正常情形，不是"没办成"。
    fn stored(&self) -> Result<Option<Stored>, SessionError> {
        let Some(secret) = self.store.get(&self.key)? else {
            return Ok(None);
        };
        Stored::decode(&secret)
            .ok_or(SessionError::Corrupt)
            .map(Some)
    }

    /// 协议层要用的那份登录。没登录过是 `Err(NotLoggedIn)`：对工具调用来说这就是要告诉助手的事。
    pub fn load(&self) -> Result<Stored, SessionError> {
        self.stored()?.ok_or(SessionError::NotLoggedIn)
    }

    /// 把一份现成的登录存进本机。登录流程自己会调用；只有手里已经有一份登录时
    /// （比如测试）才需要直接调用。
    pub fn save(&self, stored: &Stored) -> Result<(), SessionError> {
        let secret = stored.encode().map_err(|_| SessionError::Corrupt)?;
        self.store.put(&self.key, &secret)?;
        Ok(())
    }

    /// 删掉本机的登录。调用方必须已经确认服务端那边也撤销了，或者已经不认它了。
    pub fn clear(&self) -> Result<(), SessionError> {
        self.store.delete(&self.key)?;
        Ok(())
    }

    /// 一个带着这份登录的客户端。克隆之间共享同一个令牌，所以别拿它去换另一份登录。
    fn client(&self, stored: &Stored) -> Result<Client, SessionError> {
        let client = Client::new(self.origin.clone())?;
        client.set_bearer(Some(stored.token.clone()));
        Ok(client)
    }

    /// 一个不带任何登录的客户端：登录时换码、撤销刚注册的设备都用它。
    fn fresh_client(&self) -> Result<Client, SessionError> {
        Ok(Client::new(self.origin.clone())?)
    }
}

/// 登录没有完成。
#[derive(Debug, thiserror::Error)]
pub enum LoginFailure {
    /// 已经登录过，而旧的那份没能撤销：没有开始新的登录。
    #[error("the previous session could not be retired: {0}")]
    Previous(#[from] LogoutFailure),
    /// 回调这一侧的问题：等得太久、被取消、用户在网页上点了"取消"。
    #[error(transparent)]
    Callback(#[from] LoginError),
    #[error("the service could not be reached: {0}")]
    Unreachable(#[from] ClientError),
    #[error("the client could not be built: {0}")]
    Build(#[from] ClientBuildError),
    /// 这台电脑上出的问题：钥匙串写不进去，或者客户端建不起来。
    /// 钥匙串写不进去时，刚注册的那台设备已经撤销，没有留下一台没人认领的设备。
    #[error("the new session could not be saved: {0}")]
    Save(#[from] SessionError),
}

/// 登录：在系统浏览器完成授权，用授权码换一台设备的令牌，存进钥匙串。
///
/// `open` 负责打开浏览器，拿到授权页的地址。它失败了也不算完：地址已经印在屏幕上，
/// 手动贴进浏览器一样能走完。
///
/// 打开浏览器和等回调是同时进行的。回调的监听要到开始等的那一刻才提供服务：
/// 要是先等"打开浏览器"返回再去监听，浏览器那边先一步访问回调就没人应答，永远卡住。
pub async fn login<F, Fut>(
    session: &Session,
    device: NewDevice,
    open: F,
    cancel: &CancellationToken,
) -> Result<Stored, LoginFailure>
where
    F: FnOnce(String) -> Fut,
    Fut: Future<Output = Result<(), BrowserError>>,
{
    let listener = LoginListener::bind().await?;
    let request = LoginRequest::generate(listener.redirect_uri(), device)?;
    let url = session
        .origin()
        .authorization_url(&request.encoded_request());
    eprintln!("正在打开系统浏览器完成登录。如果它没有打开，请手动打开这个地址：");
    eprintln!("  {url}");
    eprintln!("等你在这个助手的名字旁边点“确认登录”……（最多等 5 分钟，按 Ctrl+C 放弃）");

    let waiting = listener.wait(&request, WaitOptions::new(cancel, LOGIN_DEADLINE));
    let (opened, code) = tokio::join!(open(url.to_string()), waiting);
    if let Err(error) = opened {
        tracing::warn!(%error, "the system browser could not be opened");
    }
    let code = code?;
    let client = session.fresh_client()?;
    let (response, token) = client
        .exchange(
            &request.exchange_input(code),
            &RequestOptions {
                cancel,
                timeout: Some(REQUEST_TIMEOUT),
            },
        )
        .await?;
    let stored = Stored::from_exchange(response, token);
    if let Err(error) = session.save(&stored) {
        // 保存不了就不能用：撤销刚注册的设备，不留下一台没人认领的设备。
        if let Ok(signed_in) = session.client(&stored)
            && signed_in
                .logout(&RequestOptions {
                    cancel,
                    timeout: Some(REQUEST_TIMEOUT),
                })
                .await
                .is_err()
        {
            tracing::error!("a freshly registered assistant device could not be revoked");
        }
        return Err(LoginFailure::Save(error));
    }
    Ok(stored)
}

/// 退出没有完成。
#[derive(Debug, thiserror::Error)]
pub enum LogoutFailure {
    #[error(transparent)]
    Session(#[from] SessionError),
    /// 服务端没有确认撤销：本机的登录原样留着，联网之后重试。
    #[error("the service could not be reached: {0}")]
    Unreachable(ClientError),
    #[error("the client could not be built: {0}")]
    Build(#[from] ClientBuildError),
}

/// 退出：先让服务端撤销这台设备的令牌，确认之后才删掉本机保存的登录。
///
/// 返回有没有真的退掉一份登录：没登录过时什么都不做，返回 `false`，
/// 入口好如实说"本来就没有登录"，而不是说"已退出"。
pub async fn logout(session: &Session, cancel: &CancellationToken) -> Result<bool, LogoutFailure> {
    let Some(stored) = session.stored()? else {
        return Ok(false);
    };
    let client = session.client(&stored)?;
    match client
        .logout(&RequestOptions {
            cancel,
            timeout: Some(REQUEST_TIMEOUT),
        })
        .await
    {
        Ok(()) => {}
        // 服务端已经不认这台设备（比如它已经从账户里被移除）：同样可以忘掉。
        Err(error) if error.is_unauthorized() => {}
        Err(error) => return Err(LogoutFailure::Unreachable(error)),
    }
    session.clear()?;
    Ok(true)
}

/// 登录，给入口用的那一条：已经登录过的先退出，再登录。
///
/// 旧的登录没能撤销（连不上服务端）就不开始新的：免得账户里留下一台没人认领的设备。
/// 旧的登录已经失效（这台助手从账户里被移除了）不碍事，退出那一步会把它忘掉。
pub async fn sign_in_fresh<F, Fut>(
    session: &Session,
    device: NewDevice,
    open: F,
    cancel: &CancellationToken,
) -> Result<Stored, LoginFailure>
where
    F: FnOnce(String) -> Fut,
    Fut: Future<Output = Result<(), BrowserError>>,
{
    logout(session, cancel).await?;
    login(session, device, open, cancel).await
}

/// 现在的登录状态。
#[derive(Debug)]
pub enum Status {
    /// 从来没登录过。
    SignedOut,
    SignedIn {
        account: Account,
        device: Device,
    },
    /// 本机有登录，但服务端已经不认它：多半是这台助手从账户里被移除了。
    Expired {
        device: Device,
    },
    /// 本机有登录，只是现在连不上服务端，没法核对。登录原样留着。
    Unreachable {
        account: Account,
        device: Device,
    },
}

/// 核对一遍现在的登录状态。会真的问一次服务端。
pub async fn status(session: &Session, cancel: &CancellationToken) -> Result<Status, SessionError> {
    let Some(stored) = session.stored()? else {
        return Ok(Status::SignedOut);
    };
    let client = session.client(&stored)?;
    let check = client
        .me(&RequestOptions {
            cancel,
            timeout: Some(REQUEST_TIMEOUT),
        })
        .await;
    Ok(match check {
        Ok(me) => Status::SignedIn {
            account: me.account,
            device: me.device,
        },
        Err(error) if error.is_unauthorized() => Status::Expired {
            device: stored.device,
        },
        Err(_) => Status::Unreachable {
            account: stored.account,
            device: stored.device,
        },
    })
}

/// 一次调用要用到的：带着登录的客户端，以及这台助手是谁。
#[derive(Clone)]
pub struct SignedIn {
    pub client: Client,
    pub account: Account,
    pub device: Device,
}

// 客户端里带着令牌：那一份不打印，只写明它连的是哪个服务。
impl std::fmt::Debug for SignedIn {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("SignedIn")
            .field("client", &format_args!("for {}", self.client.origin()))
            .field("account", &self.account.username)
            .field("device", &self.device.name)
            .finish()
    }
}

/// 协议层从这里拿"已登录的客户端"。拿不到就是还没登录，或者登录已经被收回。
///
/// 协议层会把它交给别的线程去等：返回的 future 必须能跨线程，所以写明 `Send`。
pub trait SessionSource: Send + Sync + 'static {
    fn signed_in(&self) -> impl Future<Output = Result<SignedIn, SessionError>> + Send;
}

impl Session {
    /// 本机保存的登录，换成能用的客户端和"这台助手是谁"。
    fn signed_in_now(&self) -> Result<SignedIn, SessionError> {
        let stored = self.load()?;
        Ok(SignedIn {
            client: self.client(&stored)?,
            account: stored.account,
            device: stored.device,
        })
    }
}

impl SessionSource for Session {
    // 读钥匙串本来就是同步的：立刻给出结果，不装成异步的。
    fn signed_in(&self) -> impl Future<Output = Result<SignedIn, SessionError>> + Send {
        std::future::ready(self.signed_in_now())
    }
}

/// 进程里记着的一份登录：第一次用到时才去读钥匙串，之后复用同一份。
///
/// 助手的会话可以开很久，每一次工具调用都去读一遍钥匙串既慢，也更容易让系统弹确认框。
pub struct CachedSession {
    session: Session,
    cached: Mutex<Option<SignedIn>>,
}

impl CachedSession {
    pub fn new(session: Session) -> Self {
        Self {
            session,
            cached: Mutex::new(None),
        }
    }

    /// 放下手里的登录：退出登录之后用，下次用到时重新读钥匙串。
    pub fn forget(&self) {
        *self.cached.lock().unwrap_or_else(PoisonError::into_inner) = None;
    }
}

impl CachedSession {
    fn cached_or_load(&self) -> Result<SignedIn, SessionError> {
        if let Some(cached) = self
            .cached
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .as_ref()
        {
            return Ok(cached.clone());
        }
        let fresh = self.session.signed_in_now()?;
        *self.cached.lock().unwrap_or_else(PoisonError::into_inner) = Some(fresh.clone());
        Ok(fresh)
    }
}

impl SessionSource for CachedSession {
    // 读钥匙串本来就是同步的：立刻给出结果，不装成异步的。
    fn signed_in(&self) -> impl Future<Output = Result<SignedIn, SessionError>> + Send {
        std::future::ready(self.cached_or_load())
    }
}

#[cfg(test)]
mod tests {
    use std::sync::atomic::{AtomicUsize, Ordering};

    use super::*;
    use assemble_protocol::primitives::Platform;

    fn session(store: Arc<dyn SecretStore>, name: &str) -> Session {
        Session::new(ServerOrigin::development(), name, store)
    }

    fn signed_in(device_name: &str) -> Stored {
        let at = assemble_protocol::primitives::Timestamp::now();
        Stored {
            token: SecretString::new("a-secret-that-must-stay-in-the-store".to_owned()),
            account: Account {
                id: "account-1".into(),
                username: "alex".into(),
                display_name: "Alex".into(),
                created_at: at,
            },
            device: Device {
                id: "device-1".into(),
                name: device_name.into(),
                platform: Platform::Macos,
                online: false,
                last_seen_at: None,
                created_at: at,
            },
        }
    }

    #[test]
    fn a_session_round_trips_through_the_store_under_its_own_key() {
        let store = Arc::new(assemble_store::secret::MemorySecretStore::new());
        let mine = session(store.clone(), "ZCode 的助手");
        assert!(matches!(mine.load(), Err(SessionError::NotLoggedIn)));

        mine.save(&signed_in("ZCode 的助手")).unwrap();
        let back = mine.load().unwrap();
        assert_eq!(back.device.name, "ZCode 的助手");
        assert_eq!(back.token.expose(), "a-secret-that-must-stay-in-the-store");

        // 另一个助手、另一套服务来源，各有各的登录，互不干扰。
        let other = session(store.clone(), "Codex 的助手");
        assert!(matches!(other.load(), Err(SessionError::NotLoggedIn)));
        let elsewhere = Session::new(
            ServerOrigin::parse("https://as.example.com").unwrap(),
            "ZCode 的助手",
            store,
        );
        assert!(matches!(elsewhere.load(), Err(SessionError::NotLoggedIn)));

        mine.clear().unwrap();
        assert!(matches!(mine.load(), Err(SessionError::NotLoggedIn)));
        // 删一个不存在的条目不算错误。
        mine.clear().unwrap();
    }

    #[test]
    fn the_key_is_separate_from_the_desktop_app_s_and_says_which_service_it_belongs_to() {
        let origin = ServerOrigin::parse("https://as.example.com").unwrap();
        let key = key_for(&origin, "ZCode 的助手");
        assert_eq!(key.service, "assemble.mcp-session:https://as.example.com");
        assert_eq!(key.account, "ZCode 的助手");
        assert!(!key.service.starts_with("assemble.device-session:"));
    }

    #[test]
    fn a_broken_record_is_reported_without_echoing_its_content() {
        let store = Arc::new(assemble_store::secret::MemorySecretStore::new());
        let mine = session(store.clone(), "助手");
        let key = key_for(&ServerOrigin::development(), "助手");
        store
            .put(
                &key,
                &SecretString::new("{\"token\": \"not-a-session\"}".to_owned()),
            )
            .unwrap();
        let error = mine.load().unwrap_err();
        assert!(matches!(error, SessionError::Corrupt));
        assert!(!error.to_string().contains("not-a-session"));
    }

    #[test]
    fn the_stored_value_is_one_secret_that_contains_the_token() {
        let store = Arc::new(assemble_store::secret::MemorySecretStore::new());
        let mine = session(store.clone(), "助手");
        mine.save(&signed_in("助手")).unwrap();
        let key = key_for(&ServerOrigin::development(), "助手");
        let raw = store.get(&key).unwrap().expect("a stored value");
        assert!(
            raw.expose()
                .contains("a-secret-that-must-stay-in-the-store")
        );
        assert!(raw.expose().contains("\"name\":\"助手\""));
    }

    #[test]
    fn names_follow_the_rules_for_device_names() {
        assert_eq!(name_of(None).unwrap(), DEFAULT_NAME);
        assert_eq!(name_of(Some("  ZCode 的助手 ")).unwrap(), "ZCode 的助手");
        assert!(name_of(Some("   ")).is_err());
        assert!(name_of(Some(&"长".repeat(61))).is_err());
        assert!(name_of(Some("a\u{1}b")).is_err());
    }

    /// 记着钥匙串被读了多少次的内存存储。
    #[derive(Default)]
    struct CountingStore {
        inner: assemble_store::secret::MemorySecretStore,
        reads: AtomicUsize,
    }

    impl SecretStore for CountingStore {
        fn put(&self, key: &SecretKey, value: &SecretString) -> assemble_store::secret::Result<()> {
            self.inner.put(key, value)
        }

        fn get(&self, key: &SecretKey) -> assemble_store::secret::Result<Option<SecretString>> {
            self.reads.fetch_add(1, Ordering::Relaxed);
            self.inner.get(key)
        }

        fn delete(&self, key: &SecretKey) -> assemble_store::secret::Result<()> {
            self.inner.delete(key)
        }
    }

    #[tokio::test]
    async fn a_cached_session_reads_the_store_once_until_told_to_start_over() {
        let store = Arc::new(CountingStore::default());
        let plain = session(store.clone(), "助手");
        plain.save(&signed_in("助手")).unwrap();
        let cached = CachedSession::new(session(store.clone(), "助手"));

        let first = cached.signed_in().await.unwrap();
        let second = cached.signed_in().await.unwrap();
        assert!(first.client.has_bearer() && second.client.has_bearer());
        assert_eq!(first.device.name, "助手");
        assert_eq!(first.account.username, "alex");
        assert_eq!(store.reads.load(Ordering::Relaxed), 1);

        cached.forget();
        cached.signed_in().await.unwrap();
        assert_eq!(store.reads.load(Ordering::Relaxed), 2);

        plain.clear().unwrap();
        cached.forget();
        assert!(matches!(
            cached.signed_in().await,
            Err(SessionError::NotLoggedIn)
        ));
    }

    #[tokio::test]
    async fn without_a_stored_session_there_is_nothing_to_sign_in_with() {
        let store = Arc::new(assemble_store::secret::MemorySecretStore::new());
        let plain = session(store.clone(), "助手");
        assert!(matches!(
            plain.signed_in().await,
            Err(SessionError::NotLoggedIn)
        ));
        plain.save(&signed_in("助手")).unwrap();
        let signed = plain.signed_in().await.unwrap();
        assert!(signed.client.has_bearer());
        assert_eq!(signed.device.name, "助手");
    }
}
