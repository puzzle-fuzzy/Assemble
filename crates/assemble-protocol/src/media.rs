//! 共享与私人片库的线上契约；可空字段显式序列化，避免隐私投影产生两种形状。

use serde::{Deserialize, Serialize};

macro_rules! media_enum {
    ($name:ident { $($variant:ident),+ }) => {
        #[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
        #[serde(rename_all = "camelCase")]
        pub enum $name { $($variant),+ }
    };
}
media_enum!(MediaScope { Shared, Private });
media_enum!(MediaType { Movie, Tv });
media_enum!(MediaStatus {
    Planned,
    Watching,
    Completed
});
media_enum!(MediaProvider { Tmdb, Douban });
media_enum!(MediaSort {
    Updated,
    Title,
    Year,
    Watched
});
media_enum!(MediaJobStatus {
    Queued,
    Running,
    Completed,
    Failed,
    Cancelled,
    Interrupted
});
media_enum!(MediaJobStage {
    Metadata,
    Images,
    Saving
});

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct MediaIdentity {
    pub provider: MediaProvider,
    pub media_type: MediaType,
    pub external_id: String,
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct MediaPersonIdentity {
    pub provider: MediaProvider,
    pub external_id: String,
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct MediaImage {
    pub asset_id: String,
    pub width: u32,
    pub height: u32,
    pub content_type: String,
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct MediaCast {
    pub name: String,
    pub character: String,
    pub identity: Option<MediaPersonIdentity>,
    pub photo: Option<MediaImage>,
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct MediaContent {
    pub media_type: MediaType,
    pub title: String,
    pub original_title: String,
    pub year: Option<i32>,
    pub release_date: Option<String>,
    pub overview: String,
    pub genres: Vec<String>,
    pub episode_count: Option<u32>,
    pub status: MediaStatus,
    pub watched_at: Option<String>,
    pub review: Option<String>,
    pub notes: Option<String>,
    pub magnet_uri: Option<String>,
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct MediaEntry {
    pub id: String,
    pub version: u64,
    pub scope: MediaScope,
    #[serde(flatten)]
    pub content: MediaContent,
    pub cast: Vec<MediaCast>,
    pub identities: Vec<MediaIdentity>,
    pub poster: Option<MediaImage>,
    pub magnet_available: bool,
    pub created_at: String,
    pub updated_at: String,
}
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct MediaCounts {
    pub all: u64,
    pub planned: u64,
    pub watching: u64,
    pub completed: u64,
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct MediaList {
    pub items: Vec<MediaEntry>,
    pub total: u64,
    pub page: u32,
    pub page_size: u32,
    pub counts: MediaCounts,
}
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct MediaQuery {
    pub q: Option<String>,
    pub page: Option<u32>,
    pub page_size: Option<u32>,
    pub media_type: Option<MediaType>,
    pub status: Option<MediaStatus>,
    pub genre: Option<String>,
    pub year: Option<i32>,
    pub sort: Option<MediaSort>,
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct MediaProviders {
    pub tmdb: bool,
    pub douban: bool,
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct MediaCapabilities {
    pub providers: MediaProviders,
    pub can_maintain_shared: bool,
    pub can_maintain_private: bool,
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct MediaCandidate {
    pub identity: MediaIdentity,
    pub title: String,
    pub original_title: String,
    pub year: Option<i32>,
    pub overview: String,
    pub poster: Option<MediaImage>,
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct MediaJob {
    pub id: String,
    pub scope: MediaScope,
    pub status: MediaJobStatus,
    pub stage: Option<MediaJobStage>,
    pub completed_items: Option<u32>,
    pub total_items: Option<u32>,
    pub entry_id: Option<String>,
    pub error_code: Option<String>,
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct MediaGallery {
    pub entry_id: String,
    pub version: u64,
    pub items: Vec<MediaImage>,
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct MediaActor {
    pub identity: MediaPersonIdentity,
    pub name: String,
    pub photo: Option<MediaImage>,
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct MediaActorCredits {
    pub actor: MediaActor,
    pub items: Vec<MediaCandidate>,
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct MediaCollection {
    pub id: String,
    pub version: u64,
    pub title: String,
    pub description: String,
    pub poster: Option<MediaImage>,
    pub items: Vec<MediaEntry>,
    pub created_at: String,
    pub updated_at: String,
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct MediaCollectionList {
    pub items: Vec<MediaCollection>,
    pub total: u64,
    pub page: u32,
    pub page_size: u32,
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct MediaYearCount {
    pub year: Option<i32>,
    pub count: u64,
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct MediaGenreCount {
    pub genre: String,
    pub count: u64,
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct MediaMonthCount {
    pub month: String,
    pub count: u64,
}
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct MediaStatistics {
    pub total: u64,
    pub planned: u64,
    pub watching: u64,
    pub completed: u64,
    pub movies: u64,
    pub tv: u64,
    pub by_year: Vec<MediaYearCount>,
    pub by_genre: Vec<MediaGenreCount>,
    pub by_month: Vec<MediaMonthCount>,
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct MediaAccount {
    pub id: String,
    pub display_name: String,
    pub avatar_url: Option<String>,
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct MediaSession {
    pub account: Option<MediaAccount>,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct MediaItem<T> {
    pub item: T,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct MediaItems<T> {
    pub items: Vec<T>,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct MediaCreate {
    pub content: MediaContent,
    pub request_id: String,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct MediaUpdate {
    pub version: u64,
    pub content: MediaContent,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct MediaVersion {
    pub version: u64,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct MediaImport {
    pub identity: MediaIdentity,
    pub request_id: String,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct MediaSync {
    pub version: u64,
    pub request_id: String,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct MediaPoster {
    pub version: u64,
    pub asset_id: String,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct MediaCollectionCreate {
    pub title: String,
    pub description: String,
    pub entry_ids: Vec<String>,
    pub request_id: String,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct MediaCollectionUpdate {
    pub version: u64,
    pub title: String,
    pub description: String,
    pub entry_ids: Vec<String>,
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn shared_types_round_trip_the_typescript_fixtures() {
        let sample: serde_json::Value = serde_json::from_str(include_str!(
            "../../../packages/media/contract/snapshots.json"
        ))
        .unwrap();
        let entries: Vec<MediaEntry> = serde_json::from_value(sample["entries"].clone()).unwrap();
        assert_eq!(serde_json::to_value(entries).unwrap(), sample["entries"]);
        let _: MediaList = serde_json::from_value(sample["listing"].clone()).unwrap();
        let _: MediaCapabilities = serde_json::from_value(sample["capabilities"].clone()).unwrap();
        let _: MediaJob = serde_json::from_value(sample["job"].clone()).unwrap();
        let _: MediaGallery = serde_json::from_value(sample["gallery"].clone()).unwrap();
        let _: MediaCandidate = serde_json::from_value(sample["candidate"].clone()).unwrap();
        let _: MediaCollection = serde_json::from_value(sample["collection"].clone()).unwrap();
        let _: MediaStatistics = serde_json::from_value(sample["statistics"].clone()).unwrap();
    }
}
