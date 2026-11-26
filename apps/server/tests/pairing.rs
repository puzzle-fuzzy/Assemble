//! 手机扫码登录的集成测试：对真实运行的服务走完整条链路。
//!
//! 网页那一侧是会存 Cookie 的"浏览器"，手机那一侧是一个不带任何 Cookie 的 HTTP 客户端。
//! 这里核对的是几方拼起来的行为：令牌是不是只在本人点了"允许"之后才发出去、
//! 拿错的校验器能不能再试第二次、别的账户能不能替人决定。

mod common;

use assemble_protocol::pairing::{
    CLAIM_PATH, FINISH_PATH, PairingClaim, PairingFinish, pairing_uri, ticket_from_uri,
};
use assemble_protocol::primitives::Platform;
use assemble_store::secret::SecretString;
use base64::Engine as _;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use common::*;
use reqwest::StatusCode;
use serde_json::{Value, json};
use sha2::{Digest, Sha256};

const PHONE_NAME: &str = "我的 iPhone";

// ---- 工具 ----

/// 一部"手机"：不带任何 Cookie，手里只有自己的 PKCE 校验器。
struct Phone {
    http: reqwest::Client,
    verifier: String,
}

impl Phone {
    fn new(seed: char) -> Self {
        let http = reqwest::Client::builder()
            .redirect(reqwest::redirect::Policy::none())
            .build()
            .unwrap();
        Self {
            http,
            verifier: std::iter::once(seed)
                .chain(std::iter::repeat_n('v', 42))
                .collect(),
        }
    }

    fn challenge(&self) -> String {
        URL_SAFE_NO_PAD.encode(Sha256::digest(self.verifier.as_bytes()))
    }

    async fn claim(&self, origin: &str, ticket: &str, name: &str) -> reqwest::Response {
        self.http
            .post(format!("{origin}{CLAIM_PATH}"))
            .json(&PairingClaim {
                ticket: ticket.to_owned(),
                code_challenge: self.challenge(),
                device_name: name.to_owned(),
            })
            .send()
            .await
            .unwrap()
    }

    async fn finish(&self, origin: &str, ticket: &str, verifier: &str) -> reqwest::Response {
        self.http
            .post(format!("{origin}{FINISH_PATH}"))
            .json(&PairingFinish {
                ticket: ticket.to_owned(),
                code_verifier: verifier.to_owned(),
            })
            .send()
            .await
            .unwrap()
    }
}

async fn body(response: reqwest::Response) -> Value {
    let status = response.status();
    let text = response.text().await.unwrap();
    serde_json::from_str(&text).unwrap_or_else(|error| panic!("{status}: {text} ({error})"))
}

/// 在已登录的浏览器上生成一张二维码，交出这一次配对的 `id` 和二维码里的那一串地址。
async fn new_code(browser: &Browser) -> (String, String) {
    let mut start = send(browser.get("/web/pairing")).await;
    // 这个浏览器还没登录：先登录（替身账户的密码是公开的），再回到配对页。
    if start.status != StatusCode::OK {
        log_in(browser).await;
        start = send(browser.get("/web/pairing")).await;
    }
    assert_eq!(start.status, StatusCode::OK);
    assert!(start.html.contains("显示二维码"));
    let csrf = hidden(&start.html, "csrf").unwrap();
    let created = send(browser.post_form("/web/pairing", &[("csrf", &csrf)])).await;
    assert_eq!(created.status, StatusCode::SEE_OTHER);
    let location = created.location().unwrap();
    assert!(location.starts_with("/web/pairing?id="), "{location}");
    let page = send(browser.get(&location)).await;
    assert_eq!(page.status, StatusCode::OK);
    // 等手机来扫的这一页自己刷新，页面上有二维码，也有可以照着粘贴的那一串。
    assert!(
        page.html
            .contains("<meta http-equiv=\"refresh\" content=\"2\">")
    );
    assert!(page.html.contains("aria-label=\"配对二维码\""));
    let marker = "break-all text-sm select-all\">";
    let address = page
        .html
        .split(marker)
        .nth(1)
        .and_then(|rest| rest.split('<').next())
        .expect("the pairing address is shown as text");
    (
        location.split("id=").nth(1).unwrap().to_owned(),
        address.to_owned(),
    )
}

async fn decide(browser: &Browser, id: &str, choice: &str) -> Page {
    // 防伪值从第一页上取：那总是一个带着表单的页面，而配对页本身可能已经没有表单了。
    let page = send(browser.get("/web/pairing")).await;
    assert_eq!(page.status, StatusCode::OK);
    let csrf = hidden(&page.html, "csrf").unwrap();
    send(browser.post_form(
        &format!("/web/pairing/{id}"),
        &[("csrf", &csrf), ("choice", choice)],
    ))
    .await
}

