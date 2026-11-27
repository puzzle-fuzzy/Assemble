//! Jarvis 日程的一次性维护导入：严格校验、显式账户映射、事务与来源回执。
use assemble_protocol::{
    memo::MemoContent,
    primitives::{Timestamp, is_valid_id},
    schedule::{Location, ReminderContent, ReminderStatus, ScheduleContent, ScheduleStatus},
};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use sqlx::SqlitePool;
use std::{
    collections::{BTreeMap, BTreeSet},
    path::Path,
};

const LIMIT: u64 = 16 * 1024 * 1024;
#[derive(Debug, thiserror::Error)]
#[error("{0}")]
pub struct Error(pub &'static str);
impl From<sqlx::Error> for Error {
    fn from(_: sqlx::Error) -> Self {
        Self("PLANNER_IMPORT_DATABASE")
    }
}
#[derive(Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct Archive {
    version: u32,
    source_origin: String,
    source_account_id: String,
    records: Vec<Record>,
    navigation: Vec<Binding>,
    occurrences: Vec<OccurrenceRow>,
    baselines: Vec<Baseline>,
}
#[derive(Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct Record {
    id: String,
    resource: String,
    version: i64,
    data: serde_json::Value,
    created_at: i64,
    updated_at: i64,
}
#[derive(Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct Binding {
    resource: String,
    id: String,
    source_version: i64,
    locations: Vec<Location>,
}
#[derive(Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct OccurrenceRow {
    schedule_id: String,
    occurrence_at: i64,
    notification_id: String,
    state: String,
    legacy_epoch: Option<i64>,
}
#[derive(Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct Baseline {
    schedule_id: String,
    cutoff_at: i64,
    skip_through_at: i64,
}
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Report {
    pub schedules: usize,
    pub reminders: usize,
    pub memos: usize,
    pub notifications: usize,
    pub location_bindings: usize,
    pub already_imported: bool,
}

fn invalid() -> Error {
    Error("PLANNER_IMPORT_INVALID_ARCHIVE")
}
fn hash(bytes: impl AsRef<[u8]>) -> String {
    assemble_protocol::primitives::to_hex(&Sha256::digest(bytes))
}
fn parsed<T: serde::de::DeserializeOwned>(value: &serde_json::Value) -> Result<T, Error> {
    serde_json::from_value(value.clone()).map_err(|_| invalid())
}
fn status<T: serde::de::DeserializeOwned>(record: &Record) -> Result<T, Error> {
    parsed(record.data.get("status").ok_or_else(invalid)?)
}
fn locations_valid(locations: &[Location]) -> bool {
    let mut labels = BTreeMap::new();
    locations.len() <= 32
        && locations.iter().all(|l| {
            let valid = |text: &str, max: usize| {
                !text.trim().is_empty()
                    && text.encode_utf16().count() <= max
                    && !text.chars().any(char::is_control)
            };
            valid(&l.label, 300)
                && valid(&l.address, 1000)
                && labels
                    .insert(&l.label, &l.address)
                    .is_none_or(|address| address == &l.address)
        })
}
fn preference(record: &Record) -> Result<MemoContent, Error> {
    let key = record
        .data
        .get("key")
        .and_then(serde_json::Value::as_str)
        .ok_or_else(invalid)?;
    if !key.starts_with("personal.") {
        return Err(invalid());
    }
    let value = record.data.get("value").ok_or_else(invalid)?;
    let text = if let Some(text) = value.as_str() {
        text.to_owned()
    } else {
        serde_json::to_string_pretty(value).map_err(|_| invalid())?
    };
    let content = MemoContent {
        title: key.to_owned(),
        text,
    };
    content.cleaned().ok_or_else(invalid)
}
fn is_registry(record: &Record) -> bool {
    record.resource == "preferences"
        && record.data.get("key").and_then(serde_json::Value::as_str)
            == Some("navigation.locations.v1")
}

