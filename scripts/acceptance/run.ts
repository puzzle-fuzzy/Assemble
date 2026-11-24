// 一条命令起好本机验收环境，并在登录之后自动核对收发。只在 macOS 上用。
//
//   bun run acceptance           起环境、等你登录、自动核对
//   bun run acceptance stop      收摊：停掉替身设备与本脚本起的服务端
//
// 需要你亲手做的只有三件：在应用里登录、往回发一个文件、从菜单栏退出应用。
// 其余的——起服务端、建账户、让替身设备发文件、核对落盘的摘要与回执——都由这个脚本做，
// 每一步的结果如实打印。它只在屏幕上显示文字、启动应用，不替你点任何东西。
//
// 它起的东西都在本机：服务端的数据在 data/local/server；替身设备“工作电脑”的令牌、
// 它收到的文件和全部日志在 data/local/acceptance；桌面应用是现场构建的发布版，
// 连的是 http://127.0.0.1:8787。中途想停：按 Ctrl+C，再运行 `bun run acceptance stop`。

import { spawn } from "node:child_process";
import {
  closeSync,
  mkdirSync,
  openSync,
  readFileSync,
  rmSync,
  statSync,
  writeFileSync,
} from "node:fs";
import { writeFile } from "node:fs/promises";
import { homedir } from "node:os";
import { join, relative } from "node:path";
import { fileURLToPath } from "node:url";
import {
  findLanded,
  formatSize,
  markerLines,
  type Options,
  parseArguments,
  randomFile,
  sha256,
  sha256File,
  USAGE,
} from "./helpers";

const root = fileURLToPath(new URL("../..", import.meta.url));
const local = join(root, "data/local");
const work = join(local, "acceptance");
const serverData = join(local, "server");
const origin = "http://127.0.0.1:8787";

const serverBin = join(root, "target/debug/assemble-server");
const fakeBin = join(root, "target/debug/examples/fake_device");
const desktopBin = join(root, "target/release/assemble-desktop");

/** 替身设备的名字：面板与设备列表里显示的就是它。 */
const fakeName = "工作电脑";
const tokenFile = join(work, "work-pc.json");
/** 替身设备收到的东西落在这里。 */
const fakeInbox = join(work, "received-by-work-pc");
const outbox = join(work, "outbox");
const log = {
  server: join(work, "server.log"),
  fake: join(work, "work-pc.log"),
  desktop: join(work, "desktop.log"),
};
const pidFile = {
  server: join(work, "server.pid"),
  fake: join(work, "work-pc.pid"),
};

/** 桌面端没改过接收位置时，收到的文件落在这里。 */
const defaultInbox = join(homedir(), "Downloads", "Assemble");

const NOTE_NAME = "Assemble 验收说明.txt";
const NOTE_TEXT = [
  "这是 Assemble 验收时，由替身设备“工作电脑”发给这台电脑的文件。",
  "",
  "它出现在这里，说明这一条路是通的：",
  "服务端 → 这台电脑上的 Assemble → 核对长度与摘要 → 落进接收文件夹 → 弹通知 → 回执。",
  "",
  "看完可以删掉。",
  "",
].join("\n");
const BLOB_NAME = "Assemble 验收数据.bin";
const BLOB_BYTES = 2 * 1024 * 1024;

/** 脚本看不见、只能由人确认的几样。 */
const BY_HAND = [
  "收到文件时弹了系统通知；点它，访达打开并选中那个文件。",
  "“传输”卡片里：收到的两条写着“已保存”，你发出去的那条写着“已送达”。",
  "把一个文件夹拖进 Assemble 窗口：应当提示“只能发送文件，不能发送文件夹。”。",
  "“云盘”卡片：点“上传…”存一个文件，再点“取回”，最后点“删除”。",
  "让应用挂着放上十几分钟，再打开窗口，看是不是空白。",
];

// ---- 输出 ----

interface Check {
  ok: boolean;
  text: string;
}

function step(title: string): void {
  console.log(`\n▶ ${title}`);
}

function note(text: string): void {
  console.log(`  ${text}`);
}

function record(checks: Check[], ok: boolean, text: string): void {
  checks.push({ ok, text });
  console.log(`  ${ok ? "✓" : "✗"} ${text}`);
}

