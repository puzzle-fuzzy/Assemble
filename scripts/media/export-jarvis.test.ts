import { Database } from "bun:sqlite";
import { afterEach, expect, test } from "bun:test";
import { mkdir, mkdtemp, readFile, rm, writeFile } from "node:fs/promises";
import { tmpdir } from "node:os";
import { dirname, join, resolve } from "node:path";
import { exportJarvis, sha256, watchedDate } from "./export-jarvis";

test("观看日期沿用源站上海日期，跨日的 UTC 时间不能截断成前一天", () => {
  expect(watchedDate(null)).toBeNull();
  expect(watchedDate("2026-11-25")).toBe("2026-11-25");
  expect(watchedDate("2026-09-12T16:00:00.000Z")).toBe("2026-09-13");
  expect(watchedDate("2026-11-22T00:00:00+08:00")).toBe("2026-11-22");
  expect(() => watchedDate("invalid")).toThrow("INVALID_WATCHED_DATE");
});

const temporary: string[] = [];
afterEach(async () => {
  for (const directory of temporary.splice(0)) {
    if (
      dirname(directory) !== resolve(tmpdir()) ||
      !directory.split(/[\\/]/).at(-1)?.startsWith("assemble-media-export-")
    )
      throw new Error("INVALID_TEST_DIRECTORY");
    await rm(directory, { recursive: true, force: true });
  }
});
async function fixture() {
  const directory = await mkdtemp(join(tmpdir(), "assemble-media-export-"));
  temporary.push(directory);
  const media = join(directory, "media");
  await mkdir(join(media, "assets"), { recursive: true });
  await mkdir(join(media, "archives"));
  const database = join(directory, "source.sqlite");
  const db = new Database(database);
  db.exec(`CREATE TABLE media_assets(id TEXT,scope TEXT,sha256 TEXT,storage_key TEXT,size INTEGER,width INTEGER,height INTEGER,media_type TEXT,variant TEXT,original_asset_id TEXT,created_at INTEGER);
CREATE TABLE media_entries(id TEXT,scope TEXT,version INTEGER,media_type TEXT,title TEXT,original_title TEXT,year INTEGER,release_date TEXT,overview TEXT,genres TEXT,episode_count INTEGER,status TEXT,watched_at TEXT,personal_review TEXT,notes TEXT,magnet_uri TEXT,primary_poster_id TEXT,created_at INTEGER,updated_at INTEGER);
CREATE TABLE media_entry_assets(id TEXT,entry_id TEXT,asset_id TEXT,position INTEGER);
CREATE TABLE media_entry_cast(entry_id TEXT,position INTEGER,identities TEXT,name TEXT,character TEXT,photo_asset_id TEXT);
CREATE TABLE media_cache(id TEXT,scope TEXT,provider TEXT,media_type TEXT,external_id TEXT,metadata TEXT,expires_at INTEGER,created_at INTEGER,updated_at INTEGER);
CREATE TABLE media_collections(id TEXT,version INTEGER,title TEXT,overview TEXT,cover_entry_id TEXT,created_at INTEGER,updated_at INTEGER);
CREATE TABLE media_collection_members(collection_id TEXT,entry_id TEXT,position INTEGER);
CREATE TABLE media_identities(entry_id TEXT,provider TEXT,media_type TEXT,external_id TEXT);
CREATE TABLE media_cache_assets(id TEXT);
CREATE TABLE media_jobs(id TEXT,session_id TEXT,status TEXT,input TEXT);
CREATE TABLE media_migration_runs(id TEXT);`);
  const bytes = Buffer.from("public image fixture");
  await writeFile(join(media, "assets", "one.jpg"), bytes);
  await writeFile(
    join(media, "archives", "source.json"),
    "original source archive",
  );
  const at = 1795660000000;
  function insert(table: string, row: Record<string, string | number | null>) {
    db.query(
      `INSERT INTO ${table} (${Object.keys(row).join(",")}) VALUES (${Object.keys(
        row,
      )
        .map(() => "?")
        .join(",")})`,
    ).run(...Object.values(row));
  }
  insert("media_assets", {
    id: "asset-one",
    scope: "public",
    sha256: sha256(bytes),
    storage_key: "one.jpg",
    size: bytes.length,
    width: 100,
    height: 150,
    media_type: "image/jpeg",
    variant: "original",
    original_asset_id: null,
    created_at: at,
  });
  insert("media_entries", {
    id: "movie-one",
    scope: "public",
    version: 4,
    media_type: "movie",
    title: "原有电影",
    original_title: "Film",
    year: 2020,
    release_date: null,
    overview: "原有简介",
    genres: '["剧情"]',
    episode_count: null,
    status: "completed",
    watched_at: "2026-11-25",
    personal_review: "保留观后感",
    notes: "管理员备注",
    magnet_uri: "magnet:?xt=urn:btih:0123456789012345678901234567890123456789",
    primary_poster_id: "gallery-one",
    created_at: at,
    updated_at: at + 1000,
  });
  insert("media_entry_assets", {
    id: "gallery-one",
    entry_id: "movie-one",
    asset_id: "asset-one",
    position: 0,
  });
  insert("media_entry_cast", {
    entry_id: "movie-one",
    position: 0,
    identities: '[{"provider":"tmdb","externalId":"person"}]',
    name: "演员",
    character: "角色",
    photo_asset_id: "asset-one",
  });
  insert("media_collections", {
    id: "collection-one",
    version: 2,
    title: "合集",
    overview: "合集简介",
    cover_entry_id: "movie-one",
    created_at: at,
    updated_at: at,
  });
  insert("media_collection_members", {
    collection_id: "collection-one",
    entry_id: "movie-one",
    position: 0,
  });
  insert("media_identities", {
    entry_id: "movie-one",
    provider: "tmdb",
    media_type: "movie",
    external_id: "42",
  });
  insert("media_cache", {
    id: "cached-one",
    scope: "public",
    provider: "tmdb",
    media_type: "movie",
    external_id: "43",
    metadata: JSON.stringify({
      schemaVersion: 2,
      media: {
        mediaType: "movie",
        title: "缓存电影",
        originalTitle: "Cache",
        year: 2021,
        releaseDate: null,
        overview: "已缓存",
        genres: [],
        cast: [],
        episodeCount: null,
        primaryPoster: { assetId: "asset-one" },
      },
    }),
    expires_at: at,
    created_at: at,
    updated_at: at,
  });
  insert("media_jobs", {
    id: "old-job",
    session_id: "synthetic-session-not-for-export",
    status: "completed",
    input: '{"identity":"42"}',
  });
  db.close();
  return { directory, media, database };
}
test("只读导出保留资料、观看历史、缓存、封面、合集顺序与所有文件，排除认证关联", async () => {
  const f = await fixture();
  const before = sha256(await readFile(f.database));
  const output = join(f.directory, "export");
  const report = await exportJarvis(f.database, f.media, output);
  expect(report).toMatchObject({
    entries: 1,
    collections: 1,
    catalog: 1,
    assets: 1,
    sourceFiles: 2,
  });
  const manifest = await readFile(join(output, "manifest.json"), "utf8");
  const bundle = JSON.parse(manifest);
  expect(manifest).not.toContain("synthetic-session-not-for-export");
  expect(bundle.entries[0].entry).toMatchObject({
    version: 4,
    status: "completed",
    watchedAt: "2026-11-25",
    review: "保留观后感",
    notes: "管理员备注",
    poster: { assetId: "asset-one" },
  });
  expect(bundle.catalog[0].entry.title).toBe("缓存电影");
  expect(bundle.catalog[0].entry.notes).toBeNull();
  expect(bundle.collections[0].entryIds).toEqual(["movie-one"]);
  expect(bundle.collections[0].coverEntryId).toBe("movie-one");
  for (const file of bundle.files)
    expect(sha256(await readFile(join(output, "files", file.sha256)))).toBe(
      file.sha256,
    );
  expect(sha256(await readFile(f.database))).toBe(before);
  await expect(exportJarvis(f.database, f.media, output)).rejects.toThrow();
});
test("私人范围必须先映射所有者，源图片损坏不能生成完成清单", async () => {
  const f = await fixture();
  const db = new Database(f.database);
  db.exec("UPDATE media_entries SET scope='private'");
  db.close();
  await expect(
    exportJarvis(f.database, f.media, join(f.directory, "private")),
  ).rejects.toThrow("PRIVATE_OWNER_MAPPING_REQUIRED");
  const again = new Database(f.database);
  again.exec("UPDATE media_entries SET scope='public'");
  again.close();
  await writeFile(join(f.media, "assets", "one.jpg"), "changed image");
  const output = join(f.directory, "corrupt");
  await expect(exportJarvis(f.database, f.media, output)).rejects.toThrow(
    "SOURCE_ASSET_INTEGRITY_FAILED",
  );
  expect(await Bun.file(join(output, "manifest.json")).exists()).toBe(false);
});