fn validate_record(record: &Record, report: &mut Report) -> Result<(), Error> {
    match record.resource.as_str() {
        "schedules" => {
            let content: ScheduleContent = parsed(&record.data)?;
            if content.cleaned().as_ref() != Some(&content) {
                return Err(invalid());
            }
            let _: ScheduleStatus = status(record)?;
            report.schedules += 1;
        }
        "reminders" => {
            let content: ReminderContent = parsed(&record.data)?;
            if content.cleaned().as_ref() != Some(&content) {
                return Err(invalid());
            }
            let _: ReminderStatus = status(record)?;
            let zone = record
                .data
                .get("timezone")
                .and_then(serde_json::Value::as_str)
                .ok_or_else(invalid)?;
            if zone.is_empty() || zone.len() > 100 {
                return Err(invalid());
            }
            if let Some(location) = record.data.get("location").filter(|l| !l.is_null()) {
                let location: Location = parsed(location)?;
                if !locations_valid(&[location]) {
                    return Err(invalid());
                }
            }
            report.reminders += 1;
        }
        "preferences" if is_registry(record) => {}
        "preferences" => {
            preference(record)?;
            report.memos += 1;
        }
        "notifications" => {
            if record
                .data
                .get("title")
                .and_then(serde_json::Value::as_str)
                .is_none()
                || record
                    .data
                    .get("body")
                    .and_then(serde_json::Value::as_str)
                    .is_none()
                || record
                    .data
                    .get("read")
                    .and_then(serde_json::Value::as_bool)
                    .is_none()
            {
                return Err(invalid());
            }
            report.notifications += 1;
        }
        _ => return Err(invalid()),
    }
    Ok(())
}

async fn checked(directory: &Path) -> Result<(Archive, String, String, Report), Error> {
    let path = directory.join("archive.json");
    let metadata = tokio::fs::symlink_metadata(&path)
        .await
        .map_err(|_| Error("PLANNER_IMPORT_READ"))?;
    if !metadata.is_file() || metadata.len() > LIMIT {
        return Err(invalid());
    }
    let bytes = tokio::fs::read(path)
        .await
        .map_err(|_| Error("PLANNER_IMPORT_READ"))?;
    let digest = hash(&bytes);
    let expected = tokio::fs::read_to_string(directory.join("archive.sha256"))
        .await
        .map_err(|_| Error("PLANNER_IMPORT_READ"))?;
    if expected.trim() != digest {
        return Err(Error("PLANNER_IMPORT_DIGEST"));
    }
    let archive: Archive = serde_json::from_slice(&bytes).map_err(|_| invalid())?;
    if archive.version != 1
        || archive.source_origin != "https://jarvis.yxswy.com"
        || !is_valid_id(&archive.source_account_id)
        || archive.records.len() > 20_000
        || archive.navigation.len() > 512
    {
        return Err(invalid());
    }
    let mut keys = BTreeSet::new();
    let mut report = Report {
        schedules: 0,
        reminders: 0,
        memos: 0,
        notifications: 0,
        location_bindings: 0,
        already_imported: false,
    };
    for record in &archive.records {
        if !is_valid_id(&record.id)
            || record.version < 1
            || record.version == i64::MAX
            || record.created_at > record.updated_at
            || Timestamp::from_millis(record.created_at).is_none()
            || Timestamp::from_millis(record.updated_at).is_none()
            || !keys.insert((&record.resource, &record.id))
        {
            return Err(invalid());
        }
        validate_record(record, &mut report)?;
    }
    if report.schedules + report.reminders == 0 {
        return Err(invalid());
    }
    let mut bindings = BTreeSet::new();
    for binding in &archive.navigation {
        if !matches!(
            binding.resource.as_str(),
            "schedules" | "reminders" | "notifications" | "preferences"
        ) || !locations_valid(&binding.locations)
            || binding.source_version < 1
            || !bindings.insert((&binding.resource, &binding.id))
        {
            return Err(invalid());
        }
        // 旧版本或已删除记录的地址不能投影到当前正文；原始登记表仍留在受限来源包。
        if archive.records.iter().any(|r| {
            r.resource == binding.resource
                && r.id == binding.id
                && r.version == binding.source_version
        }) {
            report.location_bindings += 1;
        }
    }
    for row in &archive.baselines {
        if row.skip_through_at < row.cutoff_at
            || !keys.contains(&(&"schedules".to_owned(), &row.schedule_id))
        {
            return Err(invalid());
        }
    }
    for row in &archive.occurrences {
        if !matches!(row.state.as_str(), "generated" | "invalidated")
            || Timestamp::from_millis(row.occurrence_at).is_none()
            || !is_valid_id(&row.notification_id)
        {
            return Err(invalid());
        }
    }
    let raw = String::from_utf8(bytes).map_err(|_| invalid())?;
    Ok((archive, digest, raw, report))
}
pub async fn inspect(directory: &Path) -> Result<Report, Error> {
    Ok(checked(directory).await?.3)
}
fn target_id(archive: &Archive, account: &str, resource: &str, id: &str) -> String {
    format!(
        "j-{}",
        &hash(format!(
            "{}\0{}\0{account}\0{resource}\0{id}",
            archive.source_origin, archive.source_account_id
        ))[..40]
    )
}

