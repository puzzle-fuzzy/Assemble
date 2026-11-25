// 使用独立 Chrome 配置和随机命名的宿主登记；结束时撤销登记、关闭夹具并清除桥接凭据。

import assert from "node:assert/strict";
import { createHash, randomBytes } from "node:crypto";
import { copyFile, cp, mkdir, rm } from "node:fs/promises";
import { join, resolve } from "node:path";
import { fileURLToPath } from "node:url";

const root = fileURLToPath(new URL("../..", import.meta.url));
const nonce = randomBytes(8).toString("hex");
const area = join(root, ".artifacts/browser-tests", nonce);
const native = Bun.argv.includes("--native");
if (process.platform !== "win32")
  throw new Error(
    "This local Chrome/native smoke runner currently requires Windows",
  );
const browser =
  process.env.ASSEMBLE_TEST_CHROME ??
  "C:/Program Files/Google/Chrome/Application/chrome.exe";
const extension = join(area, "extension");
await mkdir(area, { recursive: true });
await cp(join(root, "apps/browser-extension/dist"), extension, {
  recursive: true,
});
const manifest = await Bun.file(join(extension, "manifest.json")).json();
const id = createHash("sha256")
  .update(Buffer.from(manifest.key, "base64"))
  .digest("hex")
  .slice(0, 32)
  .replace(/[0-9a-f]/g, (v) =>
    String.fromCharCode(97 + Number.parseInt(v, 16)),
  );
const appId = `com.puzzlefuzzy.assemble.smoke.${nonce}`;
const hostName = `com.puzzlefuzzy.assemble.smoke_${nonce}`;
const registry = `HKCU\\Software\\Google\\Chrome\\NativeMessagingHosts\\${hostName}`;
const nativeDir = join(area, "native");
const outputs = join(area, "files");
const fixtureExe = join(root, "target/debug/examples/browser-smoke.exe");
let registered = false;
let fixture: ReturnType<typeof Bun.spawn<"pipe", "pipe", "pipe">> | undefined;
const site = Bun.serve({
  hostname: "127.0.0.1",
  port: 0,
  fetch: () =>
    new Response(
      '<!doctype html><title>公开视频 &lt;fixture&gt;</title><video preload="none" src="https://interactive-examples.mdn.mozilla.net/media/cc0-videos/flower.mp4"></video><a href="/fixture.m3u8?signature=fixture-secret">M3U8</a>',
      { headers: { "content-type": "text/html; charset=utf-8" } },
    ),
});
const chromeProcess = Bun.spawn(
  [
    browser,
    "--headless=new",
    "--no-first-run",
    "--no-default-browser-check",
    "--enable-unsafe-extension-debugging",
    "--remote-debugging-port=0",
    `--user-data-dir=${join(area, "profile")}`,
    "about:blank",
  ],
  { stdout: "ignore", stderr: "ignore" },
);
let socket: WebSocket | undefined;
let nextId = 0;
const pending = new Map<
  number,
  {
    method: string;
    resolve: (value: unknown) => void;
    reject: (reason: Error) => void;
  }