async fn pairing_count(fixture: &Fixture) -> i64 {
    sqlx::query_scalar("SELECT COUNT(*) FROM device_pairings")
        .fetch_one(&fixture.server.state.pool)
        .await
        .unwrap()
}

async fn device_count(fixture: &Fixture) -> i64 {
    sqlx::query_scalar("SELECT COUNT(*) FROM devices")
        .fetch_one(&fixture.server.state.pool)
        .await
        .unwrap()
}

async fn audit_actions(fixture: &Fixture) -> Vec<String> {
    sqlx::query_scalar("SELECT action FROM audit_log ORDER BY id")
        .fetch_all(&fixture.server.state.pool)
        .await
        .unwrap()
}

// ---- 整条路 ----

#[tokio::test]
async fn a_phone_that_the_owner_allows_is_signed_in_and_joins_the_account() {
    let fixture = fixture().await;
    let mac = register(&fixture, 'm', "MacBook", Platform::Macos).await;
    let (id, address) = new_code(&fixture.browser).await;
    assert!(ticket_from_uri(&address).is_some(), "{address}");
    let ticket = ticket_from_uri(&address).unwrap();
    let phone = Phone::new('p');

    // 认领：得到一个六位的确认码。
    let claimed = phone
        .claim(&fixture.browser.origin, ticket, PHONE_NAME)
        .await;
    assert_eq!(claimed.status(), StatusCode::OK);
    let claimed = body(claimed).await;
    let code = claimed["confirmationCode"].as_str().unwrap();
    assert_eq!(code.len(), 6);
    assert!(code.bytes().all(|byte| byte.is_ascii_digit()), "{code}");
    // 账户是谁不告诉手机。
    assert_eq!(claimed.as_object().unwrap().len(), 2, "{claimed}");

    // 网页上显示出同一串数字，等本人核对。
    let page = send(fixture.browser.get(&format!("/web/pairing?id={id}"))).await;
    assert_eq!(page.status, StatusCode::OK);
    assert!(page.html.contains(PHONE_NAME));
    assert!(page.html.contains(code));
    assert!(!page.html.contains("http-equiv=\"refresh\""));
    // 页面上留着的是标题旁的那个图标，二维码本身撤掉了。
    assert!(
        !page.html.contains("aria-label=\"配对二维码\""),
        "the code replaces the QR image"
    );

    // 本人还没点"允许"：手机问多少次都只是"等着"。
    for _ in 0..2 {
        let waiting = phone
            .finish(&fixture.browser.origin, ticket, &phone.verifier)
            .await;
        assert_eq!(waiting.status(), StatusCode::OK);
        assert_eq!(body(waiting).await, json!({"state": "pending"}));
    }
    // 没有登录之前，库里没有这台设备。
    assert_eq!(device_count(&fixture).await, 1);

    let allowed = decide(&fixture.browser, &id, "allow").await;
    assert_eq!(allowed.status, StatusCode::SEE_OTHER);
    let page = send(fixture.browser.get(&format!("/web/pairing?id={id}"))).await;
    assert!(page.html.contains("已允许"));
    assert!(page.html.contains(PHONE_NAME));

    // 手机拿到令牌、账户和设备。
    let finished = phone
        .finish(&fixture.browser.origin, ticket, &phone.verifier)
        .await;
    assert_eq!(finished.status(), StatusCode::OK);
    let finished = body(finished).await;
    assert_eq!(finished["state"], "complete");
    assert_eq!(finished["account"]["username"], USERNAME);
    assert_eq!(finished["device"]["platform"], "ios");
    assert_eq!(finished["device"]["name"], PHONE_NAME);
    let token = finished["token"].as_str().unwrap();
    assert_eq!(token.len(), 43);

    // 令牌真的能用：它就是一台设备。
    let me = phone
        .http
        .get(format!("{}/v1/me", fixture.browser.origin))
        .bearer_auth(token)
        .send()
        .await
        .unwrap();
    assert_eq!(me.status(), StatusCode::OK);
    assert_eq!(body(me).await["device"]["id"], finished["device"]["id"]);

    // 电脑那边看到账户里多了一台手机。
    let devices = mac.devices(&mac.options()).await.unwrap();
    let listed = devices
        .iter()
        .find(|device| device.name == PHONE_NAME)
        .expect("the phone is listed");
    assert_eq!(listed.platform, Platform::Ios);
    assert_eq!(listed.id, finished["device"]["id"].as_str().unwrap());

    // 这张票据到此为止。
    let again = phone
        .finish(&fixture.browser.origin, ticket, &phone.verifier)
        .await;
    assert_eq!(again.status(), StatusCode::BAD_REQUEST);
    assert_eq!(body(again).await["error"]["code"], "INVALID_AUTH_CODE");
    let page = send(fixture.browser.get(&format!("/web/pairing?id={id}"))).await;
    assert!(page.html.contains("添加好了"));

    let state: String = sqlx::query_scalar("SELECT state FROM device_pairings WHERE id = ?")
        .bind(&id)
        .fetch_one(&fixture.server.state.pool)
        .await
        .unwrap();
    assert_eq!(state, "consumed");
    let actions = audit_actions(&fixture).await;
    assert!(
        actions.contains(&"pairing.allowed".to_owned()),
        "{actions:?}"
    );
    assert!(
        actions.contains(&"device.registered".to_owned()),
        "{actions:?}"
    );
}

