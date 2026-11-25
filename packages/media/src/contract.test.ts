import { expect, test } from "bun:test";
import fixture from "../contract/snapshots.json";
import {
  isMediaCandidate,
  isMediaCapabilities,
  isMediaCollection,
  isMediaEntry,
  isMediaGallery,
  isMediaJob,
  isMediaList,
  isMediaStatistics,
} from "./guard";

test("Rust 与 TypeScript 共用影视状态样本", () => {
  expect(fixture.entries.every(isMediaEntry)).toBe(true);
  expect(isMediaList(fixture.listing)).toBe(true);
  expect(isMediaCapabilities(fixture.capabilities)).toBe(true);
  expect(isMediaJob(fixture.job)).toBe(true);
  expect(isMediaGallery(fixture.gallery)).toBe(true);
  expect(isMediaCandidate(fixture.candidate)).toBe(true);
  expect(isMediaCollection(fixture.collection)).toBe(true);
  expect(isMediaStatistics(fixture.statistics)).toBe(true);
});
test("边界拒绝缺字段、错误枚举与非整数计数", () => {
  expect(isMediaEntry({ ...fixture.entries[0], notes: undefined })).toBe(false);
  expect(isMediaEntry({ ...fixture.entries[0], status: "future" })).toBe(false);
  expect(isMediaJob({ ...fixture.job, totalItems: -1 })).toBe(false);
  expect(isMediaList({ ...fixture.listing, total: 1.5 })).toBe(false);
});
