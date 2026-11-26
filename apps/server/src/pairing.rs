//! 手机扫码登录的服务端一半。线上的类型在 [`assemble_protocol::pairing`]，那边的模块注释
//! 讲了整条路为什么这样走。这里管的是：网页上的配对页、两个谁都能调的接口、限速和清理。
//!
//! 网页这一侧不用脚本：等手机来扫的那一段靠页面自己刷新；二维码是页面里的一段内联
//! 标记，页面共用的内容安全策略不用放宽。令牌只在网页上的人点了"允许"之后才发给手机，
//! 而且手机必须拿对 PKCE 的校验器。
//!
//! 票据在库里只存摘要，冒领不了；可页面每两秒刷新一次，刷新时要把二维码重画出来，
//! 所以还没被认领的票据另在这个进程的内存里留 [`PAIRING_TTL`] 这么久。服务端在那之内
//! 重启过，这一张就作废：页面上会说重新生成。

use std::collections::HashMap;
use std::fmt::{self, Write};
use std::sync::{Mutex, PoisonError};
use std::time::{Duration, Instant};

use assemble_protocol::device::NewDevice;
use assemble_protocol::error::codes;
use assemble_protocol::pairing::{
    CLAIM_PATH, FINISH_PATH, PairingClaim, PairingClaimed, PairingFinish, PairingOutcome,
    PairingState, pairing_uri,
};
use assemble_protocol::primitives::{Platform, Timestamp, is_valid_id};
use axum::Json;
use axum::body::Bytes;
use axum::extract::{Form, Path, State};
use axum::http::{HeaderMap, StatusCode};
use axum::response::{IntoResponse, Redirect, Response};
use axum_extra::extract::CookieJar;
use serde::Deserialize;
use subtle::ConstantTimeEq;

use crate::audit::{self, actions};
use crate::auth::{WebSession, is_same_origin, web_session};
use crate::desktop_login::{pkce_matches, register};
use crate::error::ApiError;
use crate::http::Shared;
use crate::secrets::{random_id, random_token, token_digest};
use crate::source::Source;
use crate::templates;
use crate::web::{page, query_param, refused, retry_after, to_login, unavailable};

/// 网页上配对页的路径。
pub const PAIRING_PATH: &str = "/web/pairing";

/// 一次配对从生成二维码起这么久没走完就作废。
pub const PAIRING_TTL: Duration = Duration::from_secs(180);

/// 等手机来扫的那一段，页面自己刷新的间隔（秒）。
const REFRESH_SECONDS: u64 = 2;

/// 每个账户同时进行的配对最多这么多：二维码一张一张地生成，攒着没用。
const MAX_ACTIVE: i64 = 5;

/// 限速的窗口，和两个接口各自在这段时间里的次数。它们谁都能调，所以按来源分；
/// 问结果的那个，手机两秒问一次，留出三倍的余量。
const THROTTLE_WINDOW: Duration = Duration::from_secs(60);
const CLAIM: &str = "claim";
const CLAIM_BUDGET: usize = 10;
const FINISH: &str = "finish";
const FINISH_BUDGET: usize = 90;

/// 这个进程里的配对现场：还没被认领的票据，和限速的记录。
#[derive(Default)]
pub struct Pairings {
    /// `id` 到（票据，生成的时刻）。票据是凭据：不进库、不进日志，也不进调试输出。
    tickets: Mutex<HashMap<String, (String, Instant)>>,
    /// （接口，来源）到请求的时刻。来源未知的不记：那时没法把请求分到各自的来源上，
    /// 记在一起等于让一个乱试的人挡住所有人。凭据本身是 32 字节的随机数，猜不出来。
    seen: Mutex<HashMap<(&'static str, String), Vec<Instant>>>,
}

impl fmt::Debug for Pairings {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let pending = self.tickets.lock().map_or(0, |tickets| tickets.len());
        f.debug_struct("Pairings")
            .field("pending", &pending)
            .finish_non_exhaustive()
    }
}

impl Pairings {
    /// 刚生成了一张票据，记到内存里。
    fn remember(&self, id: &str, ticket: &str, now: Instant) {
        let mut tickets = self.tickets.lock().unwrap_or_else(PoisonError::into_inner);
        tickets.retain(|_, (_, at)| now.saturating_duration_since(*at) < PAIRING_TTL);
        tickets.insert(id.to_owned(), (ticket.to_owned(), now));
    }

