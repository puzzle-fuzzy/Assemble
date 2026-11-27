// 源库以只读连接和同一个事务读取；不碰账号、令牌、设备与日志。
import { Database } from "bun:sqlite";
import { createHash } from "node:crypto";
import { lstat, mkdir, writeFile } from "node:fs/promises";
import { dirname, resolve } from "node:path";

type RecordRow = {
  id: string;
  resource: string;
  version: number;
  data: string;
  created_at: number;
  updated_at: number;
};
export function snapshot(db: Database, sourceAccountId: string) {
  db.exec("BEGIN");
  try {
    const records = db
      .query<RecordRow, [string]>(
        "SELECT id,resource,version,data,created_at,updated_at FROM jarvis_records WHERE owner_id=? AND (resource IN ('schedules','reminders') OR (resource='preferences' AND (substr(json_extract(data,'$.key'),1,9)='personal.' OR json_extract(data,'$.key')='navigation.locations.v1'))) ORDER BY resource,id",
      )
      .all(sourceAccountId);
    if (
      !records.some(
        (row) => row.resource === "schedules" || row.resource === "reminders",
      )
    )
      throw new Error("SOURCE_PLANNER_EMPTY");
    const notices = db.query<RecordRow, [string, string, string]>(
      "SELECT id,resource,version,data,created_at,updated_at FROM jarvis_records WHERE owner_id=? AND resource='notifications' AND (json_extract(data,'$.scheduleId') IN (SELECT id FROM jarvis_records WHERE owner_id=? AND resource='schedules') OR json_extract(data,'$.reminderId') IN (SELECT id FROM jarvis_records WHERE owner_id=? AND resource='reminders')) ORDER BY id",
    );
    // 显式三次绑定，避免跨账户同名来源混入通知。
    const notifications = notices.all(
      sourceAccountId,
      sourceAccountId,
      sourceAccountId,
    );
    const rows = [...records, ...notifications].map((row) => ({
      id: row.id,
      resource: row.resource,
      version: row.version,
      data: JSON.parse(row.data) as unknown,
      createdAt: row.created_at,
      updatedAt: row.updated_at,
    }));
    const registry = rows
      .filter(
        (row) =>
          row.resource === "preferences" &&
          (row.data as { key?: string }).key === "navigation.locations.v1",
      )
      .sort((a, b) => b.updatedAt - a.updatedAt || b.id.localeCompare(a.id))[0];
    const navigation = registry
      ? (registry.data as { value: { entries: unknown[] } }).value.entries
      : [];
    const history = (table: string) =>
      db
        .query<Record<string, unknown>, [string]>(
          `SELECT * FROM ${table} WHERE owner_id=? ORDER BY schedule_id`,
        )
        .all(sourceAccountId)
        .map(({ owner_id: _, ...row }) => row);
    const result = {
      version: 1,
      sourceOrigin: "https://jarvis.yxswy.com",
      sourceAccountId,
      records: rows,
      navigation,
      occurrences: history("jarvis_schedule_occurrences"),
      baselines: history("jarvis_schedule_baselines"),
    };
    db.exec("COMMIT");
    return result;
  } catch (error) {
    db.exec("ROLLBACK");
    throw error;
  }
}

export async function exportPlanner(
  database: string,
  owner: string,
  output: string,
) {
  database = resolve(database);
  output = resolve(output);
  if (
    (await lstat(database)).isSymbolicLink() ||
    dirname(output) === output ||
    output === database ||
    output === dirname(database)
  )
    throw new Error("INVALID_EXPORT_PATH");
  const db = new Database(database, { readonly: true, create: false });
  let archive: ReturnType<typeof snapshot>;
  try {
    archive = snapshot(db, owner);
  } finally {
    db.close();
  }
  // 目录必须新建，不能覆盖此前的导出；生产脚本另以 umask 077 执行。
  await mkdir(output, { mode: 0o700 });
  const bytes = JSON.stringify(archive);
  await writeFile(resolve(output, "archive.json"), bytes, {
    mode: 0o600,
    flag: "wx",
  });
  await writeFile(
    resolve(output, "archive.sha256"),
    createHash("sha256").update(bytes).digest("hex"),
    { mode: 0o600, flag: "wx" },
  );
  return {
    records: archive.records.length,
    schedules: archive.records.filter((x) => x.resource === "schedules").length,
    reminders: archive.records.filter((x) => x.resource === "reminders").length,
    memos: archive.records.filter(
      (x) =>
        x.resource === "preferences" &&
        (x.data as { key: string }).key.startsWith("personal."),
    ).length,
    navigation: archive.navigation.length,
  };
}
if (import.meta.main) {
  const [database, owner, output, ...extra] = process.argv.slice(2);
  if (!database || !owner || !output || extra.length)
    throw new Error(
      "USAGE: export-jarvis.ts DATABASE SOURCE_ACCOUNT OUTPUT_DIRECTORY",
    );
  console.log(JSON.stringify(await exportPlanner(database, owner, output)));
}