#[tokio::test]
async fn a_ticket_can_be_claimed_only_once() {
    let fixture = fixture().await;
    let (id, address) = new_code(&fixture.browser).await;
    let ticket = ticket_from_uri(&address).unwrap().to_owned();
    let first = Phone::new('a');
    let second = Phone::new('b');

    assert_eq!(
        first
            .claim(&fixture.browser.origin, &ticket, PHONE_NAME)
            .await
            .status(),
        StatusCode::OK
    );
    // 第二个来扫的人（比如拍下了二维码的人）认领不了。
    let late = second
        .claim(&fixture.browser.origin, &ticket, "别人的手机")
        .await;
    assert_eq!(late.status(), StatusCode::BAD_REQUEST);
    assert_eq!(body(late).await["error"]["code"], "INVALID_AUTH_CODE");
    // 网页上给人核对的是第一部手机的名字。
    let page = send(fixture.browser.get(&format!("/web/pairing?id={id}"))).await;
    assert!(page.html.contains(PHONE_NAME));
    assert!(!page.html.contains("别人的手机"));
}

// ---- 不该放行的 ----

#[tokio::test]
async fn a_wrong_verifier_burns_the_ticket_even_after_the_owner_allows() {
    let fixture = fixture().await;
    let (id, address) = new_code(&fixture.browser).await;
    let ticket = ticket_from_uri(&address).unwrap();
    let owner_phone = Phone::new('p');
    let thief = Phone::new('t');
    assert_eq!(
        owner_phone
            .claim(&fixture.browser.origin, ticket, PHONE_NAME)
            .await
            .status(),
        StatusCode::OK
    );

    // 拿着别的校验器来问：拒绝，而且这张票据不能再用了。
    let wrong = thief
        .finish(&fixture.browser.origin, ticket, &thief.verifier)
        .await;
    assert_eq!(wrong.status(), StatusCode::BAD_REQUEST);
    assert_eq!(body(wrong).await["error"]["code"], "INVALID_AUTH_CODE");
    // 就算本人随后点了"允许"，真的手机也拿不到令牌。
    assert_eq!(
        decide(&fixture.browser, &id, "allow").await.status,
        StatusCode::SEE_OTHER
    );
    let late = owner_phone
        .finish(&fixture.browser.origin, ticket, &owner_phone.verifier)
        .await;
    assert_eq!(late.status(), StatusCode::BAD_REQUEST);
    assert_eq!(body(late).await["error"]["code"], "INVALID_AUTH_CODE");
    assert_eq!(device_count(&fixture).await, 0);
    assert!(
        !audit_actions(&fixture)
            .await
            .contains(&"device.registered".to_owned())
    );
}

#[tokio::test]
async fn a_denied_pairing_tells_the_phone_so_and_registers_nothing() {
    let fixture = fixture().await;
    let (id, address) = new_code(&fixture.browser).await;
    let ticket = ticket_from_uri(&address).unwrap();
    let phone = Phone::new('p');
    assert_eq!(
        phone
            .claim(&fixture.browser.origin, ticket, PHONE_NAME)
            .await
            .status(),
        StatusCode::OK
    );

    assert_eq!(
        decide(&fixture.browser, &id, "deny").await.status,
        StatusCode::SEE_OTHER
    );
    let refused = phone
        .finish(&fixture.browser.origin, ticket, &phone.verifier)
        .await;
    assert_eq!(refused.status(), StatusCode::OK);
    assert_eq!(body(refused).await, json!({"state": "denied"}));
    assert_eq!(device_count(&fixture).await, 0);
    let page = send(fixture.browser.get(&format!("/web/pairing?id={id}"))).await;
    assert!(page.html.contains("已结束"));
    assert!(
        audit_actions(&fixture)
            .await
            .contains(&"pairing.denied".to_owned())
    );
    // 决定过的不能改。
    assert_eq!(
        decide(&fixture.browser, &id, "allow").await.status,
        StatusCode::SEE_OTHER
    );
    let still = phone
        .finish(&fixture.browser.origin, ticket, &phone.verifier)
        .await;
    assert_eq!(body(still).await, json!({"state": "denied"}));
}