    /// 还没到点的一张票据。
    fn ticket(&self, id: &str, now: Instant) -> Option<String> {
        let mut tickets = self.tickets.lock().unwrap_or_else(PoisonError::into_inner);
        tickets.retain(|_, (_, at)| now.saturating_duration_since(*at) < PAIRING_TTL);
        tickets.get(id).map(|(ticket, _)| ticket.clone())
    }

    /// 这个来源现在能不能再调一次这个接口；能就记下这一次。见字段上的说明。
    fn admit(&self, endpoint: &'static str, source: &Source, budget: usize, now: Instant) -> bool {
        if source.is_unknown() {
            return true;
        }
        let mut seen = self.seen.lock().unwrap_or_else(PoisonError::into_inner);
        seen.retain(|_, times| {
            times
                .last()
                .is_some_and(|last| now.saturating_duration_since(*last) < THROTTLE_WINDOW)
        });
        let times = seen.entry((endpoint, source.key().to_owned())).or_default();
        times.retain(|time| now.saturating_duration_since(*time) < THROTTLE_WINDOW);
        if times.len() >= budget {
            return false;
        }
        times.push(now);
        true
    }
}

/// 一次配对走到了哪一步。名字就是库里的拼法。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Stage {
    /// 二维码生成好了，等手机来扫。
    Pending,
    /// 手机认领了，等网页上的人决定。
    Claimed,
    Approved,
    Denied,
    /// 手机取走了令牌，或者拿错的校验器试过一次：这张票据到此为止。
    Consumed,
}

impl Stage {
    fn parse(stored: &str) -> Option<Self> {
        match stored {
            "pending" => Some(Self::Pending),
            "claimed" => Some(Self::Claimed),
            "approved" => Some(Self::Approved),
            "denied" => Some(Self::Denied),
            "consumed" => Some(Self::Consumed),
            _ => None,
        }
    }

    fn stored(self) -> &'static str {
        match self {
            Self::Pending => "pending",
            Self::Claimed => "claimed",
            Self::Approved => "approved",
            Self::Denied => "denied",
            Self::Consumed => "consumed",
        }
    }
}

/// 库里一次配对的几个字段。
#[derive(Debug, sqlx::FromRow)]
struct PairingRow {
    id: String,
    account_id: String,
    code_challenge: Option<String>,
    device_name: Option<String>,
    confirmation_code: Option<String>,
    state: String,
    expires_at: i64,
}

impl PairingRow {
    async fn load(
        pool: &sqlx::SqlitePool,
        account_id: &str,
        id: &str,
    ) -> Result<Option<Self>, sqlx::Error> {
        sqlx::query_as::<_, Self>("SELECT * FROM device_pairings WHERE account_id = ? AND id = ?")
            .bind(account_id)
            .bind(id)
            .fetch_optional(pool)
            .await
    }

    /// 网页这一侧此刻该显示什么。
    fn view(&self, now: i64) -> Option<View> {
        let in_time = self.expires_at > now;
        let name = || self.device_name.clone().unwrap_or_default();
        Some(match Stage::parse(&self.state)? {
            Stage::Pending if in_time => View::Waiting,
            Stage::Claimed if in_time => View::Confirm {
                device_name: name(),
                code: self.confirmation_code.clone().unwrap_or_default(),
            },
            // 允许了但手机一直没来取：过了时限一样作废，重新扫。
            Stage::Approved if in_time => View::Approved {
                device_name: name(),
            },
            Stage::Consumed => View::Consumed {
                device_name: name(),
            },
            Stage::Denied => View::Denied,
            Stage::Pending | Stage::Claimed | Stage::Approved => View::Expired,
        })
    }
}

/// 网页这一侧能看到的页面。
enum View {
    /// 等手机来扫。二维码要现画，票据在内存里。
    Waiting,
    Confirm {
        device_name: String,
        code: String,
    },
    Approved {
        device_name: String,
    },
    Consumed {
        device_name: String,
    },
    Denied,
    Expired,
}

