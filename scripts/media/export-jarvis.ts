// 只读导出 Jarvis 的影视业务；不打开身份表，不导出认证关联，不修改源库或源文件。
import { Database } from "bun:sqlite";
import { createHash } from "node:crypto";
import { lstat, mkdir, readdir, readFile, writeFile } from "node:fs/promises";
import { dirname, isAbsolute, join, relative, resolve } from "node:path";
import type {
  MediaCast,
  MediaEntry,
  MediaImage,
  MediaStatus,
} from "../../packages/media/src/model";

type Row = Record<string, string | number | null>;
const tables = [
  "media_assets",
  "media_cache",
  "media_cache_assets",
  "media_collection_members",
  "media_collections",
  "media_entries",
  "media_entry_assets",
  "media_entry_cast",
  "media_identities",
  "media_jobs",
  "media_migration_runs",
];
const credentialFields = new Set([
  "session_id",
  "sessionId",
  "token",
  "password",
  "cookie",
  "authorization",
  "accessToken",
  "refreshToken",
  "passwordHash",
]);
export const sha256 = (bytes: Uint8Array | string) =>
  createHash("sha256").update(bytes).digest("hex");
export function watchedDate(value: unknown): string | null {
  if (value === null || value === undefined) return null;
  if (typeof value !== "string") throw new Error("INVALID_WATCHED_DATE");
  if (/^\d{4}-\d{2}-\d{2}$/.test(value)) return value;
  if (!/^\d{4}-\d{2}-\d{2}T.*(?:Z|[+-]\d{2}:\d{2})$/.test(value))
    throw new Error("INVALID_WATCHED_DATE");
  const instant = new Date(value);
  if (!Number.isFinite(instant.getTime()))
    throw new Error("INVALID_WATCHED_DATE");
  // Jarvis 的观影日期按上海时区显示，新片库只存日期；原时间戳仍完整留在原始业务行里。
  const parts = new Intl.DateTimeFormat("en", {
    timeZone: "Asia/Shanghai",
    year: "numeric",
    month: "2-digit",
    day: "2-digit",
  }).formatToParts(instant);
  const part = (type: string) =>
    parts.find((value) => value.type === type)?.value;
  return `${part("year")}-${part("month")}-${part("day")}`;
}
const parsed = <T>(value: unknown, fallback: T): T =>
  value === null || value === undefined
    ? fallback
    : typeof value === "string"
      ? JSON.parse(value)
      : (value as T);
function clean(value: unknown): unknown {
  if (Array.isArray(value)) return value.map(clean);
  if (value && typeof value === "object")
    return Object.fromEntries(
      Object.entries(value)
        .filter(([key]) => !credentialFields.has(key))
        .map(([key, value]) => [key, clean(value)]),
    );
  return value;
}
const within = (root: string, path: string) => {
  const part = relative(root, path);
  return part === "" || (!part.startsWith("..") && !isAbsolute(part));
};