/** 给人看的路径：仓库里的写成相对仓库的，家目录里的写成 `~/…`，其余照原样。 */
function shown(path: string): string {
  const inside = relative(root, path);
  if (!inside.startsWith("..")) return inside;
  const home = homedir();
  return path === home || path.startsWith(`${home}/`)
    ? `~${path.slice(home.length)}`
    : path;
}

function abort(reason: string): number {
  console.error(`\n✗ ${reason}`);
  console.error(
    "\n环境留着不动，方便排查；不用了就收摊：bun run acceptance stop",
  );
  return 1;
}

// ---- 进程与文件 ----

interface Finished {
  code: number;
  stdout: string;
  stderr: string;
}

/** 这个终端里碰巧设着的 `ASSEMBLE_*` 不带给孩子：服务端的配置只认本脚本给的那几项。 */
const cleanEnv = Object.fromEntries(
  Object.entries(process.env).filter(([key]) => !key.startsWith("ASSEMBLE_")),
);

function run(
  command: string,
  args: string[],
  env: NodeJS.ProcessEnv = {},
  cwd = root,
): Promise<Finished> {
  return new Promise((resolve, reject) => {
    const child = spawn(command, args, {
      cwd,
      env: { ...cleanEnv, ...env },
      stdio: ["ignore", "pipe", "pipe"],
    });
    let stdout = "";
    let stderr = "";
    child.stdout.setEncoding("utf8").on("data", (chunk: string) => {
      stdout += chunk;
    });
    child.stderr.setEncoding("utf8").on("data", (chunk: string) => {
      stderr += chunk;
    });
    child.on("error", reject);
    child.on("close", (code) => resolve({ code: code ?? -1, stdout, stderr }));
  });
}

/** 输出直接打到这个终端上（编译的进度、设密码时的提示），只返回退出码。 */
function inherit(
  command: string,
  args: string[],
  env: NodeJS.ProcessEnv = {},
  cwd = root,
): Promise<number> {
  return new Promise((resolve, reject) => {
    const child = spawn(command, args, {
      cwd,
      env: { ...cleanEnv, ...env },
      stdio: "inherit",
    });
    child.on("error", reject);
    child.on("close", (code) => resolve(code ?? -1));
  });
}

/** 起一个在本脚本退出之后继续运行的进程：输出写进日志，进程号记进文件。 */
function startDetached(
  command: string,
  args: string[],
  settings: { logFile: string; pidFile?: string; env?: NodeJS.ProcessEnv },
): void {
  const output = openSync(settings.logFile, "w");
  const child = spawn(command, args, {
    cwd: root,
    env: { ...cleanEnv, ...settings.env },
    stdio: ["ignore", output, output],
    detached: true,
  });
  closeSync(output);
  if (child.pid === undefined) throw new Error(`${command} 没有启动起来。`);
  child.unref();
  if (settings.pidFile !== undefined)
    writeFileSync(settings.pidFile, `${child.pid}\n`);
}

const serverEnv = {
  ASSEMBLE_DATA_DIR: serverData,
  ASSEMBLE_PUBLIC_ORIGIN: origin,
};

const fake = (args: string[]) => run(fakeBin, ["--token", tokenFile, ...args]);

function sleep(milliseconds: number): Promise<void> {
  return new Promise((resolve) => setTimeout(resolve, milliseconds));
}

/** 进程还在不在。发信号 0 只是问一句，不打扰它。 */
function isAlive(pid: number): boolean {
  try {
    process.kill(pid, 0);
    return true;
  } catch (error) {
    // 不属于自己的进程会答"没有权限"，那说明它也在。
    return (error as NodeJS.ErrnoException).code === "EPERM";
  }
}

/** 请它退出；已经不在了就算了。 */
function terminate(pid: number): void {
  if (isAlive(pid)) process.kill(pid, "SIGTERM");
}

async function commandLine(pid: number): Promise<string> {
  const described = await run("ps", ["-p", String(pid), "-o", "command="]);
  return described.code === 0 ? described.stdout.trim() : "";
}