/// 一次配对的二维码，画成内联的 SVG。只用属性，不用内联样式：页面的策略不放行内联样式。
fn qr_svg(text: &str) -> Option<String> {
    let code = qrcodegen::QrCode::encode_text(text, qrcodegen::QrCodeEcc::Medium).ok()?;
    let modules = usize::try_from(code.size()).ok()?;
    // 四格静区是扫码的要求；白底不随页面的深浅色变，不然深色模式下可能扫不出来。
    let quiet = 4;
    let scale = 6;
    let mut dots = String::new();
    for y in 0..modules {
        for x in 0..modules {
            if code.get_module(i32::try_from(x).ok()?, i32::try_from(y).ok()?) {
                write!(
                    dots,
                    "<rect x=\"{}\" y=\"{}\" width=\"{scale}\" height=\"{scale}\"/>",
                    (x + quiet) * scale,
                    (y + quiet) * scale,
                )
                .ok()?;
            }
        }
    }
    let pixels = (modules + quiet * 2) * scale;
    Some(format!(
        "<svg xmlns=\"http://www.w3.org/2000/svg\" viewBox=\"0 0 {pixels} {pixels}\" width=\"{pixels}\" height=\"{pixels}\" shape-rendering=\"crispEdges\" role=\"img\" aria-label=\"配对二维码\"><rect width=\"{pixels}\" height=\"{pixels}\" fill=\"#ffffff\"/><g fill=\"#000000\">{dots}</g></svg>"
    ))
}

/// 六位的确认码，前面可以有 0。拒绝采样，免得头上的数字略多一点。
fn confirmation_code() -> Result<String, ApiError> {
    let mut bytes = [0u8; 4];
    loop {
        getrandom::fill(&mut bytes)
            .map_err(|_| ApiError::internal("a random value could not be generated"))?;
        let value = u32::from_be_bytes(bytes);
        if value < 4_000_000_000 {
            return Ok(format!("{:06}", value % 1_000_000));
        }
    }
}

async fn active_count(
    pool: &sqlx::SqlitePool,
    account_id: &str,
    now: i64,
) -> Result<i64, sqlx::Error> {
    sqlx::query_scalar(
        "SELECT COUNT(*) FROM device_pairings \
         WHERE account_id = ? AND expires_at > ? AND state IN ('pending', 'claimed')",
    )
    .bind(account_id)
    .bind(now)
    .fetch_one(pool)
    .await
}

fn invalid_code() -> ApiError {
    ApiError::new(
        StatusCode::BAD_REQUEST,
        codes::INVALID_AUTH_CODE,
        "the pairing ticket is invalid, already used or expired",
    )
}

fn too_many() -> Response {
    retry_after(
        ApiError::new(
            StatusCode::TOO_MANY_REQUESTS,
            codes::TOO_MANY_ATTEMPTS,
            "too many pairing attempts from this address",
        )
        .into_response(),
        THROTTLE_WINDOW.as_secs(),
    )
}

fn pairing_page(title: &str, body: &str, success: bool) -> Response {
    page(
        StatusCode::OK,
        templates::pairing_over_page(PAIRING_PATH, title, body, success),
    )
}

fn expired() -> Response {
    pairing_page(
        "这次配对已作废",
        "二维码生成之后三分钟内没走完，或者服务重启过。想添加手机，重新生成一张。",
        false,
    )
}

/// 网页一侧的三道门：来自本站、登录着、防伪值对得上。邮箱要验证过，和桌面授权一样。
/// 没过的话交出该对用户说的那一句话，由调用方回成页面。
async fn gate(
    state: &Shared,
    jar: &CookieJar,
    headers: &HeaderMap,
    csrf: &str,
) -> Result<WebSession, &'static str> {
    if !is_same_origin(headers, state.config.public_origin.as_str()) {
        return Err("请回到账户页重新操作。");
    }
    let Some(session) = web_session(&state.pool, jar).await else {
        return Err("登录状态已失效，请重新登录。");
    };
    if session.email.is_none() {
        return Err("请先验证邮箱，再添加手机。");
    }
    let paired = session.csrf_token.len() == csrf.len()
        && bool::from(session.csrf_token.as_bytes().ct_eq(csrf.as_bytes()));
    if !paired {
        return Err("页面可能已经过期，请回到账户页重新操作。");
    }
    Ok(session)
}

