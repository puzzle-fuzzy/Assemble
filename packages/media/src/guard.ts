import type {
  MediaActorCredits,
  MediaCandidate,
  MediaCapabilities,
  MediaCollection,
  MediaCollectionList,
  MediaContent,
  MediaEntry,
  MediaGallery,
  MediaImage,
  MediaJob,
  MediaList,
  MediaSession,
  MediaStatistics,
} from "./model";

type Guard = (value: unknown) => boolean;
const record = (value: unknown): value is Record<string, unknown> =>
  value !== null && typeof value === "object" && !Array.isArray(value);
const text: Guard = (value) => typeof value === "string";
const integer: Guard = (value) =>
  typeof value === "number" && Number.isSafeInteger(value) && value >= 0;
const year: Guard = (value) =>
  typeof value === "number" &&
  Number.isSafeInteger(value) &&
  value >= 1800 &&
  value <= 9999;
const flag: Guard = (value) => typeof value === "boolean";
const nullable =
  (guard: Guard): Guard =>
  (value) =>
    value === null || guard(value);
const array =
  (guard: Guard): Guard =>
  (value) =>
    Array.isArray(value) && value.every(guard);
const oneOf =
  (...values: string[]): Guard =>
  (value) =>
    typeof value === "string" && values.includes(value);
const shape =
  (fields: Record<string, Guard>): Guard =>
  (value) =>
    record(value) &&
    Object.entries(fields).every(
      ([key, guard]) => Object.hasOwn(value, key) && guard(value[key]),
    );
const provider = oneOf("tmdb", "douban");
const mediaType = oneOf("movie", "tv");
const scope = oneOf("shared", "private");
const status = oneOf("planned", "watching", "completed");
const identity = shape({ provider, mediaType, externalId: text });
const person = shape({ provider, externalId: text });
const image = shape({
  assetId: text,
  width: integer,
  height: integer,
  contentType: oneOf("image/jpeg", "image/png", "image/webp"),
});
const cast = shape({
  name: text,
  character: text,
  identity: nullable(person),
  photo: nullable(image),
});
const contentFields = {
  published: flag,
  mediaType,
  title: text,
  originalTitle: text,
  year: nullable(year),
  releaseDate: nullable(text),
  overview: text,
  genres: array(text),
  episodeCount: nullable(integer),
  status,
  watchedAt: nullable(text),
  review: nullable(text),
  notes: nullable(text),
  magnetUri: nullable(text),
};
const entry = shape({
  ...contentFields,
  id: text,
  version: integer,
  scope,
  owner: nullable(
    shape({ id: text, displayName: text, avatarUrl: nullable(text) }),
  ),
  cast: array(cast),
  identities: array(identity),
  poster: nullable(image),
  magnetAvailable: flag,
  createdAt: text,
  updatedAt: text,
});
const candidate = shape({
  identity,
  title: text,
  originalTitle: text,
  year: nullable(year),
  overview: text,
  poster: nullable(image),
});
const collection = shape({
  id: text,
  version: integer,
  title: text,
  description: text,
  poster: nullable(image),
  items: array(entry),
  createdAt: text,
  updatedAt: text,
});
export const isMediaImage = (value: unknown): value is MediaImage =>
  image(value);
export const isMediaContent = (value: unknown): value is MediaContent =>
  shape(contentFields)(value);
export const isMediaEntry = (value: unknown): value is MediaEntry =>
  entry(value);
export const isMediaCandidate = (value: unknown): value is MediaCandidate =>
  candidate(value);
export const isMediaList = (value: unknown): value is MediaList =>
  shape({
    items: array(entry),
    total: integer,
    page: integer,
    pageSize: integer,
    counts: shape({
      all: integer,
      planned: integer,
      watching: integer,
      completed: integer,
    }),
  })(value);
export const isMediaCapabilities = (
  value: unknown,
): value is MediaCapabilities =>
  shape({
    providers: shape({ tmdb: flag, douban: flag }),
    canMaintainShared: flag,
    canMaintainPrivate: flag,
  })(value);
export const isMediaJob = (value: unknown): value is MediaJob =>
  shape({
    id: text,
    scope,
    status: oneOf(
      "queued",
      "running",
      "completed",
      "failed",
      "cancelled",
      "interrupted",
    ),
    stage: nullable(oneOf("metadata", "images", "saving")),
    completedItems: nullable(integer),
    totalItems: nullable(integer),
    entryId: nullable(text),
    errorCode: nullable(text),
  })(value);
export const isMediaGallery = (value: unknown): value is MediaGallery =>
  shape({ entryId: text, version: integer, items: array(image) })(value);
export const isMediaActorCredits = (
  value: unknown,
): value is MediaActorCredits =>
  shape({
    actor: shape({ identity: person, name: text, photo: nullable(image) }),
    items: array(candidate),
    total: integer,
    page: (value) => typeof value === "number" && integer(value) && value > 0,
    pageSize: (value) =>
      typeof value === "number" && integer(value) && value > 0 && value <= 48,
  })(value);
export const isMediaCollection = (value: unknown): value is MediaCollection =>
  collection(value);
export const isMediaCollectionList = (
  value: unknown,
): value is MediaCollectionList =>
  shape({
    items: array(collection),
    total: integer,
    page: integer,
    pageSize: integer,
  })(value);
export const isMediaStatistics = (value: unknown): value is MediaStatistics =>
  shape({
    total: integer,
    planned: integer,
    watching: integer,
    completed: integer,
    movies: integer,
    tv: integer,
    byYear: array(shape({ year: nullable(year), count: integer })),
    byGenre: array(shape({ genre: text, count: integer })),
    byMonth: array(shape({ month: text, count: integer })),
  })(value);
export const isMediaSession = (value: unknown): value is MediaSession =>
  shape({
    account: nullable(
      shape({ id: text, displayName: text, avatarUrl: nullable(text) }),
    ),
  })(value);
export function requireMedia<T>(
  value: unknown,
  guard: (input: unknown) => input is T,
): T {
  if (!guard(value)) throw new Error("INVALID_MEDIA_RESPONSE");
  return value;
}
