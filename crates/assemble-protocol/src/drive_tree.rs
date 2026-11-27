//! 云盘目录的独立契约；旧平铺接口不因目录能力而改变。
use serde::{Deserialize, Serialize};

use crate::{
    drive::clean_file_name,
    primitives::{Timestamp, is_valid_id},
};

pub const MAX_FOLDERS: usize = 1000;
pub const MAX_DEPTH: usize = 16;
/// 回执永久保留至账户删除，达到上限后拒绝新的创建，避免过期重放意外重建。
pub const MAX_CREATE_REQUESTS: i64 = 10_000;
pub const MAX_QUERY_CHARS: usize = 200;
pub const ROOT: &str = "root";

pub fn valid_parent(id: &str) -> bool {
    id == ROOT || is_valid_id(id)
}
pub fn parent_id(id: &str) -> Option<&str> {
    (id != ROOT).then_some(id)
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Folder {
    pub id: String,
    pub parent_id: Option<String>,
    pub name: String,
    pub version: i64,
    pub created_at: Timestamp,
    pub updated_at: Timestamp,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TreeFile {
    #[serde(flatten)]
    pub file: crate::drive::DriveFile,
    pub parent_id: Option<String>,
    pub version: i64,
    pub updated_at: Option<Timestamp>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "camelCase")]
pub enum Entry {
    Folder(Folder),
    File(TreeFile),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum Kind {
    Folder,
    File,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Listing {
    pub current: Option<Folder>,
    pub ancestors: Vec<Folder>,
    pub items: Vec<Entry>,
    pub total: u64,
    pub page: u32,
    pub page_size: u32,
    pub used_bytes: u64,
    pub limit_bytes: u64,
}

fn one() -> u32 {
    1
}
fn twenty() -> u32 {
    20
}
fn root() -> String {
    ROOT.to_owned()
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Query {
    #[serde(default)]
    pub folders_only: bool,
    #[serde(default = "root")]
    pub parent_id: String,
    #[serde(default)]
    pub q: String,
    #[serde(default = "one")]
    pub page: u32,
    #[serde(default = "twenty")]
    pub page_size: u32,
}
impl Default for Query {
    fn default() -> Self {
        Self {
            folders_only: false,
            parent_id: root(),
            q: String::new(),
            page: 1,
            page_size: 20,
        }
    }
}
impl Query {
    pub fn valid(&self) -> bool {
        valid_parent(&self.parent_id)
            && self.q.chars().count() <= MAX_QUERY_CHARS
            && self.page > 0
            && (1..=100).contains(&self.page_size)
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Create {
    pub request_id: String,
    pub parent_id: String,
    pub name: String,
}
impl Create {
    pub fn valid(&self) -> bool {
        is_valid_id(&self.request_id)
            && valid_parent(&self.parent_id)
            && clean_file_name(&self.name).is_some()
    }
}
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Rename {
    pub name: String,
    pub version: i64,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Move {
    pub kind: Kind,
    pub id: String,
    pub version: i64,
    pub parent_id: String,
}

/// 脱离数据库的目录拓扑，移动时连同最深后代一起检查。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Node {
    pub id: String,
    pub parent_id: Option<String>,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TopologyError {
    Missing,
    Cycle,
    Depth,
}
pub fn ancestors<'a>(
    nodes: &'a [Node],
    parent: Option<&str>,
) -> Result<Vec<&'a Node>, TopologyError> {
    let mut chain = Vec::new();
    let mut next = parent;
    while let Some(id) = next {
        if chain.iter().any(|node: &&Node| node.id == id) {
            return Err(TopologyError::Cycle);
        }
        let node = nodes
            .iter()
            .find(|node| node.id == id)
            .ok_or(TopologyError::Missing)?;
        chain.push(node);
        if chain.len() > MAX_DEPTH {
            return Err(TopologyError::Depth);
        }
        next = node.parent_id.as_deref();
    }
    chain.reverse();
    Ok(chain)
}
pub fn check_move(nodes: &[Node], id: &str, parent: Option<&str>) -> Result<(), TopologyError> {
    if !nodes.iter().any(|node| node.id == id) {
        return Err(TopologyError::Missing);
    }
    let chain = ancestors(nodes, parent)?;
    if chain.iter().any(|node| node.id == id) {
        return Err(TopologyError::Cycle);
    }
    let mut height = 1;
    for node in nodes {
        let branch = ancestors(nodes, Some(&node.id))?;
        if let Some(index) = branch.iter().position(|node| node.id == id) {
            height = height.max(branch.len() - index);
        }
    }
    if chain.len() + height > MAX_DEPTH {
        return Err(TopologyError::Depth);
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn subtree_depth_and_cycle_are_checked() {
        let nodes: Vec<Node> = (0..MAX_DEPTH)
            .map(|i| Node {
                id: format!("n{i}"),
                parent_id: (i > 0).then(|| format!("n{}", i - 1)),
            })
            .collect();
        assert_eq!(
            check_move(&nodes, "n0", Some("n15")),
            Err(TopologyError::Cycle)
        );
        let mut branches = nodes.clone();
        branches.push(Node {
            id: "extra".into(),
            parent_id: None,
        });
        assert_eq!(
            check_move(&branches, "n0", Some("extra")),
            Err(TopologyError::Depth)
        );
        assert_eq!(check_move(&branches, "n15", Some("extra")), Ok(()));
        assert_eq!(
            ancestors(&nodes, Some("missing")),
            Err(TopologyError::Missing)
        );
    }
    #[test]
    fn bounded_queries_names_and_explicit_root() {
        assert!(Query::default().valid());
        assert!(
            !Query {
                page_size: 101,
                ..Query::default()
            }
            .valid()
        );
        assert!(
            !Query {
                q: "字".repeat(201),
                ..Query::default()
            }
            .valid()
        );
        assert!(
            !Create {
                request_id: "r1".into(),
                parent_id: ROOT.into(),
                name: "../x".into()
            }
            .valid()
        );
        assert_eq!(parent_id(ROOT), None);
    }
}