#[derive(Debug, Deserialize)]
pub struct CsrfForm {
    csrf: String,
}

/// `GET /web/pairing`：没带 `id` 是第一页；带了就是看那一次配对走到了哪一步。
pub async fn show(State(state): State<Shared>, jar: CookieJar, uri: axum::http::Uri) -> Response {
    let Some(session) = web_session(&state.pool, &jar).await else {
        return to_login(Some(crate::account::ACCOUNT_PATH));
    };
    let Some(id) = query_param(&uri, "id").filter(|id| is_valid_id(id)) else {
        return page(
            StatusCode::OK,
            templates::pairing_start_page(PAIRING_PATH, &session.csrf_token, None),
        );
    };
    let row = match PairingRow::load(&state.pool, &session.account_id, &id).await {
        Ok(row) => row,
        Err(error) => {
            tracing::error!(%error, "a pairing could not be read");
            return unavailable();
        }
    };
    let Some(row) = row else {
        return pairing_page(
            "找不到这次配对",
            "它不属于这个账户，或者从来没有过。想添加手机，重新生成一张二维码。",
            false,
        );
    };
    let Some(view) = row.view(Timestamp::now().as_millis()) else {
        return unavailable();
    };
    let body = match view {
        View::Waiting => {
            let drawn = state
                .pairings
                .ticket(&id, Instant::now())
                .and_then(|ticket| qr_svg(&pairing_uri(&ticket)).map(|qr| (ticket, qr)));
            let Some((ticket, qr)) = drawn else {
                return expired();
            };
            templates::pairing_wait_page(
                PAIRING_PATH,
                &id,
                &session.csrf_token,
                &qr,
                &pairing_uri(&ticket),
                REFRESH_SECONDS,
            )
        }
        View::Confirm { device_name, code } => templates::pairing_confirm_page(
            PAIRING_PATH,
            &id,
            &session.csrf_token,
            &device_name,
            &code,
        ),
        View::Approved { device_name } => {
            return pairing_page(
                "已允许",
                &format!("{device_name} 可以登录了。回到手机上继续。"),
                true,
            );
        }
        View::Consumed { device_name } => {
            return pairing_page(
                "添加好了",
                &format!(
                    "{device_name} 已经登录这个账户，在账户的设备名单里。手机第一期只能看日程与提醒，不能收文件。"
                ),
                true,
            );
        }
        View::Denied => {
            return pairing_page(
                "已结束",
                "手机那边登不进来。想再试，重新生成一张二维码。",
                false,
            );
        }
        View::Expired => return expired(),
    };
    page(StatusCode::OK, body)
}

/// `POST /web/pairing`：生成一张新的票据。
pub async fn create(
    State(state): State<Shared>,
    jar: CookieJar,
    headers: HeaderMap,
    Form(form): Form<CsrfForm>,
) -> Response {
    let session = match gate(&state, &jar, &headers, &form.csrf).await {
        Ok(session) => session,
        Err(reason) => return refused(reason),
    };
    let now = Timestamp::now().as_millis();
    let active = match active_count(&state.pool, &session.account_id, now).await {
        Ok(active) => active,
        Err(error) => {
            tracing::error!(%error, "pairings could not be counted");
            return unavailable();
        }
    };
    if active >= MAX_ACTIVE {
        return retry_after(
            page(
                StatusCode::TOO_MANY_REQUESTS,
                templates::pairing_start_page(
                    PAIRING_PATH,
                    &session.csrf_token,
                    Some(
                        "同时进行的配对太多了。等它们作废（三分钟），或者回到已经打开的配对页取消。",
                    ),
                ),
            ),
            PAIRING_TTL.as_secs(),
        );
    }
    let ticket = match random_token() {
        Ok(ticket) => ticket,
        Err(error) => {
            tracing::error!(%error, "a pairing ticket could not be generated");
            return unavailable();
        }
    };
    let id = match random_id() {
        Ok(id) => id,
        Err(error) => {
            tracing::error!(%error, "a pairing id could not be generated");
            return unavailable();
        }
    };
    let ttl = i64::try_from(PAIRING_TTL.as_millis()).unwrap_or(i64::MAX);
    let stored = sqlx::query(
        "INSERT INTO device_pairings (id, account_id, ticket_hash, state, created_at, expires_at) \
         VALUES (?, ?, ?, 'pending', ?, ?)",
    )
    .bind(&id)
    .bind(&session.account_id)
    .bind(token_digest(ticket.expose()))
    .bind(now)
    .bind(now + ttl)
    .execute(&state.pool)
    .await;
    if let Err(error) = stored {
        tracing::error!(%error, "a pairing could not be created");
        return unavailable();
    }
    state
        .pairings
        .remember(&id, ticket.expose(), Instant::now());
    Redirect::to(&format!("{PAIRING_PATH}?id={id}")).into_response()
}