pub async fn import(pool: &SqlitePool, directory: &Path, email: &str) -> Result<Report, Error> {
    let (archive, fingerprint, raw, mut report) = checked(directory).await?;
    let mut tx = pool.begin_with("BEGIN IMMEDIATE").await?;
    let account:Option<String>=sqlx::query_scalar("SELECT id FROM accounts WHERE lower(email)=lower(?) AND email_verified_at IS NOT NULL AND suspended_at IS NULL").bind(email).fetch_optional(&mut *tx).await?;
    let account = account.ok_or(Error("PLANNER_IMPORT_ACCOUNT_UNAVAILABLE"))?;
    let previous:Option<String>=sqlx::query_scalar("SELECT fingerprint FROM planner_import_runs WHERE account_id=? AND source_origin=? AND source_account_id=?").bind(&account).bind(&archive.source_origin).bind(&archive.source_account_id).fetch_optional(&mut *tx).await?;
    if let Some(previous) = previous {
        if previous != fingerprint {
            return Err(Error("PLANNER_IMPORT_SOURCE_CHANGED"));
        }
        report.already_imported = true;
        tx.rollback().await?;
        return Ok(report);
    }
    let id = |resource: &str, source: &str| target_id(&archive, &account, resource, source);
    for record in &archive.records {
        if is_registry(record) || record.resource == "notifications" {
            continue;
        }
        let target = id(&record.resource, &record.id);
        match record.resource.as_str() {
            "schedules" => {
                let content: ScheduleContent = parsed(&record.data)?;
                sqlx::query("INSERT INTO schedules(id,account_id,title,location_label,location_address,start_at,end_at,repeat,repeat_until,remind_before,notes,status,version,created_by,created_at,updated_at) VALUES(?,?,?,?,?,?,?,?,?,?,?,?,?,'Jarvis',?,?)")
                    .bind(&target).bind(&account).bind(&content.title).bind(content.location.as_ref().map(|l|&l.label)).bind(content.location.as_ref().map(|l|&l.address)).bind(content.start_at.as_millis()).bind(content.end_at.map(Timestamp::as_millis)).bind(super::spelling(&content.repeat)).bind(content.repeat_until.map(Timestamp::as_millis)).bind(content.remind_before.map(i64::from)).bind(&content.notes).bind(super::spelling(&status::<ScheduleStatus>(record)?)).bind(record.version).bind(record.created_at).bind(record.updated_at).execute(&mut *tx).await?;
            }
            "reminders" => {
                let content: ReminderContent = parsed(&record.data)?;
                sqlx::query("INSERT INTO reminders(id,account_id,title,due_at,notes,status,version,created_by,created_at,updated_at) VALUES(?,?,?,?,?,?,?,'Jarvis',?,?)").bind(&target).bind(&account).bind(&content.title).bind(content.due_at.as_millis()).bind(&content.notes).bind(super::spelling(&status::<ReminderStatus>(record)?)).bind(record.version).bind(record.created_at).bind(record.updated_at).execute(&mut *tx).await?;
            }
            "preferences" => {
                let content = preference(record)?;
                sqlx::query("INSERT INTO memos(id,account_id,title,text,version,created_at,updated_at) VALUES(?,?,?,?,?,?,?)").bind(&target).bind(&account).bind(content.title).bind(content.text).bind(record.version).bind(record.created_at).bind(record.updated_at).execute(&mut *tx).await?;
            }
            _ => return Err(invalid()),
        }
        let binding = archive.navigation.iter().find(|b| {
            b.resource == record.resource && b.id == record.id && b.source_version == record.version
        });
        let mut locations = binding.map_or_else(Vec::new, |b| b.locations.clone());
        if binding.is_none()
            && let Some(location) = record.data.get("location").filter(|l| !l.is_null())
        {
            locations.push(parsed::<Location>(location)?);
        }
        if !locations.is_empty() {
            let resource = match record.resource.as_str() {
                "schedules" => "schedule",
                "reminders" => "reminder",
                _ => "memo",
            };
            sqlx::query("INSERT INTO record_locations(account_id,resource,resource_id,locations) VALUES(?,?,?,?)").bind(&account).bind(resource).bind(target).bind(serde_json::to_string(&locations).map_err(|_|invalid())?).execute(&mut *tx).await?;
        }
    }
    // 已生成的场次，即使源通知后来被删除也保留占位；不会在迁移后补弹旧场次。
    for row in &archive.occurrences {
        let schedule = id("schedules", &row.schedule_id);
        sqlx::query("INSERT INTO notifications(id,account_id,kind,title,schedule_id,occurrence_at,raised_at,read_at,invalidated_at) VALUES(?,?,'schedule','Jarvis',?,?,?,?,?)")
            .bind(format!("{schedule}-{}",row.occurrence_at)).bind(&account).bind(schedule).bind(row.occurrence_at).bind(row.legacy_epoch.unwrap_or(row.occurrence_at)).bind(row.occurrence_at).bind((row.state=="invalidated").then_some(row.occurrence_at)).execute(&mut *tx).await?;
    }
    for row in &archive.baselines {
        sqlx::query("INSERT INTO planner_import_baselines(account_id,schedule_id,skip_through_at) VALUES(?,?,?)").bind(&account).bind(id("schedules",&row.schedule_id)).bind(row.skip_through_at).execute(&mut *tx).await?;
    }
    for record in archive
        .records
        .iter()
        .filter(|r| r.resource == "notifications")
    {
        let schedule = record
            .data
            .get("scheduleId")
            .and_then(serde_json::Value::as_str)
            .map(|source| id("schedules", source));
        let reminder = record
            .data
            .get("reminderId")
            .and_then(serde_json::Value::as_str)
            .map(|source| id("reminders", source));
        if schedule.is_none() && reminder.is_none() {
            return Err(invalid());
        }
        // 保留原始已读状态和生成时间，补弹仍遵守现有的时间窗口。
        sqlx::query("INSERT INTO notifications(id,account_id,kind,title,body,schedule_id,reminder_id,raised_at,read_at) VALUES(?,?,?,?,?,?,?,?,?)")
            .bind(id("notifications",&record.id)).bind(&account).bind(if schedule.is_some(){"schedule"}else{"reminder"}).bind(record.data.get("title").and_then(serde_json::Value::as_str)).bind(record.data.get("body").and_then(serde_json::Value::as_str)).bind(schedule).bind(reminder).bind(record.created_at).bind(record.data.get("read").and_then(serde_json::Value::as_bool).filter(|read| *read).map(|_|record.updated_at)).execute(&mut *tx).await?;
    }
    let now = Timestamp::now().as_millis();
    sqlx::query("INSERT INTO planner_import_runs(account_id,source_origin,source_account_id,fingerprint,archive,imported_at) VALUES(?,?,?,?,?,?)").bind(&account).bind(&archive.source_origin).bind(&archive.source_account_id).bind(&fingerprint).bind(raw).bind(now).execute(&mut *tx).await?;
    sqlx::query("INSERT INTO audit_log(account_id,action,target,created_at) VALUES(?,'planner.imported',?,?)").bind(&account).bind(&fingerprint).bind(now).execute(&mut *tx).await?;
    tx.commit().await?;
    Ok(report)
}