>();
async function until<T>(
  get: () => Promise<T | undefined>,
  timeout = 15000,
): Promise<T> {
  const start = Date.now();
  while (Date.now() - start < timeout) {
    const value = await get();
    if (value !== undefined) return value;
    await Bun.sleep(100);
  }
  throw new Error("Chrome smoke timeout");
}
async function call<T = Record<string, unknown>>(
  method: string,
  params: Record<string, unknown> = {},
  sessionId?: string,
): Promise<T> {
  const id = ++nextId;
  return await new Promise<T>((resolve, reject) => {
    const timer = setTimeout(() => {
      pending.delete(id);
      reject(new Error(`Chrome command timed out: ${method}`));
    }, 15000);
    pending.set(id, {
      method,
      resolve: (value) => {
        clearTimeout(timer);
        resolve(value as T);
      },
      reject: (error) => {
        clearTimeout(timer);
        reject(error);
      },
    });
    socket?.send(JSON.stringify({ id, method, params, sessionId }));
  });
}
async function expression<T>(session: string, code: string): Promise<T> {
  const reply = await call<{
    result: { value: T };
    exceptionDetails?: unknown;
  }>(
    "Runtime.evaluate",
    { expression: code, awaitPromise: true, returnByValue: true },
    session,
  );
  assert(!reply.exceptionDetails, "fixture evaluation failed");
  return reply.result.value;
}
try {
  // 正反两种测试都只调用本次独有宿主，绝不意外唤起用户已安装的连接组件。
  const background = join(extension, "background.js");
  const script = await Bun.file(background).text();
  assert(script.includes("com.puzzlefuzzy.assemble"));
  await Bun.write(
    background,
    script.replaceAll("com.puzzlefuzzy.assemble", hostName),
  );
  if (native) {
    await mkdir(nativeDir, { recursive: true });
    await copyFile(
      join(
        root,
        `target/${Bun.argv.includes("--release-host") ? "release" : "debug"}/assemble-browser-host.exe`,
      ),
      join(nativeDir, "assemble-browser-host.exe"),
    );
    await Bun.write(
      join(nativeDir, "assemble-browser-host.json"),
      JSON.stringify({ extensionId: id, desktopPath: fixtureExe, appId }),
    );
    const descriptor = join(nativeDir, "host.json");
    await Bun.write(
      descriptor,
      JSON.stringify({
        name: hostName,
        description: "Isolated Assemble smoke fixture",
        path: join(nativeDir, "assemble-browser-host.exe"),
        type: "stdio",
        allowed_origins: [`chrome-extension://${id}/`],
      }),
    );
    const check = Bun.spawn(["reg.exe", "query", registry], {
      stdout: "ignore",
      stderr: "ignore",
    });
    assert.notEqual(await check.exited, 0, "test registry key already exists");
    const added = Bun.spawn(
      [
        "reg.exe",
        "add",
        registry,
        "/ve",
        "/t",
        "REG_SZ",
        "/d",
        descriptor,
        "/f",
      ],
      { stdout: "ignore", stderr: "ignore" },
    );
    assert.equal(await added.exited, 0);
    registered = true;
    fixture = Bun.spawn([fixtureExe, appId, outputs], {
      stdin: "pipe",
      stdout: "pipe",
      stderr: "pipe",
    });
    const reader = fixture.stdout.getReader();
    let ready = "";
    try {
      await until(async () => {
        const chunk = await reader.read();
        if (chunk.done) throw new Error("native fixture stopped before ready");
        ready += new TextDecoder().decode(chunk.value);
        return ready.includes("READY") ? true : undefined;
      });
    } finally {
      reader.releaseLock();
    }
  }
  const connection = await until(async () => {
    const path = Bun.file(join(area, "profile/DevToolsActivePort"));
    if (!(await path.exists())) return;
    const [port, pathPart] = (await path.text()).trim().split("\n");
    return { port, pathPart };
  });
  socket = new WebSocket(
    `ws://127.0.0.1:${connection.port}${connection.pathPart}`,
  );
  await new Promise<void>((resolve, reject) => {
    socket?.addEventListener("open", () => resolve(), { once: true });
    socket?.addEventListener(
      "error",
      () => reject(new Error("Chrome debugging connection failed")),
      { once: true },
    );
  });
  socket.addEventListener("message", (event) => {
    const message = JSON.parse(String(event.data)) as {
      id?: number;
      result?: unknown;
      error?: { message: string };
    };
    if (!message.id) return;
    const request = pending.get(message.id);
    if (!request) return;
    pending.delete(message.id);
    if (message.error)
      request.reject(new Error(`${request.method}: ${message.error.message}`));
    else request.resolve(message.result);
  });
  const loaded = await call<{ id: string }>("Extensions.loadUnpacked", {
    path: extension,
  });
  assert.equal(loaded.id, id);
  const tab = await call<{ targetId: string }>("Target.createTarget", {
    url: `http://127.0.0.1:${site.port}/`,
    forTab: true,
  });
  await call("Target.activateTarget", { targetId: tab.targetId });
  const page = await until(async () => {
    const result = await call<{
      targetInfos: { type: string; url: string; targetId: string }[];
    }>("Target.getTargets");
    return result.targetInfos.find(
      (target) =>
        target.type === "page" &&
        target.url === `http://127.0.0.1:${site.port}/`,
    );
  });
  const pageSession = await call<{ sessionId: string }>(
    "Target.attachToTarget",
    { targetId: page.targetId, flatten: true },
  );
  await until(async () =>
    (await expression<boolean>(
      pageSession.sessionId,
      "document.readyState==='complete' && !!document.querySelector('video')",
    ))
      ? true
      : undefined,
  );
  await call("Extensions.triggerAction", { id, targetId: tab.targetId });
  type Target = { targetId: string; url: string; type: string };
  const popup = await until(async () => {
    const result = await call<{ targetInfos: Target[] }>("Target.getTargets");
    return result.targetInfos.find(
      (target) => target.url === `chrome-extension://${id}/popup.html`,
    );
  });
  const attached = await call<{ sessionId: string }>("Target.attachToTarget", {
    targetId: popup.targetId,
    flatten: true,
  });
  const session = attached.sessionId;
  await until(async () => {
    const count = await expression<number>(
      session,
      "document.querySelectorAll('li').length",
    );
    return count >= 2 ? count : undefined;
  });
  const body = await expression<string>(session, "document.body.innerText");
  assert(!body.includes("fixture-secret"));
  assert(body.includes("M3U8"));
  assert(body.includes("公开视频 <fixture>"));
  const screenshot = await call<{ data: string }>(
    "Page.captureScreenshot",
    { format: "png" },
    session,
  );
  await Bun.write(
    join(area, "popup.png"),
    Buffer.from(screenshot.data, "base64"),
  );
  await expression(session, "document.querySelector('li button').click()");
  if (native) {
    await until(async () => {
      const value = await expression<string>(
        session,
        "document.querySelector('li button')?.textContent || ''",
      );
      return value.includes("已加入") ? true : undefined;
    }, 60000);
    await until(async () => {
      const file = Bun.file(join(outputs, "history.json"));
      if (!(await file.exists())) return;
      const history = await file.json();
      const task = history.records[0]?.view;
      if (task?.phase === "failed")
        throw new Error(`Native download failed: ${task.errorCode}`);
      return task?.phase === "completed" ? true : undefined;
    }, 90000);
    const history = await Bun.file(join(outputs, "history.json")).json();
    assert.equal(history.records.length, 1);
    assert.equal(history.records[0].sha256.length, 64);
    assert(history.records[0].view.receivedBytes > 0);
    assert(!JSON.stringify(history).includes("flower.mp4"));
  } else {
    await until(async () => {
      const text = await expression<string>(session, "document.body.innerText");
      return text.includes("未连接到 Assemble") ? true : undefined;
    });
  }
  const worker = await until(async () => {
    const result = await call<{ targetInfos: Target[] }>("Target.getTargets");
    return result.targetInfos.find(
      (target) =>
        target.type === "service_worker" &&
        target.url === `chrome-extension://${id}/background.js`,
    );
  });
  const workerSession = await call<{ sessionId: string }>(
    "Target.attachToTarget",
    { targetId: worker.targetId, flatten: true },
  );
  await call("Target.closeTarget", { targetId: tab.targetId });
  await until(async () =>
    (await expression<number>(
      workerSession.sessionId,
      "(async()=>Object.keys(await chrome.storage.session.get(null)).filter(key=>key.startsWith('sources:')).length)()",
    )) === 0
      ? true
      : undefined,
  );
  console.log(
    `Chrome extension smoke passed (${native ? "real native host + MP4 download" : "discovery + connector error"})`,
  );
  console.log(`Screenshot: ${join(area, "popup.png")}`);
} finally {
  if (socket?.readyState === WebSocket.OPEN) {
    try {
      await call("Browser.close");
    } catch {}
    socket.close();
  }
  chromeProcess.kill();
  await chromeProcess.exited;
  const cleanup = await Promise.allSettled([
    (async () => {
      if (fixture) {
        fixture.stdin.write("stop\n");
        fixture.stdin.end();
        const code = await fixture.exited;
        const reader = fixture.stdout.getReader();
        let tail = "";
        for (;;) {
          const chunk = await reader.read();
          if (chunk.done) break;
          tail += new TextDecoder().decode(chunk.value);
        }
        reader.releaseLock();
        assert.equal(code, 0);
        assert(tail.includes("CLEANED"));
      }
    })(),
    (async () => {
      if (registered) {
        const removed = Bun.spawn(["reg.exe", "delete", registry, "/f"], {
          stdout: "ignore",
          stderr: "ignore",
        });
        assert.equal(await removed.exited, 0);
      }
    })(),
  ]);
  site.stop(true);
  // 配置目录留下供检查；不递归删除浏览器配置，不接触日常 Chrome 资料。
  assert(resolve(area).startsWith(resolve(root, ".artifacts/browser-tests")));
  await rm(join(nativeDir, "assemble-browser-host.json"), { force: true });
  for (const result of cleanup)
    if (result.status === "rejected") {
      console.error("Chrome smoke fixture cleanup failed");
      process.exitCode = 1;
    }
}