#[derive(Debug, Deserialize)]
pub struct DecideForm {
    csrf: String,
    /// `allow`、`deny`，或者还没被认领时的 `cancel`。
    choice: String,
}

/// 这一次提交算不算数：算的话，交出（要求它现在所处的状态，改成什么，记哪一笔审计）。
fn decision(
    stored: &str,
    choice: &str,
    in_time: bool,
) -> Option<(&'static str, &'static str, &'static str)> {
    match (Stage::parse(stored), choice, in_time) {
        (Some(Stage::Claimed), "allow", true) => Some((
            Stage::Claimed.stored(),
            Stage::Approved.stored(),
            actions::PAIRING_ALLOWED,
        )),
        (Some(Stage::Claimed), "deny", true) => Some((
            Stage::Claimed.stored(),
            Stage::Denied.stored(),
            actions::PAIRING_DENIED,
        )),
        (Some(Stage::Pending), "cancel", _) => Some((
            Stage::Pending.stored(),
            Stage::Denied.stored(),
            actions::PAIRING_DENIED,
        )),
        _ => None,
    }
}

/// `POST /web/pairing/{id}`
pub async fn decide(
    State(state): State<Shared>,
    jar: CookieJar,
    headers: HeaderMap,
    Path(id): Path<String>,
    Form(form): Form<DecideForm>,
) -> Response {
    let session = match gate(&state, &jar, &headers, &form.csrf).await {
        Ok(session) => session,
        Err(reason) => return refused(reason),
    };
    if !is_valid_id(&id) {
        return pairing_page("找不到这次配对", "想添加手机，重新生成一张二维码。", false);
    }
    let row = match PairingRow::load(&state.pool, &session.account_id, &id).await {
        Ok(row) => row,
        Err(error) => {
            tracing::error!(%error, "a pairing could not be read");
            return unavailable();
        }
    };
    let Some(row) = row else {
        return pairing_page(
            "找不到这次配对",
            "它不属于这个账户，或者从来没有过。想添加手机，重新生成一张二维码。",
            false,
        );
    };
    let now = Timestamp::now().as_millis();
    let back = Redirect::to(&format!("{PAIRING_PATH}?id={id}")).into_response();
    let Some((expected, next, action)) = decision(&row.state, &form.choice, row.expires_at > now)
    else {
        // 已经决定过了，或者过了时限：回到页面上看它现在的样子。
        return back;
    };
    let changed = sqlx::query(
        "UPDATE device_pairings SET state = ?, decided_at = ? WHERE id = ? AND state = ?",
    )
    .bind(next)
    .bind(now)
    .bind(&id)
    .bind(expected)
    .execute(&state.pool)
    .await;
    match changed {
        Ok(done) if done.rows_affected() == 1 => {
            audit::record(
                &state.pool,
                Some(&session.account_id),
                None,
                action,
                row.device_name.as_deref(),
            )
            .await;
        }
        // 另一个标签页已经替这次提交做了决定：不重复记账。
        Ok(_) => {}
        Err(error) => {
            tracing::error!(%error, "a pairing could not be decided");
            return unavailable();
        }
    }
    back
}

