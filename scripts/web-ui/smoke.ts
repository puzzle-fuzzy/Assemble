// 真实 Chrome 与临时 Rust 服务验身份页；不读取日常浏览器资料、密码、Cookie 或授权码。
import assert from "node:assert/strict";
import { randomBytes } from "node:crypto";
import { mkdir } from "node:fs/promises";
import { join } from "node:path";
import { fileURLToPath } from "node:url";

const root = fileURLToPath(new URL("../..", import.meta.url));
if (process.platform !== "win32")
  throw new Error("This local UI smoke runner currently requires Windows");
const area = join(root, ".artifacts/web-ui", randomBytes(8).toString("hex"));
await mkdir(area, { recursive: true });
const fixture = Bun.spawn(
  [join(root, "target/debug/examples/web-ui-preview.exe")],
  { stdin: "pipe", stdout: "pipe", stderr: "pipe" },
);
let chromeProcess: ReturnType<typeof Bun.spawn> | undefined;
let socket: WebSocket | undefined;
let session = "";
let nextId = 0;
let policyErrors = 0;
const pending = new Map<
  number,
  { resolve: (value: unknown) => void; reject: (error: Error) => void }
>();

async function until<T>(
  get: () => Promise<T | undefined>,
  timeout = 15000,
): Promise<T> {
  const started = Date.now();
  while (Date.now() - started < timeout) {
    const value = await get();
    if (value !== undefined) return value;
    await Bun.sleep(100);
  }
  throw new Error("Identity UI smoke timeout");
}
async function call<T = Record<string, unknown>>(
  method: string,
  params: Record<string, unknown> = {},
  target?: string,
): Promise<T> {
  return await new Promise<T>((resolve, reject) => {
    const id = ++nextId;
    const timer = setTimeout(() => {
      pending.delete(id);
      reject(new Error(`Chrome command timed out: ${method}`));
    }, 15000);
    pending.set(id, {
      resolve: (value) => {
        clearTimeout(timer);
        resolve(value as T);
      },
      reject: (error) => {
        clearTimeout(timer);
        reject(error);
      },
    });
    socket?.send(JSON.stringify({ id, method, params, sessionId: target }));
  });
}
async function evaluate<T>(expression: string): Promise<T> {
  const value = await call<{
    result: { value: T };
    exceptionDetails?: unknown;
  }>(
    "Runtime.evaluate",
    { expression, returnByValue: true, awaitPromise: true },
    session,
  );
  assert(!value.exceptionDetails, "UI evaluation failed");
  return value.result.value;
}
async function open(url: string, heading: string) {
  await call("Page.navigate", { url }, session);
  await until(async () =>
    (await evaluate<boolean>(
      `document.readyState==='complete' && document.querySelector('h1')?.textContent===${JSON.stringify(heading)}`,
    ))
      ? true
      : undefined,
  );
  await evaluate("document.fonts.ready");
  assert(
    await evaluate<boolean>(
      "[...document.fonts].some(font=>font.family.includes('Geist') && font.status==='loaded')",
    ),
    "shared font did not load",
  );
  assert(
    await evaluate<boolean>(
      "document.documentElement.scrollWidth <= innerWidth",
    ),
    "horizontal overflow",
  );
}
async function screenshot(name: string) {
  const result = await call<{ data: string }>(
    "Page.captureScreenshot",
    { format: "png", captureBeyondViewport: true },
    session,
  );
  await Bun.write(
    join(area, `${name}.png`),
    Buffer.from(result.data, "base64"),
  );
}
try {
  const reader = fixture.stdout.getReader();
  let ready = "";
  try {
    await until(async () => {
      const chunk = await reader.read();
      if (chunk.done) throw new Error("UI fixture stopped before ready");
      ready += new TextDecoder().decode(chunk.value);
      return ready.includes("\n") ? true : undefined;
    });
  } finally {
    reader.releaseLock();
  }
  const origin = /^READY (http:\/\/127\.0\.0\.1:\d+)/.exec(ready)?.[1];
  assert(origin, "invalid fixture origin");
  chromeProcess = Bun.spawn(
    [
      process.env.ASSEMBLE_TEST_CHROME ??
        "C:/Program Files/Google/Chrome/Application/chrome.exe",
      "--headless=new",
      "--no-first-run",
      "--no-default-browser-check",
      "--remote-debugging-port=0",
      `--user-data-dir=${join(area, "profile")}`,
      "about:blank",
    ],
    { stdout: "ignore", stderr: "ignore" },
  );
  const endpoint = await until(async () => {
    const file = Bun.file(join(area, "profile/DevToolsActivePort"));
    if (!(await file.exists())) return;
    const [port, path] = (await file.text()).trim().split("\n");
    return `ws://127.0.0.1:${port}${path}`;
  });
  socket = new WebSocket(endpoint);
  await new Promise<void>((resolve, reject) => {
    socket?.addEventListener("open", () => resolve(), { once: true });
    socket?.addEventListener(
      "error",
      () => reject(new Error("Chrome connection failed")),
      { once: true },
    );
  });
  socket.addEventListener("message", (event) => {
    const message = JSON.parse(String(event.data));
    if (message.id) {
      const item = pending.get(message.id);
      pending.delete(message.id);
      if (message.error)
        item?.reject(new Error("Chrome protocol request failed"));
      else item?.resolve(message.result);
    }
    if (
      message.method === "Log.entryAdded" &&
      message.params.entry.level === "error" &&
      /Content Security Policy|font/i.test(message.params.entry.text)
    )
      policyErrors++;
  });
  const target = await call<{ targetId: string }>("Target.createTarget", {
    url: "about:blank",
  });
  session = (
    await call<{ sessionId: string }>("Target.attachToTarget", {
      targetId: target.targetId,
      flatten: true,
    })
  ).sessionId;
  await call("Page.enable", {}, session);
  await call("Log.enable", {}, session);
  await call(
    "Emulation.setDeviceMetricsOverride",
    { width: 1100, height: 900, deviceScaleFactor: 1, mobile: false },
    session,
  );
  await call(
    "Emulation.setEmulatedMedia",
    { features: [{ name: "prefers-color-scheme", value: "light" }] },
    session,
  );
  await open(`${origin}/web/login`, "登录");
  const light = await evaluate<string>(
    "getComputedStyle(document.querySelector('button')).backgroundColor",
  );
  await screenshot("login-light");
  await evaluate(
    "document.querySelector('[name=email]').value='missing@example.test';document.querySelector('[name=password]').value='synthetic-invalid-value';document.querySelector('form').requestSubmit()",
  );
  await until(async () =>
    (await evaluate<boolean>("!!document.querySelector('[role=alert]')"))
      ? true
      : undefined,
  );
  assert(
    await evaluate<boolean>(
      "document.querySelector('[role=alert]').textContent.includes('邮箱或密码不对')",
    ),
  );
  await screenshot("login-error");
  await open(`${origin}/web/register`, "注册 Assemble");
  await screenshot("register");
  await open(`${origin}/web/forgot`, "找回或设置密码");
  await open(`${origin}/web/verify`, "验证邮箱");
  await open(`${origin}/__preview/page/verify-confirm`, "验证邮箱并注册");
  await screenshot("verify-confirm");
  await open(`${origin}/__preview/page/github`, "选择账户主邮箱");
  await open(`${origin}/__preview/page/login-github`, "登录");
  await screenshot("login-github");
  await open(`${origin}/__preview/page/success`, "密码已更新");
  await open(`${origin}/__preview/session`, "账户");
  await screenshot("account");
  await open(`${origin}/__preview/authorize/allow`, "确认登录");
  await screenshot("authorize-light");
  await evaluate(
    "document.querySelector('[name=choice][value=allow]').click()",
  );
  await until(async () =>
    (await evaluate<boolean>("document.body.textContent==='allowed'"))
      ? true
      : undefined,
  );
  await open(`${origin}/__preview/authorize/deny`, "确认登录");
  await evaluate("document.querySelector('[name=choice][value=deny]').click()");
  await until(async () =>
    (await evaluate<boolean>("document.body.textContent==='denied'"))
      ? true
      : undefined,
  );
  await call(
    "Emulation.setDeviceMetricsOverride",
    { width: 360, height: 780, deviceScaleFactor: 1, mobile: false },
    session,
  );
  await call(
    "Emulation.setEmulatedMedia",
    { features: [{ name: "prefers-color-scheme", value: "dark" }] },
    session,
  );
  await open(`${origin}/__preview/authorize/allow`, "确认登录");
  await screenshot("authorize-narrow-dark");
  await evaluate("document.querySelector('[name=choice][value=deny]').click()");
  await until(async () =>
    (await evaluate<boolean>("document.body.textContent==='denied'"))
      ? true
      : undefined,
  );
  await call("Page.navigate", { url: `${origin}/web/account` }, session);
  await until(async () =>
    (await evaluate<boolean>(
      "document.querySelector('h1')?.textContent==='账户'",
    ))
      ? true
      : undefined,
  );
  await evaluate(
    "document.querySelector('form[action=\"/web/logout\"]').requestSubmit()",
  );
  await until(async () =>
    (await evaluate<boolean>(
      "document.querySelector('h1')?.textContent==='已退出'",
    ))
      ? true
      : undefined,
  );
  await screenshot("signed-out-narrow-dark");
  await open(`${origin}/web/login?next=%2Fadmin%2F`, "登录管理控制台");
  assert.notEqual(
    await evaluate<string>(
      "getComputedStyle(document.querySelector('button')).backgroundColor",
    ),
    light,
  );
  await screenshot("admin-login-narrow-dark");
  assert.equal(policyErrors, 0, "browser reported CSP/font errors");
  console.log(`Identity UI Chrome smoke passed: ${area}`);
} finally {
  if (socket?.readyState === WebSocket.OPEN) {
    try {
      await call("Browser.close");
    } catch {}
    socket.close();
  }
  if (chromeProcess) {
    chromeProcess.kill();
    await chromeProcess.exited;
  }
  fixture.stdin.write("stop\n");
  fixture.stdin.end();
  assert.equal(await fixture.exited, 0, "UI fixture cleanup failed");
  const reader = fixture.stdout.getReader();
  let tail = "";
  try {
    for (;;) {
      const chunk = await reader.read();
      if (chunk.done) break;
      tail += new TextDecoder().decode(chunk.value);
    }
  } finally {
    reader.releaseLock();
  }
  assert(tail.includes("CLEANED"));
}