#[tokio::test]
async fn an_unclaimed_code_can_be_cancelled() {
    let fixture = fixture().await;
    let (id, address) = new_code(&fixture.browser).await;
    let ticket = ticket_from_uri(&address).unwrap().to_owned();
    assert_eq!(
        decide(&fixture.browser, &id, "cancel").await.status,
        StatusCode::SEE_OTHER
    );
    let phone = Phone::new('p');
    let late = phone
        .claim(&fixture.browser.origin, &ticket, PHONE_NAME)
        .await;
    assert_eq!(late.status(), StatusCode::BAD_REQUEST);
    let page = send(fixture.browser.get(&format!("/web/pairing?id={id}"))).await;
    assert!(!page.html.contains("aria-label=\"配对二维码\""));
}

#[tokio::test]
async fn another_account_can_neither_see_nor_decide_a_pairing() {
    let fixture = fixture().await;
    let (id, address) = new_code(&fixture.browser).await;
    let ticket = ticket_from_uri(&address).unwrap();
    let phone = Phone::new('p');
    assert_eq!(
        phone
            .claim(&fixture.browser.origin, ticket, PHONE_NAME)
            .await
            .status(),
        StatusCode::OK
    );

    assemble_server::accounts::create(
        &fixture.server.state.pool,
        "blake",
        "Blake",
        SecretString::new(PASSWORD.to_owned()),
    )
    .await
    .unwrap();
    sqlx::query("UPDATE accounts SET email = 'blake@example.test', email_verified_at = 1 WHERE username = 'blake'")
        .execute(&fixture.server.state.pool)
        .await
        .unwrap();
    let stranger = Browser::new(&fixture.browser.origin);
    try_login(&stranger, "blake", PASSWORD, "").await;

    let page = send(stranger.get(&format!("/web/pairing?id={id}"))).await;
    assert_eq!(page.status, StatusCode::OK);
    assert!(page.html.contains("找不到这次配对"));
    assert!(!page.html.contains(PHONE_NAME));
    let forged = decide(&stranger, &id, "allow").await;
    assert_eq!(forged.status, StatusCode::OK);
    assert!(forged.html.contains("找不到这次配对"));
    let still = phone
        .finish(&fixture.browser.origin, ticket, &phone.verifier)
        .await;
    assert_eq!(body(still).await, json!({"state": "pending"}));
    assert_eq!(device_count(&fixture).await, 0);
}

#[tokio::test]
async fn the_web_side_wants_a_signed_in_same_origin_request_with_the_right_csrf() {
    let fixture = fixture().await;

    // 没登录：去登录页，登录之后回到账户页。
    let anonymous = Browser::new(&fixture.browser.origin);
    let page = send(anonymous.get("/web/pairing")).await;
    assert_eq!(page.status, StatusCode::SEE_OTHER);
    assert!(page.location().unwrap().starts_with("/web/login?next="));

    log_in(&fixture.browser).await;
    let start = send(fixture.browser.get("/web/pairing")).await;
    let csrf = hidden(&start.html, "csrf").unwrap();
    // 没有 Origin、防伪值不对，都过不去。
    let no_origin = send(
        fixture
            .browser
            .http
            .post(format!("{}/web/pairing", fixture.browser.origin))
            .form(&[("csrf", csrf.as_str())]),
    )
    .await;
    assert_eq!(no_origin.status, StatusCode::FORBIDDEN);
    let wrong = send(
        fixture
            .browser
            .post_form("/web/pairing", &[("csrf", "not-the-right-one")]),
    )
    .await;
    assert_eq!(wrong.status, StatusCode::FORBIDDEN);
    // 正文里的防伪值不会出现在页面上。
    assert!(!no_origin.html.contains(&csrf));
    assert_eq!(pairing_count(&fixture).await, 0);
}

