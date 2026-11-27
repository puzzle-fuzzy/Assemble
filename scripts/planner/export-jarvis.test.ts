import { Database } from "bun:sqlite";
import { expect, test } from "bun:test";
import { snapshot } from "./export-jarvis";

test("只导出指定账户的日程和私人资料，保留版本与时间，不读凭据和其他业务", () => {
  const db = new Database(":memory:");
  db.exec(
    "CREATE TABLE jarvis_records(id TEXT,owner_id TEXT,resource TEXT,version INTEGER,data TEXT,created_at INTEGER,updated_at INTEGER);CREATE TABLE jarvis_schedule_occurrences(owner_id TEXT,schedule_id TEXT,occurrence_at INTEGER,notification_id TEXT,state TEXT,legacy_epoch INTEGER);CREATE TABLE jarvis_schedule_baselines(owner_id TEXT,schedule_id TEXT,cutoff_at INTEGER,skip_through_at INTEGER)",
  );
  const insert = (owner: string, resource: string, id: string, data: unknown) =>
    db
      .query("INSERT INTO jarvis_records VALUES(?,?,?,3,?,123,456)")
      .run(id, owner, resource, JSON.stringify(data));
  insert("alice", "schedules", "s1", {
    title: "上海东方艺术中心",
    startAt: "2026-11-29T06:30:00Z",
  });
  insert("alice", "reminders", "r1", { title: "出发" });
  insert("alice", "preferences", "p1", {
    key: "personal.memory:2026-11-20",
    value: { notes: "原文\n第二行" },
  });
  insert("alice", "preferences", "private-key", {
    key: "service.config",
    value: { token: "must-not-export" },
  });
  insert("alice", "devices", "d1", { credential: "must-not-export" });
  insert("bob", "schedules", "s2", { title: "别人的日程" });
  insert("bob", "notifications", "other", {
    scheduleId: "s1",
    title: "不能混入",
  });
  insert("alice", "notifications", "n1", { scheduleId: "s1", read: true });
  const result = snapshot(db, "alice");
  expect(result.records.map((r) => r.id).sort()).toEqual([
    "n1",
    "p1",
    "r1",
    "s1",
  ]);
  expect(result.records[0]?.version).toBe(3);
  expect(JSON.stringify(result)).not.toContain("must-not-export");
  expect(
    db.query("SELECT count(*) AS total FROM jarvis_records").get(),
  ).toEqual({ total: 8 });
  expect(() => snapshot(db, "missing")).toThrow("SOURCE_PLANNER_EMPTY");
  db.close();
});