/// `POST /v1/pairing/claim`：手机认领一张票据。
pub async fn claim(State(state): State<Shared>, headers: HeaderMap, body: Bytes) -> Response {
    let input: PairingClaim = match crate::http::read_json(&body) {
        Ok(input) => input,
        Err(error) => return error.into_response(),
    };
    if !input.is_valid() {
        return invalid_code().into_response();
    }
    let source = Source::read(&headers, state.config.client_ip_header.as_ref());
    if !state
        .pairings
        .admit(CLAIM, &source, CLAIM_BUDGET, Instant::now())
    {
        return too_many();
    }
    let code = match confirmation_code() {
        Ok(code) => code,
        Err(error) => return error.into_response(),
    };
    let now = Timestamp::now().as_millis();
    let ticket_hash = token_digest(&input.ticket);
    let claimed: Result<Option<i64>, ApiError> = async {
        let mut tx = state.pool.begin_with("BEGIN IMMEDIATE").await?;
        let found: Option<(String, i64)> = sqlx::query_as(
            "SELECT id, expires_at FROM device_pairings \
             WHERE ticket_hash = ? AND state = 'pending' AND expires_at > ?",
        )
        .bind(&ticket_hash)
        .bind(now)
        .fetch_optional(&mut *tx)
        .await?;
        let Some((id, expires_at)) = found else {
            tx.commit().await?;
            return Ok(None);
        };
        sqlx::query(
            "UPDATE device_pairings \
             SET state = 'claimed', code_challenge = ?, device_name = ?, confirmation_code = ? \
             WHERE id = ? AND state = 'pending'",
        )
        .bind(&input.code_challenge)
        .bind(input.device_name.trim())
        .bind(&code)
        .bind(id)
        .execute(&mut *tx)
        .await?;
        tx.commit().await?;
        Ok(Some(expires_at))
    }
    .await;
    match claimed {
        Ok(Some(expires_at)) => Json(PairingClaimed {
            confirmation_code: code,
            expires_at: Timestamp::from_millis(expires_at).unwrap_or_else(Timestamp::now),
        })
        .into_response(),
        Ok(None) => {
            tracing::info!("pairing claim rejected");
            invalid_code().into_response()
        }
        Err(error) => error.into_response(),
    }
}

/// 手机来问结果时，这一次配对的结局。
enum Settled {
    /// 票据不对、过期、已经用过，或者校验器拿错了。
    Invalid,
    Waiting,
    Denied,
    Issued {
        account_id: String,
        device: crate::devices::DeviceRow,
        token: assemble_store::secret::SecretString,
    },
}

/// 在同一个事务里定这一次的结局。允许了就把设备登记进去：登记与票据作废要么都发生，要么都不。
async fn settle(
    state: &Shared,
    ticket_hash: &str,
    verifier: &str,
    now: i64,
) -> Result<Settled, ApiError> {
    let mut tx = state.pool.begin_with("BEGIN IMMEDIATE").await?;
    let row: Option<PairingRow> =
        sqlx::query_as("SELECT * FROM device_pairings WHERE ticket_hash = ?")
            .bind(ticket_hash)
            .fetch_optional(&mut *tx)
            .await?;
    let Some(row) = row else {
        tx.commit().await?;
        return Ok(Settled::Invalid);
    };
    let Some(step) = Stage::parse(&row.state) else {
        tx.commit().await?;
        return Ok(Settled::Invalid);
    };
    if step == Stage::Consumed {
        tx.commit().await?;
        return Ok(Settled::Invalid);
    }
    // 拿错了校验器：无论到哪一步，这张票据到此为止，不能拿别的校验器再试第二次。
    let challenge = row.code_challenge.as_deref().unwrap_or_default();
    if !pkce_matches(challenge, verifier) {
        sqlx::query("UPDATE device_pairings SET state = 'consumed' WHERE id = ?")
            .bind(&row.id)
            .execute(&mut *tx)
            .await?;
        tx.commit().await?;
        return Ok(Settled::Invalid);
    }
    let settled = match step {
        Stage::Claimed => Settled::Waiting,
        Stage::Denied => Settled::Denied,
        // 允许了但手机拖着没来取，过了时限：和没允许一样，重新扫。
        Stage::Approved if row.expires_at > now => {
            let token = random_token().map_err(|error| ApiError::internal(error.to_string()))?;
            let device = register(
                &mut tx,
                &row.account_id,
                &NewDevice {
                    name: row.device_name.clone().unwrap_or_default(),
                    platform: Platform::Iphone,
                },
                &token_digest(token.expose()),
            )
            .await?;
            sqlx::query("UPDATE device_pairings SET state = 'consumed' WHERE id = ?")
                .bind(&row.id)
                .execute(&mut *tx)
                .await?;
            Settled::Issued {
                account_id: row.account_id,
                device,
                token,
            }
        }
        Stage::Pending | Stage::Approved | Stage::Consumed => Settled::Invalid,
    };
    tx.commit().await?;
    Ok(settled)
}

