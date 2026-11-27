//! 助手只按资源标识组织本人目录，不接受本机路径、凭据或任意请求。
use crate::tools::{Call, Failure};
use assemble_protocol::drive_tree::{self as wire, Entry};
use rmcp::schemars::JsonSchema;
use serde::Deserialize;

fn explain(error: &assemble_client::ClientError) -> Failure {
    if let assemble_client::ClientError::Http {
        remote_code: Some(code),
        ..
    } = error
    {
        let message = match code.as_str() {
            "VERSION_CONFLICT" => {
                Some("目录项目已在别处更改，请先用 assemble_drive_list 重读版本，不要覆盖。")
            }
            "DRIVE_FOLDER_NOT_EMPTY" => Some("只能删除空文件夹，请先核对并移出其中内容。"),
            "DRIVE_NAME_CONFLICT" => Some("目标位置已有同名文件夹，请更换名称。"),
            "DRIVE_REQUEST_CONFLICT" => Some("该 requestId 已用于不同的新建请求，请先核对原操作。"),
            "DRIVE_CYCLE" => Some("文件夹不能移动到自身或后代目录中。"),
            "DRIVE_DEPTH_LIMIT" => Some("文件夹最多支持 16 层，请选择更上层的位置。"),
            "DRIVE_FOLDER_LIMIT" | "DRIVE_REQUEST_LIMIT" => Some("已达到文件夹或创建请求上限。"),
            _ => None,
        };
        if let Some(message) = message {
            return Failure::Rejected(message.into());
        }
    }
    crate::tools::explain(error)
}

#[derive(Debug, Clone, Copy, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub enum Kind {
    Folder,
    File,
}
impl From<Kind> for wire::Kind {
    fn from(value: Kind) -> Self {
        match value {
            Kind::Folder => Self::Folder,
            Kind::File => Self::File,
        }
    }
}
fn root() -> String {
    wire::ROOT.into()
}
fn page() -> u32 {
    1
}
#[derive(Debug, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct List {
    #[serde(default = "root")]
    pub parent_id: String,
    #[serde(default)]
    pub q: String,
    #[serde(default = "page")]
    pub page: u32,
}
#[derive(Debug, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Create {
    pub parent_id: String,
    pub name: String,
    pub request_id: String,
}
#[derive(Debug, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Rename {
    pub kind: Kind,
    pub id: String,
    pub version: i64,
    pub name: String,
}
#[derive(Debug, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Move {
    pub kind: Kind,
    pub id: String,
    pub version: i64,
    pub parent_id: String,
}
#[derive(Debug, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Delete {
    pub id: String,
    pub version: i64,
}

fn encode(value: &impl serde::Serialize) -> Result<String, Failure> {
    serde_json::to_string(value).map_err(|_| Failure::Other("目录结果无法编码。".into()))
}
fn safe_entry(entry: Entry) -> serde_json::Value {
    match entry {
        Entry::Folder(folder) => {
            serde_json::json!({"kind":"folder","id":folder.id,"parentId":folder.parent_id,"name":folder.name,"version":folder.version,"createdAt":folder.created_at,"updatedAt":folder.updated_at})
        }
        Entry::File(file) => {
            serde_json::json!({"kind":"file","id":file.file.id,"parentId":file.parent_id,"name":file.file.name,"size":file.file.size,"version":file.version,"createdAt":file.file.created_at,"updatedAt":file.updated_at})
        }
    }
}
pub async fn list(call: &Call<'_>, input: &List) -> Result<String, Failure> {
    let listing = call
        .client()
        .drive_tree(
            &wire::Query {
                folders_only: false,
                parent_id: input.parent_id.clone(),
                q: input.q.clone(),
                page: input.page,
                page_size: 20,
            },
            &call.options(),
        )
        .await
        .map_err(|error| explain(&error))?;
    encode(
        &serde_json::json!({"current":listing.current,"ancestors":listing.ancestors,"items":listing.items.into_iter().map(safe_entry).collect::<Vec<_>>(),"total":listing.total,"page":listing.page,"pageSize":listing.page_size,"usedBytes":listing.used_bytes,"limitBytes":listing.limit_bytes}),
    )
}
pub async fn create(call: &Call<'_>, input: &Create) -> Result<String, Failure> {
    let folder = call
        .client()
        .drive_folder_create(
            &wire::Create {
                parent_id: input.parent_id.clone(),
                name: input.name.clone(),
                request_id: input.request_id.clone(),
            },
            &call.options(),
        )
        .await
        .map_err(|error| explain(&error))?;
    encode(&folder)
}
pub async fn rename(call: &Call<'_>, input: &Rename) -> Result<String, Failure> {
    let entry = call
        .client()
        .drive_tree_rename(
            input.kind.into(),
            &input.id,
            &wire::Rename {
                name: input.name.clone(),
                version: input.version,
            },
            &call.options(),
        )
        .await
        .map_err(|error| explain(&error))?;
    encode(&safe_entry(entry))
}
pub async fn move_entry(call: &Call<'_>, input: &Move) -> Result<String, Failure> {
    let entry = call
        .client()
        .drive_tree_move(
            &wire::Move {
                kind: input.kind.into(),
                id: input.id.clone(),
                parent_id: input.parent_id.clone(),
                version: input.version,
            },
            &call.options(),
        )
        .await
        .map_err(|error| explain(&error))?;
    encode(&safe_entry(entry))
}
pub async fn delete(call: &Call<'_>, input: &Delete) -> Result<String, Failure> {
    call.client()
        .drive_tree_delete(
            wire::Kind::Folder,
            &input.id,
            input.version,
            &call.options(),
        )
        .await
        .map_err(|error| explain(&error))?;
    Ok("空文件夹已删除。".into())
}
