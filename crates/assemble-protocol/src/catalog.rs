//! 能力目录：服务端对每种资源的自描述，供管理端与 MCP 生成界面和工具。

use serde::{Deserialize, Serialize};
use serde_json::Value;

/// 一种资源开放了哪些操作。五个开关是线上的原样结构，不另外设计。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
#[allow(clippy::struct_excessive_bools)]
pub struct ResourceActions {
    pub list: bool,
    pub get: bool,
    pub create: bool,
    pub update: bool,
    pub delete: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum FieldType {
    Text,
    Number,
    Boolean,
    Datetime,
    Select,
    Json,
    Textarea,
}

/// 资源的一个可编辑字段。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ResourceField {
    pub name: String,
    pub label: String,
    pub label_en: String,
    #[serde(rename = "type")]
    pub kind: FieldType,
    pub required: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub options: Option<Vec<String>>,
}

/// 一种资源的完整自描述。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct JarvisCapability {
    pub resource: String,
    pub label: String,
    pub label_en: String,
    pub readonly: bool,
    pub actions: ResourceActions,
    pub fields: Vec<ResourceField>,
    /// 写入该资源时的 JSON Schema。
    pub input_schema: Value,
}

/// `GET /api/jarvis/capabilities` 的结果。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CapabilitiesResponse {
    pub resources: Vec<JarvisCapability>,
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    #[test]
    fn a_capability_describes_its_actions_fields_and_schema() {
        let response: CapabilitiesResponse = serde_json::from_value(json!({
            "resources": [{
                "resource": "reminders",
                "label": "提醒",
                "labelEn": "Reminders",
                "readonly": false,
                "actions": { "list": true, "get": true, "create": true, "update": true, "delete": true },
                "fields": [
                    { "name": "status", "label": "状态", "labelEn": "Status", "type": "select",
                      "required": true, "options": ["scheduled", "fired"] },
                    { "name": "notes", "label": "备注", "labelEn": "Notes", "type": "textarea", "required": false }
                ],
                "inputSchema": { "type": "object" }
            }]
        }))
        .unwrap();
        let reminder = &response.resources[0];
        assert!(!reminder.readonly);
        assert!(reminder.actions.delete);
        assert_eq!(reminder.fields[0].kind, FieldType::Select);
        assert_eq!(
            reminder.fields[0].options.as_deref(),
            Some(&["scheduled".to_owned(), "fired".to_owned()][..])
        );
        assert_eq!(reminder.fields[1].kind, FieldType::Textarea);
        assert!(reminder.fields[1].options.is_none());
    }
}
