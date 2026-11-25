export type MediaScope = "shared" | "private";
export type MediaType = "movie" | "tv";
export type MediaStatus = "planned" | "watching" | "completed";
export type MediaProvider = "tmdb" | "douban";
export interface MediaIdentity {
  provider: MediaProvider;
  mediaType: MediaType;
  externalId: string;
}
export interface MediaPersonIdentity {
  provider: MediaProvider;
  externalId: string;
}
export interface MediaImage {
  assetId: string;
  width: number;
  height: number;
  contentType: string;
}
export interface MediaCast {
  name: string;
  character: string;
  identity: MediaPersonIdentity | null;
  photo: MediaImage | null;
}
export interface MediaContent {
  mediaType: MediaType;
  title: string;
  originalTitle: string;
  year: number | null;
  releaseDate: string | null;
  overview: string;
  genres: string[];
  episodeCount: number | null;
  status: MediaStatus;
  watchedAt: string | null;
  review: string | null;
  notes: string | null;
  magnetUri: string | null;
}
export interface MediaEntry extends MediaContent {
  id: string;
  version: number;
  scope: MediaScope;
  cast: MediaCast[];
  identities: MediaIdentity[];
  poster: MediaImage | null;
  magnetAvailable: boolean;
  createdAt: string;
  updatedAt: string;
}
export interface MediaCounts {
  all: number;
  planned: number;
  watching: number;
  completed: number;
}
export interface MediaList {
  items: MediaEntry[];
  total: number;
  page: number;
  pageSize: number;
  counts: MediaCounts;
}
export interface MediaQuery {
  q?: string;
  page?: number;
  pageSize?: number;
  mediaType?: MediaType;
  status?: MediaStatus;
  genre?: string;
  year?: number;
  sort?: "updated" | "title" | "year" | "watched";
}
export interface MediaCapabilities {
  providers: { tmdb: boolean; douban: boolean };
  canMaintainShared: boolean;
  canMaintainPrivate: boolean;
}
export interface MediaCandidate {
  identity: MediaIdentity;
  title: string;
  originalTitle: string;
  year: number | null;
  overview: string;
  poster: MediaImage | null;
}
export interface MediaJob {
  id: string;
  scope: MediaScope;
  status:
    | "queued"
    | "running"
    | "completed"
    | "failed"
    | "cancelled"
    | "interrupted";
  stage: "metadata" | "images" | "saving" | null;
  completedItems: number | null;
  totalItems: number | null;
  entryId: string | null;
  errorCode: string | null;
}
export interface MediaGallery {
  entryId: string;
  version: number;
  items: MediaImage[];
}
export interface MediaActorCredits {
  actor: {
    identity: MediaPersonIdentity;
    name: string;
    photo: MediaImage | null;
  };
  items: MediaCandidate[];
}
export interface MediaCollection {
  id: string;
  version: number;
  title: string;
  description: string;
  poster: MediaImage | null;
  items: MediaEntry[];
  createdAt: string;
  updatedAt: string;
}
export interface MediaCollectionList {
  items: MediaCollection[];
  total: number;
  page: number;
  pageSize: number;
}
export interface MediaStatistics {
  total: number;
  planned: number;
  watching: number;
  completed: number;
  movies: number;
  tv: number;
  byYear: { year: number | null; count: number }[];
  byGenre: { genre: string; count: number }[];
  byMonth: { month: string; count: number }[];
}
export interface MediaSession {
  account: { id: string; displayName: string; avatarUrl: string | null } | null;
}
export type MediaImageVariant = "original" | "200" | "360" | "480";
export type ImageLoader = (
  image: MediaImage,
  variant: MediaImageVariant,
  signal?: AbortSignal,
) => Promise<string>;