/** 命令行与条件相符的进程号；没有就是空数组。 */
async function pidsMatching(pgrepArguments: string[]): Promise<number[]> {
  const found = await run("pgrep", pgrepArguments);
  return found.code === 1
    ? []
    : found.stdout
        .split("\n")
        .flatMap((line) => (/^\d+$/.test(line.trim()) ? [Number(line)] : []));
}

const desktopPids = () => pidsMatching(["-x", "assemble-desktop"]);

type StopOutcome = "stopped" | "absent" | "survived";

/** 停掉进程号文件里记着的那个进程，返回它最后的状态。 */
async function stopRecorded(
  file: string,
  mustMention: string,
): Promise<StopOutcome> {
  let pid = Number.NaN;
  try {
    pid = Number(readFileSync(file, "utf8").trim());
  } catch {
    pid = Number.NaN;
  }
  rmSync(file, { force: true });
  // 进程号会被系统重新分给别的程序：只停命令行里还带着本脚本那条路径的。
  if (!Number.isInteger(pid) || !isAlive(pid)) return "absent";
  if (!(await commandLine(pid)).includes(mustMention)) return "absent";
  terminate(pid);
  for (let waited = 0; waited < 50 && isAlive(pid); waited += 1)
    await sleep(100);
  return isAlive(pid) ? "survived" : "stopped";
}

async function healthy(): Promise<boolean> {
  try {
    const response = await fetch(`${origin}/healthz`, {
      signal: AbortSignal.timeout(1500),
    });
    return response.ok;
  } catch {
    return false;
  }
}

/** 日志的最后几行；还没有日志时如实说。 */
function tail(path: string, lines = 8): string {
  let text: string;
  try {
    text = readFileSync(path, "utf8");
  } catch {
    return "  （还没有日志）";
  }
  return text
    .trimEnd()
    .split("\n")
    .slice(-lines)
    .map((line) => `  ${line}`)
    .join("\n");
}

function size(path: string): number {
  try {
    return statSync(path).size;
  } catch {
    return 0;
  }
}

/** 日志从第 `from` 个字节起的内容；读不了就当是空的。 */
function readFrom(path: string, from: number): string {
  try {
    return readFileSync(path).subarray(from).toString("utf8");
  } catch {
    return "";
  }
}

/**
 * 每隔一秒问一次，直到 `probe` 给出结果，或者时限到了（返回 `null`）。
 * 等待期间在同一行上报已经等了多久，让人知道它没有卡死。
 */
async function waitUntil<T>(
  probe: () => Promise<T | null>,
  seconds: number,
  what: string,
): Promise<T | null> {
  const started = Date.now();
  const deadline = started + seconds * 1000;
  process.stdout.write(`  ${what}……`);
  try {
    while (Date.now() < deadline) {
      const found = await probe();
      if (found !== null) return found;
      const waited = Math.round((Date.now() - started) / 1000);
      process.stdout.write(`\r  ${what}……已等 ${waited} 秒 `);
      await sleep(1000);
    }
    return null;
  } finally {
    process.stdout.write("\n");
  }
}

/** 别的设备里在线的那一台；还没有就返回 `null`。 */
async function onlinePeer(): Promise<{ id: string; name: string } | null> {
  const waited = await fake([
    "wait",
    "--origin",
    origin,
    "--until",
    "peer-online",
    "--timeout",
    "2",
  ]);
  if (waited.code === 3) return null;
  if (waited.code !== 0) {
    throw new Error(waited.stderr.trim() || "替身设备问不到设备列表。");
  }
  const [peer] = markerLines(waited.stdout, "peer-online");
  return peer?.[0] && peer[1] ? { id: peer[0], name: peer[1] } : null;
}

/** 登记替身设备（给同一个账户登记过就沿用），再确认它连得上服务端。失败时返回一段说明。 */
async function registerFake(account: string): Promise<string> {
  const register = () =>
    fake([
      "register",
      "--data-dir",
      serverData,
      "--account",
      account,
      "--name",
      fakeName,
      "--platform",
      "windows",
    ]);
  const reach = () => fake(["devices", "--origin", origin]);

  let result = await register();
  if (result.code !== 0) return `登记替身设备失败：${result.stderr.trim()}`;
  let probe = await reach();
  if (probe.code !== 0) {
    // 令牌失效多半是这台替身设备被从账户里移除了：删掉令牌，重新登记一台。
    rmSync(tokenFile, { force: true });
    result = await register();
    if (result.code !== 0) return `登记替身设备失败：${result.stderr.trim()}`;
    probe = await reach();
    if (probe.code !== 0) return `替身设备连不上服务端：${probe.stderr.trim()}`;
  }
  return "";
}