#[tokio::test]
async fn malformed_requests_are_refused_before_anything_is_looked_up() {
    let fixture = fixture().await;
    let origin = fixture.browser.origin.as_str();
    let phone = Phone::new('p');
    let (_id, address) = new_code(&fixture.browser).await;
    let ticket = ticket_from_uri(&address).unwrap();

    let raw = phone
        .http
        .post(format!("{origin}{CLAIM_PATH}"))
        .body("not json")
        .send()
        .await
        .unwrap();
    assert_eq!(raw.status(), StatusCode::BAD_REQUEST);
    assert_eq!(body(raw).await["error"]["code"], "INVALID_REQUEST");

    for bad in [
        PairingClaim {
            ticket: "short".to_owned(),
            code_challenge: phone.challenge(),
            device_name: PHONE_NAME.to_owned(),
        },
        PairingClaim {
            ticket: ticket.to_owned(),
            code_challenge: "short".to_owned(),
            device_name: PHONE_NAME.to_owned(),
        },
        PairingClaim {
            ticket: ticket.to_owned(),
            code_challenge: phone.challenge(),
            device_name: "   ".to_owned(),
        },
    ] {
        let refused = phone
            .http
            .post(format!("{origin}{CLAIM_PATH}"))
            .json(&bad)
            .send()
            .await
            .unwrap();
        assert_eq!(refused.status(), StatusCode::BAD_REQUEST, "{bad:?}");
        assert_eq!(
            body(refused).await["error"]["code"],
            "INVALID_AUTH_CODE",
            "{bad:?}"
        );
    }
    // 多出来的字段不被接受。
    let mut extra = serde_json::to_value(&PairingFinish {
        ticket: ticket.to_owned(),
        code_verifier: phone.verifier.clone(),
    })
    .unwrap();
    extra["surprise"] = json!(1);
    let refused = phone
        .http
        .post(format!("{origin}{FINISH_PATH}"))
        .json(&extra)
        .send()
        .await
        .unwrap();
    assert_eq!(refused.status(), StatusCode::BAD_REQUEST);

    // 上面的都没动过这张票据：它还是能认领。
    assert_eq!(
        phone.claim(origin, ticket, PHONE_NAME).await.status(),
        StatusCode::OK
    );
}

#[tokio::test]
async fn an_expired_pairing_is_refused_and_swept() {
    let fixture = fixture().await;
    let (id, address) = new_code(&fixture.browser).await;
    let ticket = ticket_from_uri(&address).unwrap();
    let phone = Phone::new('p');
    sqlx::query("UPDATE device_pairings SET expires_at = 1")
        .execute(&fixture.server.state.pool)
        .await
        .unwrap();

    let late = phone
        .claim(&fixture.browser.origin, ticket, PHONE_NAME)
        .await;
    assert_eq!(late.status(), StatusCode::BAD_REQUEST);
    assert_eq!(body(late).await["error"]["code"], "INVALID_AUTH_CODE");
    let page = send(fixture.browser.get(&format!("/web/pairing?id={id}"))).await;
    assert!(page.html.contains("这次配对已作废"));
    assert!(!page.html.contains("http-equiv=\"refresh\""));

    let report = assemble_server::maintenance::run_once(&fixture.server.state)
        .await
        .unwrap();
    assert_eq!(report.expired_pairings, 1);
    assert_eq!(pairing_count(&fixture).await, 0);
}

#[tokio::test]
async fn only_a_handful_of_pairings_run_at_once_per_account() {
    let fixture = fixture().await;
    for _ in 0..5 {
        new_code(&fixture.browser).await;
    }
    let start = send(fixture.browser.get("/web/pairing")).await;
    let csrf = hidden(&start.html, "csrf").unwrap();
    let sixth = send(
        fixture
            .browser
            .post_form("/web/pairing", &[("csrf", &csrf)]),
    )
    .await;
    assert_eq!(sixth.status, StatusCode::TOO_MANY_REQUESTS);
    assert_eq!(sixth.headers.get("retry-after").unwrap(), "180");
    assert!(sixth.html.contains("同时进行的配对太多了"));
    assert_eq!(pairing_count(&fixture).await, 5);
}

#[tokio::test]
async fn the_account_page_leads_to_the_pairing_page() {
    let fixture = fixture().await;
    log_in(&fixture.browser).await;
    let page = send(fixture.browser.get("/web/account")).await;
    assert_eq!(page.status, StatusCode::OK);
    assert!(page.html.contains("href=\"/web/pairing\""));
    assert!(page.html.contains("添加手机"));
}

#[test]
fn the_address_in_the_code_is_what_the_phone_later_pastes() {
    let ticket = "abcdefghijklmnopqrstuvwxyz0123456789-_ABCDE";
    assert_eq!(ticket_from_uri(&pairing_uri(ticket)), Some(ticket));
}