/// `POST /v1/pairing/finish`：手机拿着校验器来问结果，没决定之前会一直问。
pub async fn finish(State(state): State<Shared>, headers: HeaderMap, body: Bytes) -> Response {
    let input: PairingFinish = match crate::http::read_json(&body) {
        Ok(input) => input,
        Err(error) => return error.into_response(),
    };
    if !input.is_valid() {
        return invalid_code().into_response();
    }
    let source = Source::read(&headers, state.config.client_ip_header.as_ref());
    if !state
        .pairings
        .admit(FINISH, &source, FINISH_BUDGET, Instant::now())
    {
        return too_many();
    }
    let settled = settle(
        &state,
        &token_digest(&input.ticket),
        &input.code_verifier,
        Timestamp::now().as_millis(),
    )
    .await;
    match settled {
        Ok(Settled::Invalid) => {
            tracing::info!("pairing finish rejected");
            invalid_code().into_response()
        }
        Ok(Settled::Waiting) => Json(PairingOutcome::pending()).into_response(),
        Ok(Settled::Denied) => Json(PairingOutcome::denied()).into_response(),
        Ok(Settled::Issued {
            account_id,
            device,
            token,
        }) => {
            let account = match crate::accounts::find_by_id(&state.pool, &account_id).await {
                Ok(Some(account)) => account,
                Ok(None) => return ApiError::unauthorized().into_response(),
                Err(error) => return ApiError::from(error).into_response(),
            };
            audit::record(
                &state.pool,
                Some(&account_id),
                Some(&device.id),
                actions::DEVICE_REGISTERED,
                Some(&device.name),
            )
            .await;
            // 一台新设备出现在账户里。
            crate::devices::broadcast_changed(&state, &account_id).await;
            tracing::info!(device = %device.name, "phone paired");
            Json(PairingOutcome {
                state: PairingState::Complete,
                account: Some(account.public()),
                device: Some(crate::devices::to_device(device, false)),
                token: Some(token.expose().to_owned()),
            })
            .into_response()
        }
        Err(error) => error.into_response(),
    }
}

/// 清掉作废的配对。每一轮维护跑一次。
pub async fn sweep(pool: &sqlx::SqlitePool, now: i64) -> Result<u64, sqlx::Error> {
    Ok(
        sqlx::query("DELETE FROM device_pairings WHERE expires_at < ?")
            .bind(now)
            .execute(pool)
            .await?
            .rows_affected(),
    )
}

/// 网页一侧的路由：配对页。
pub fn routes() -> axum::Router<Shared> {
    axum::Router::new()
        .route(PAIRING_PATH, axum::routing::get(show).post(create))
        .route("/web/pairing/{id}", axum::routing::post(decide))
}