// ---- 起环境并核对 ----

async function start(options: Options): Promise<number> {
  const checks: Check[] = [];
  mkdirSync(work, { recursive: true });

  const running = await desktopPids();
  if (running.length > 0) {
    console.error(
      `Assemble 已经在运行（进程 ${running.join("、")}）。这个命令要重新构建并启动应用：请先从菜单栏图标退出它，再运行一次。`,
    );
    return 1;
  }

  step("编译服务端与替身设备");
  const built = await inherit("cargo", [
    "build",
    "-p",
    "assemble-server",
    "--bin",
    "assemble-server",
    "--example",
    "fake_device",
  ]);
  if (built !== 0)
    return abort("服务端或替身设备没有编译出来，原因在上面的输出里。");

  step("服务端");
  if (await healthy()) {
    note(`${origin} 上已经有一个在应答的服务端，沿用它（收摊时不去停它）。`);
  } else {
    startDetached(serverBin, ["serve"], {
      logFile: log.server,
      pidFile: pidFile.server,
      env: serverEnv,
    });
    const up = await waitUntil(
      async () => ((await healthy()) ? true : null),
      30,
      "等它启动",
    );
    if (up === null) {
      return abort(
        `服务端 30 秒内没有应答。日志的最后几行：\n${tail(log.server)}`,
      );
    }
    note(`已在 ${origin} 启动，数据在 ${shown(serverData)}。`);
  }

  step("账户");
  const listed = await run(serverBin, ["list-accounts"], serverEnv);
  if (listed.code !== 0)
    return abort(`问不到账户清单：${listed.stderr.trim()}`);
  const usernames = listed.stdout
    .split("\n")
    .map((line) => line.trim())
    .filter(Boolean);
  if (!usernames.includes(options.account)) {
    // 建账户要你亲手设密码，所以只能在你自己的终端里做。
    if (!process.stdin.isTTY) {
      return abort(
        `没有叫 ${options.account} 的账户。先建一个（会让你设密码）：bun run server:account ${options.account}`,
      );
    }
    note(
      `还没有叫 ${options.account} 的账户，现在建一个：按提示设一个密码（至少 8 个字符，输入时不回显）。它只在本机的登录页上用。`,
    );
    const created = await inherit(
      serverBin,
      ["create-account", options.account],
      serverEnv,
    );
    if (created !== 0) {
      return abort(
        `账户没有建成。也可以稍后自己运行：bun run server:account ${options.account}`,
      );
    }
  }
  note(`登录用的账户：${options.account}`);

  step(`替身设备“${fakeName}”`);
  // 上一回被中断时留下的替身设备：先停掉，免得两台用同一个令牌同时在线。
  for (const pid of await pidsMatching(["-f", tokenFile])) terminate(pid);
  const registered = await registerFake(options.account);
  if (registered !== "") return abort(registered);
  startDetached(
    fakeBin,
    [
      "--token",
      tokenFile,
      "online",
      "--origin",
      origin,
      "--receive-dir",
      fakeInbox,
    ],
    { logFile: log.fake, pidFile: pidFile.fake },
  );
  const connected = await waitUntil(
    async () =>
      markerLines(readFrom(log.fake, 0), "online").length > 0 ? true : null,
    15,
    "等它上线",
  );
  if (connected === null) {
    return abort(
      `替身设备 15 秒内没有连上服务端。日志的最后几行：\n${tail(log.fake)}`,
    );
  }
  note(`已上线，收到的东西存进 ${shown(fakeInbox)}。`);

  step("构建桌面应用（发布版，连本机的服务端）");
  const packaged = await inherit(
    "bunx",
    ["tauri", "build", "--no-bundle"],
    { ASSEMBLE_SERVER_ORIGIN: origin },
    join(root, "apps/desktop"),
  );
  if (packaged !== 0)
    return abort("桌面应用没有构建出来，原因在上面的输出里。");

  step("启动桌面应用，等你登录");
  startDetached(desktopBin, [], { logFile: log.desktop });
  console.log("  窗口打开之后，请你：");
  console.log("    1. 点“使用浏览器登录”。系统浏览器会打开本机的登录页。");
  console.log(
    `    2. 用账户 ${options.account} 和你设的密码登录，然后点“确认登录”。`,
  );
  console.log(
    `    3. 回到 Assemble 窗口：账户下面写着“这台设备：…”，“设备”里“${fakeName}”是在线的。`,
  );
  console.log(
    "    （如果系统问要不要允许访问钥匙串，点“始终允许”。没签名的应用重新构建之后会再问一次，签名之后就没有了。）",
  );
  const peer = await waitUntil(onlinePeer, options.timeout, "等桌面应用上线");
  if (peer === null) {
    return abort(
      `等了 ${options.timeout} 秒，桌面应用没有以账户 ${options.account} 上线。登录卡在哪一步，把浏览器和应用窗口上的提示告诉我。`,
    );
  }
  record(
    checks,
    true,
    `桌面应用已登录并上线，它在这个账户里叫“${peer.name}”。`,
  );

  step(`收文件：“${fakeName}” → 这台电脑`);
  note(
    "替身设备发两个文件过来；应用收好并回执之后，再到默认接收文件夹里核对摘要。",
  );
  mkdirSync(outbox, { recursive: true });
  const noteBytes = new TextEncoder().encode(NOTE_TEXT);
  const blob = randomFile(BLOB_BYTES);
  const offerings = [
    {
      name: NOTE_NAME,
      path: join(outbox, NOTE_NAME),
      sha256: sha256(noteBytes),
    },
    { name: BLOB_NAME, path: join(outbox, BLOB_NAME), sha256: blob.sha256 },
  ];
  await writeFile(offerings[0].path, noteBytes);
  await writeFile(offerings[1].path, blob.bytes);
  const since = Date.now();
  for (const offering of offerings) {
    const sent = await fake([
      "send",
      "--origin",
      origin,
      "--to",
      peer.id,
      "--file",
      offering.path,
      "--wait",
      "--timeout",
      String(Math.min(options.timeout, 120)),
    ]);
    const uploaded = sent.stdout
      .split("\n")
      .find((line) => line.startsWith("已上传"))
      ?.replace(/^已上传/, "已发出");
    record(
      checks,
      sent.code === 0,
      sent.code === 0
        ? `${uploaded ?? `已发出「${offering.name}」`}，应用已回执“已收下”。`
        : `「${offering.name}」没有送达：${sent.stderr.trim() || "替身设备没有报告结局"}`,
    );
  }
  for (const offering of offerings) {
    const landed = await findLanded(
      defaultInbox,
      offering.name,
      offering.sha256,
      since,
    );
    if (landed !== null)
      record(checks, true, `落盘一致：${shown(landed.path)}`);
    else {
      note(
        `默认接收文件夹里没找到“${offering.name}”。如果你改过接收位置，请到那里看一眼它是不是在。`,
      );
    }
  }

  step(`发文件：这台电脑 → “${fakeName}”`);
  console.log(
    "  现在请你发一个文件回去：把任意一个文件拖进 Assemble 窗口（或者点窗口里的“发送文件…”挑一个），",
  );
  console.log(`  在“发送到…”面板里点“${fakeName}”。`);
  const offset = size(log.fake);
  const received = await waitUntil(
    async () => {
      const lines = markerLines(readFrom(log.fake, offset), "received");
      // 一次可以拖好几个文件：第一个到了之后再等三秒，把同一批的都收齐再报告。
      if (lines.length === 0) return null;
      await sleep(3000);
      return markerLines(readFrom(log.fake, offset), "received");
    },
    options.timeout,
    "等它收到",
  );
  if (received === null || received.length === 0) {
    record(
      checks,
      false,
      `等了 ${options.timeout} 秒，“${fakeName}”没有收到文件。`,
    );
  } else {
    for (const fields of received) {
      // 一行记录四个字段：名字、字节数、声明的摘要、落盘的路径。
      if (fields.length !== 4) {
        record(
          checks,
          false,
          `“${fakeName}”收到了文件，但它的记录看不懂：${fields.join("｜")}`,
        );
        continue;
      }
      const [name, bytes, declared, path] = fields;
      const actual = await sha256File(path).catch(() => "");
      record(
        checks,
        actual === declared,
        actual === declared
          ? `「${name}」（${formatSize(Number(bytes))}）已存进 ${shown(path)}，摘要与声明的一致。应用里这一条应当显示“已送达”。`
          : `“${fakeName}”收到了“${name}”，但落盘之后的摘要与声明的不一致：${path}`,
      );
    }
  }

  step("退出");
  console.log("  最后一步：点菜单栏上的 Assemble 图标 →“退出 Assemble”。");
  const quit = await waitUntil(
    async () => {
      if ((await desktopPids()).length > 0) return null;
      const offline = await fake([
        "wait",
        "--origin",
        origin,
        "--until",
        "peer-offline",
        "--timeout",
        "2",
      ]);
      return offline.code === 0 ? true : null;
    },
    options.timeout,
    "等应用退出",
  );
  record(
    checks,
    quit !== null,
    quit !== null
      ? "应用已退出，服务端上这台电脑随即显示离线。"
      : `等了 ${options.timeout} 秒，应用没有退出，或者退出之后服务端上它仍是在线的。`,
  );

  step("结果");
  for (const check of checks)
    console.log(`  ${check.ok ? "✓" : "✗"} ${check.text}`);
  console.log("\n脚本看不见、请你顺手看一眼的几样：");
  for (const item of BY_HAND) console.log(`  - ${item}`);
  console.log("\n哪一条不对，把现象告诉我（点了什么、看到了什么、隔了多久）。");
  console.log(
    `日志：${shown(log.desktop)}（应用）、${shown(log.fake)}（替身设备）、${shown(log.server)}（服务端）。`,
  );
  console.log("收摊：bun run acceptance stop。桌面应用请从菜单栏图标退出。");
  return checks.every((check) => check.ok) ? 0 : 1;
}