export async function exportJarvis(
  database: string,
  mediaRoot: string,
  output: string,
) {
  database = resolve(database);
  mediaRoot = resolve(mediaRoot);
  output = resolve(output);
  if (
    within(mediaRoot, output) ||
    within(output, mediaRoot) ||
    within(output, database) ||
    output === dirname(output)
  )
    throw new Error("INVALID_EXPORT_PATH");
  if (
    (await lstat(database)).isSymbolicLink() ||
    (await lstat(mediaRoot)).isSymbolicLink()
  )
    throw new Error("SYMLINK_SOURCE");
  const db = new Database(database, { readonly: true, create: false });
  const readTables = () =>
    Object.fromEntries(
      tables.map((table) => [
        table,
        db
          .query<Row, []>(`SELECT * FROM "${table}" ORDER BY rowid`)
          .all()
          .map((row) => clean(row) as Row),
      ]),
    );
  db.exec("BEGIN");
  try {
    const actual = db
      .query<{ name: string }, []>(
        "SELECT name FROM sqlite_master WHERE type='table' AND name LIKE 'media_%' ORDER BY name",
      )
      .all()
      .map((row) => row.name);
    if (JSON.stringify(actual) !== JSON.stringify([...tables].sort()))
      throw new Error("UNKNOWN_MEDIA_SCHEMA");
    const sourceTables = readTables();
    if (
      Object.values(sourceTables).some((rows) =>
        rows.some((row) => row.scope === "private"),
      )
    )
      throw new Error("PRIVATE_OWNER_MAPPING_REQUIRED");
    await mkdir(output, { mode: 0o700 });
    await mkdir(join(output, "files"), { mode: 0o700 });
    const files: { key: string; sha256: string; size: number }[] = [];
    async function walk(directory: string) {
      for (const item of (
        await readdir(directory, { withFileTypes: true })
      ).sort((a, b) => a.name.localeCompare(b.name, "en"))) {
        const file = join(directory, item.name);
        if (item.isSymbolicLink()) throw new Error("SYMLINK_SOURCE");
        if (item.isDirectory()) {
          await walk(file);
          continue;
        }
        if (!item.isFile()) throw new Error("UNSUPPORTED_SOURCE_FILE");
        const before = await lstat(file);
        const bytes = await readFile(file);
        const after = await lstat(file);
        if (
          before.size !== bytes.length ||
          before.size !== after.size ||
          before.mtimeMs !== after.mtimeMs ||
          before.ino !== after.ino
        )
          throw new Error("SOURCE_FILE_CHANGED");
        const hash = sha256(bytes);
        try {
          await writeFile(join(output, "files", hash), bytes, {
            flag: "wx",
            mode: 0o600,
          });
        } catch (error) {
          if ((error as NodeJS.ErrnoException).code !== "EEXIST") throw error;
        }
        files.push({
          key: relative(mediaRoot, file).split("\\").join("/"),
          sha256: hash,
          size: bytes.length,
        });
      }
    }
    await walk(mediaRoot);
    const fileMap = new Map(files.map((file) => [file.key, file]));
    const originalRows = sourceTables.media_assets.filter(
      (row) => row.variant === "original",
    );
    const canonical = new Map<string, Row>();
    const assetIds = new Map<string, string>();
    for (const row of originalRows) {
      const hash = String(row.sha256);
      const previous = canonical.get(hash);
      if (!previous) canonical.set(hash, row);
      assetIds.set(String(row.id), String((previous ?? row).id));
    }
    for (const row of sourceTables.media_assets) {
      if (row.variant !== "original") {
        const original = assetIds.get(String(row.original_asset_id));
        if (!original) throw new Error("MISSING_ORIGINAL_ASSET");
        assetIds.set(String(row.id), original);
      }
      const file = fileMap.get(`assets/${row.storage_key}`);
      if (!file || file.sha256 !== row.sha256 || file.size !== row.size)
        throw new Error("SOURCE_ASSET_INTEGRITY_FAILED");
    }
    const assets = [...canonical.values()].map((row) => ({
      id: String(row.id),
      sha256: String(row.sha256),
      createdAt: Number(row.created_at),
      variants: Object.fromEntries(
        sourceTables.media_assets
          .filter((asset) => assetIds.get(String(asset.id)) === row.id)
          .sort((a, b) => String(a.id).localeCompare(String(b.id)))
          .map((asset) => [String(asset.variant), String(asset.sha256)]),
      ),
    }));
    const assetMap = new Map(
      [...canonical.values()].map((row) => [String(row.id), row]),
    );
    function image(id: unknown): MediaImage | null {
      if (!id) return null;
      const canonicalId = assetIds.get(String(id));
      const asset = canonicalId ? assetMap.get(canonicalId) : undefined;
      if (!asset || !canonicalId) throw new Error("MISSING_IMAGE_REFERENCE");
      return {
        assetId: canonicalId,
        width: Number(asset.width),
        height: Number(asset.height),
        contentType: String(asset.media_type),
      };
    }
    function galleryImageId(id: unknown) {
      const value = image(id);
      if (!value) throw new Error("MISSING_IMAGE_REFERENCE");
      return value.assetId;
    }
    const identities = (entryId: unknown) =>
      sourceTables.media_identities
        .filter((row) => row.entry_id === entryId)
        .map((row) => ({
          provider: row.provider as "tmdb" | "douban",
          mediaType: row.media_type as "movie" | "tv",
          externalId: String(row.external_id),
        }));
    function cast(row: Record<string, unknown>, cached = false): MediaCast {
      const ids = parsed<{ provider: "tmdb" | "douban"; externalId: string }[]>(
        row.identities,
        [],
      );
      const photo = cached
        ? (row.photo as { assetId?: string } | null)?.assetId
        : row.photo_asset_id;
      return {
        name: String(row.name ?? ""),
        character: String(row.character ?? ""),
        identity: ids.find((id) => id.provider === "tmdb") ?? ids[0] ?? null,
        photo: image(photo),
      };
    }
    const entries = sourceTables.media_entries.map((row) => {
      const gallery = sourceTables.media_entry_assets
        .filter((link) => link.entry_id === row.id)
        .sort((a, b) => Number(a.position) - Number(b.position));
      const cover = gallery.find((link) => link.id === row.primary_poster_id);
      if (row.primary_poster_id && !cover)
        throw new Error("MISSING_COVER_REFERENCE");
      const entry: MediaEntry = {
        id: String(row.id),
        version: Number(row.version),
        scope: "shared",
        mediaType: row.media_type as "movie" | "tv",
        title: String(row.title),
        originalTitle: String(row.original_title ?? ""),
        year: row.year as number | null,
        releaseDate: row.release_date as string | null,
        overview: String(row.overview ?? ""),
        genres: parsed<string[]>(row.genres, []),
        episodeCount: row.episode_count as number | null,
        status: row.status as MediaStatus,
        watchedAt: watchedDate(row.watched_at),
        review: row.personal_review as string | null,
        notes: row.notes as string | null,
        magnetUri: row.magnet_uri as string | null,
        magnetAvailable: Boolean(row.magnet_uri),
        cast: sourceTables.media_entry_cast
          .filter((person) => person.entry_id === row.id)
          .sort((a, b) => Number(a.position) - Number(b.position))
          .map((person) => cast(person)),
        identities: identities(row.id),
        poster: image(cover?.asset_id),
        createdAt: new Date(Number(row.created_at)).toISOString(),
        updatedAt: new Date(Number(row.updated_at)).toISOString(),
      };
      return {
        entry,
        gallery: [
          ...new Set(gallery.map((link) => galleryImageId(link.asset_id))),
        ],
      };
    });
    const collections = sourceTables.media_collections.map((row) => ({
      id: String(row.id),
      version: Number(row.version),
      title: String(row.title),
      description: String(row.overview ?? ""),
      coverEntryId: row.cover_entry_id,
      entryIds: sourceTables.media_collection_members
        .filter((member) => member.collection_id === row.id)
        .sort((a, b) => Number(a.position) - Number(b.position))
        .map((member) => String(member.entry_id)),
      createdAt: Number(row.created_at),
      updatedAt: Number(row.updated_at),
    }));
    const catalog = sourceTables.media_cache.map((row) => {
      const metadata = parsed<{ media: Record<string, unknown> }>(
        row.metadata,
        { media: {} },
      ).media;
      const poster = (metadata.primaryPoster as { assetId?: string } | null)
        ?.assetId;
      const entry: MediaEntry = {
        id: String(row.id),
        version: 1,
        scope: "shared",
        mediaType: row.media_type as "movie" | "tv",
        title: String(metadata.title ?? ""),
        originalTitle: String(metadata.originalTitle ?? ""),
        year: metadata.year as number | null,
        releaseDate: metadata.releaseDate as string | null,
        overview: String(metadata.overview ?? ""),
        genres: parsed<string[]>(metadata.genres, []),
        episodeCount: metadata.episodeCount as number | null,
        status: "planned",
        watchedAt: null,
        review: null,
        notes: null,
        magnetUri: null,
        magnetAvailable: false,
        cast: parsed<Record<string, unknown>[]>(metadata.cast, []).map(
          (person) => cast(person, true),
        ),
        identities: [
          {
            provider: row.provider as "tmdb" | "douban",
            mediaType: row.media_type as "movie" | "tv",
            externalId: String(row.external_id),
          },
        ],
        poster: image(poster),
        createdAt: new Date(Number(row.created_at)).toISOString(),
        updatedAt: new Date(Number(row.updated_at)).toISOString(),
      };
      return { entry, expiresAt: Number(row.expires_at) };
    });
    db.exec("COMMIT");
    if (
      sha256(JSON.stringify(readTables())) !==
      sha256(JSON.stringify(sourceTables))
    )
      throw new Error("SOURCE_DATABASE_CHANGED");
    const bundle = {
      version: 1,
      source: "jarvis-media",
      capturedAt: Date.now(),
      sourceTables,
      files,
      assets,
      entries,
      collections,
      catalog,
    };
    const content = JSON.stringify(bundle);
    await writeFile(join(output, "manifest.json"), content, {
      flag: "wx",
      mode: 0o600,
    });
    return {
      sha256: sha256(content),
      entries: entries.length,
      collections: collections.length,
      catalog: catalog.length,
      assets: assets.length,
      sourceFiles: files.length,
      sourceBytes: files.reduce((sum, file) => sum + file.size, 0),
    };
  } finally {
    db.close();
  }
}

if (import.meta.main) {
  const [database, mediaRoot, output, ...extra] = Bun.argv.slice(2);
  if (!database || !mediaRoot || !output || extra.length)
    throw new Error("EXPECTED_DATABASE_MEDIA_OUTPUT");
  try {
    console.log(
      JSON.stringify(await exportJarvis(database, mediaRoot, output)),
    );
  } catch {
    console.error("MEDIA_EXPORT_FAILED");
    process.exitCode = 1;
  }
}
