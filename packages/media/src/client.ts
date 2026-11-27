import type {
  MediaActorCredits,
  MediaCandidate,
  MediaCapabilities,
  MediaCollection,
  MediaCollectionList,
  MediaContent,
  MediaEntry,
  MediaGallery,
  MediaIdentity,
  MediaJob,
  MediaList,
  MediaPersonIdentity,
  MediaProvider,
  MediaQuery,
  MediaScope,
  MediaStatistics,
  MediaType,
} from "./model";

/** 浏览器和桌面使用相同业务接口；认证留在各自的传输层。 */
export interface MediaClient {
  readonly scope: MediaScope;
  capabilities(signal?: AbortSignal): Promise<MediaCapabilities>;
  list(query?: MediaQuery, signal?: AbortSignal): Promise<MediaList>;
  get(id: string, signal?: AbortSignal): Promise<MediaEntry>;
  create(
    content: MediaContent,
    requestId: string,
    signal?: AbortSignal,
  ): Promise<MediaEntry>;
  update(
    id: string,
    version: number,
    content: MediaContent,
    signal?: AbortSignal,
  ): Promise<MediaEntry>;
  remove(id: string, version: number, signal?: AbortSignal): Promise<void>;
  search(
    provider: MediaProvider,
    mediaType: MediaType,
    q: string,
    signal?: AbortSignal,
  ): Promise<MediaCandidate[]>;
  catalog(
    identity: MediaIdentity,
    signal?: AbortSignal,
  ): Promise<MediaCandidate>;
  actor(
    identity: MediaPersonIdentity,
    mediaType: MediaType,
    query?: { page?: number; pageSize?: number },
    signal?: AbortSignal,
  ): Promise<MediaActorCredits>;
  import(
    identity: MediaIdentity,
    requestId: string,
    signal?: AbortSignal,
  ): Promise<MediaJob>;
  job(id: string, signal?: AbortSignal): Promise<MediaJob>;
  gallery(id: string, signal?: AbortSignal): Promise<MediaGallery>;
  syncGallery(
    id: string,
    version: number,
    requestId: string,
    signal?: AbortSignal,
  ): Promise<MediaJob>;
  selectPoster(
    id: string,
    version: number,
    assetId: string,
    signal?: AbortSignal,
  ): Promise<MediaEntry>;
}
export interface MediaCollectionContent {
  title: string;
  description: string;
  entryIds: string[];
}
export interface MediaCollectionsClient {
  list(
    query?: { page?: number; pageSize?: number },
    signal?: AbortSignal,
  ): Promise<MediaCollectionList>;
  get(id: string, signal?: AbortSignal): Promise<MediaCollection>;
  create(
    content: MediaCollectionContent,
    requestId: string,
    signal?: AbortSignal,
  ): Promise<MediaCollection>;
  update(
    id: string,
    version: number,
    content: MediaCollectionContent,
    signal?: AbortSignal,
  ): Promise<MediaCollection>;
  remove(id: string, version: number, signal?: AbortSignal): Promise<void>;
}
export type MediaStatisticsLoader = (
  signal?: AbortSignal,
) => Promise<MediaStatistics>;