/// 手机一侧的路由：两个谁都能调的接口。
pub fn device_routes() -> axum::Router<Shared> {
    axum::Router::new()
        .route(CLAIM_PATH, axum::routing::post(claim))
        .route(FINISH_PATH, axum::routing::post(finish))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn key(source: &str) -> Source {
        Source::read(
            &HeaderMap::from_iter([(
                axum::http::header::HeaderName::from_static("x-real-ip"),
                axum::http::HeaderValue::from_str(source).unwrap(),
            )]),
            Some(&axum::http::header::HeaderName::from_static("x-real-ip")),
        )
    }

    #[test]
    fn the_qr_code_is_an_inline_drawing_on_a_white_field() {
        let svg = qr_svg("assemble://pair?ticket=abcdefghijklmnopqrstuvwxyz0123456789-_ABCDE")
            .expect("a short address always fits");
        assert!(svg.starts_with("<svg xmlns=\"http://www.w3.org/2000/svg\""));
        assert!(svg.contains("fill=\"#ffffff\""));
        // 只有黑白两色和位置，没有任何脚本或样式。
        assert!(!svg.contains("<script"));
        assert!(!svg.contains("style="));
        assert_eq!(
            svg.matches("<rect").count(),
            1 + svg.matches("width=\"6\"").count()
        );
        // 加上两边各四格静区、每格六像素之后，是一个正方形的观景框；编码的边长是 21 + 4k 格。
        let (width, height) = svg
            .split("viewBox=\"0 0 ")
            .nth(1)
            .and_then(|rest| rest.split('"').next())
            .and_then(|side| side.split_once(' '))
            .expect("a viewBox");
        assert_eq!(width, height);
        let modules = width.parse::<usize>().unwrap() / 6 - 8;
        assert!(
            (21..=57).contains(&modules) && (modules - 21) % 4 == 0,
            "{svg}"
        );
    }

    #[test]
    fn a_text_too_long_for_a_qr_code_is_refused_rather_than_drawn_wrong() {
        assert!(qr_svg(&"a".repeat(3_000)).is_none());
    }

    #[test]
    fn confirmation_codes_are_six_digits_with_leading_zeros_allowed() {
        for _ in 0..200 {
            let code = confirmation_code().unwrap();
            assert_eq!(code.len(), 6);
            assert!(code.bytes().all(|byte| byte.is_ascii_digit()));
        }
    }

    #[test]
    fn tickets_live_in_memory_only_until_they_expire() {
        let pairings = Pairings::default();
        let start = Instant::now();
        pairings.remember("id-1", "ticket-1", start);
        assert_eq!(pairings.ticket("id-1", start).as_deref(), Some("ticket-1"));
        assert_eq!(pairings.ticket("id-2", start), None);
        let late = start + PAIRING_TTL;
        assert_eq!(pairings.ticket("id-1", late), None);
        // 票据不出现在调试输出里。
        pairings.remember("id-1", "a-secret-ticket", start);
        let shown = format!("{pairings:?}");
        assert!(!shown.contains("a-secret-ticket"));
        assert!(shown.contains("pending: 1"));
    }

    #[test]
    fn only_decisions_that_match_the_current_stage_count() {
        let allowed = decision("claimed", "allow", true).expect("the owner may allow");
        assert_eq!(allowed, ("claimed", "approved", actions::PAIRING_ALLOWED));
        let denied = decision("claimed", "deny", true).expect("the owner may deny");
        assert_eq!(denied, ("claimed", "denied", actions::PAIRING_DENIED));
        let cancelled =
            decision("pending", "cancel", true).expect("an unclaimed code may be cancelled");
        assert_eq!(cancelled, ("pending", "denied", actions::PAIRING_DENIED));
        // 过了时限的不能再用；没被认领的不能"允许"；决定过的不能改。
        assert!(decision("claimed", "allow", false).is_none());
        assert!(decision("pending", "allow", true).is_none());
        assert!(decision("approved", "deny", true).is_none());
        assert!(decision("consumed", "allow", true).is_none());
        assert!(decision("something-else", "allow", true).is_none());
    }

    #[test]
    fn each_source_gets_its_own_budget_and_unknown_sources_are_not_counted() {
        let pairings = Pairings::default();
        let now = Instant::now();
        for _ in 0..CLAIM_BUDGET {
            assert!(pairings.admit(CLAIM, &key("203.0.113.7"), CLAIM_BUDGET, now));
        }
        assert!(!pairings.admit(CLAIM, &key("203.0.113.7"), CLAIM_BUDGET, now));
        // 另一个来源不受影响；同一个来源的另一个接口各算各的。
        assert!(pairings.admit(CLAIM, &key("203.0.113.8"), CLAIM_BUDGET, now));
        assert!(pairings.admit(FINISH, &key("203.0.113.7"), FINISH_BUDGET, now));
        // 窗口过去之后恢复。
        assert!(pairings.admit(
            CLAIM,
            &key("203.0.113.7"),
            CLAIM_BUDGET,
            now + THROTTLE_WINDOW
        ));
        for _ in 0..1_000 {
            assert!(pairings.admit(CLAIM, &Source::unknown(), CLAIM_BUDGET, now));
        }
    }
}