async function stop(): Promise<number> {
  const stopped = await stopRecorded(pidFile.fake, tokenFile);
  // 进程号文件可能已经不在了（上一回被中断）：再按令牌文件的路径找一遍。
  const stragglers = await pidsMatching(["-f", tokenFile]);
  for (const pid of stragglers) terminate(pid);
  const words: Record<StopOutcome, string> = {
    stopped: "已停止",
    absent: "没有在运行",
    survived: "请它退出之后 5 秒内没有退出",
  };
  const fakeWords =
    stopped === "absent" && stragglers.length > 0 ? "已停止" : words[stopped];
  console.log(`替身设备“${fakeName}”：${fakeWords}`);

  const server = await stopRecorded(pidFile.server, serverBin);
  console.log(
    `本脚本起的服务端：${words[server]}（自己用 bun run server:dev 起的不归这里管）`,
  );

  const running = await desktopPids();
  console.log(
    running.length > 0
      ? `桌面应用还在运行（进程 ${running.join("、")}）。它不归这条命令管：要退出，点菜单栏图标 →“退出 Assemble”。`
      : "桌面应用没有在运行。",
  );
  return server === "survived" ? 1 : 0;
}

process.on("SIGINT", () => {
  console.log("\n\n已中断。要收摊：bun run acceptance stop");
  process.exit(130);
});

async function main(): Promise<number> {
  if (process.platform !== "darwin") {
    console.error(
      "这条命令只在 macOS 上用。Windows 上的验收按运行手册末尾的清单走：docs/runbooks/verify-macos-launch.md",
    );
    return 1;
  }
  const argv = process.argv.slice(2);
  if (argv.some((argument) => argument === "--help" || argument === "-h")) {
    console.log(USAGE);
    return 0;
  }
  const parsed = parseArguments(argv);
  if (typeof parsed === "string") {
    console.error(parsed);
    return 1;
  }
  try {
    return parsed.command === "stop" ? await stop() : await start(parsed);
  } catch (error) {
    return abort((error as Error).message);
  }
}

process.exitCode = await main();
