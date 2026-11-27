import { expect, test } from "bun:test";
import { actorPath, readRoute, userLibraryPath, wallPath } from "./navigation";

test("人物分页保留类型和页码，非法页码回到第一页", () => {
  const identity = { provider: "tmdb" as const, externalId: "1136406" };
  const path = actorPath(identity, "movie", 2);
  expect(readRoute(path)).toEqual({
    kind: "actor",
    identity,
    mediaType: "movie",
    page: 2,
  });
  expect(readRoute(`${actorPath(identity, "tv")}&page=-1`)).toEqual({
    kind: "actor",
    identity,
    mediaType: "tv",
    page: 1,
  });
});

test("个人公开主页只用账户 ID，筛选与三种状态分页不丢失归属", () => {
  const path = userLibraryPath("owner-1");
  const route = readRoute(
    `${path}?mediaType=tv&plannedPage=2&watchingPage=3&completedPage=4`,
  );
  expect(route.kind).toBe("wall");
  if (route.kind !== "wall") throw new Error("EXPECTED_OWNER_WALL");
  expect(route.scope).toBe("shared");
  expect(route.query.ownerId).toBe("owner-1");
  expect(route.query.mediaType).toBe("tv");
  expect(route.pages).toEqual({ planned: 2, watching: 3, completed: 4 });
  expect(wallPath(route.scope, route.query, route.pages)).toBe(
    `${path}?mediaType=tv&plannedPage=2&watchingPage=3&completedPage=4`,
  );
  expect(readRoute("/movies/users/owner-1/extra")).toEqual({ kind: "missing" });
  expect(readRoute("/movies/users/owner%2Fother")).toEqual({ kind: "missing" });
});
