// 在模拟器里把 iOS 应用真的跑一遍：起一个一次性的本机服务端，建一个测试账户，
// 用 Xcode 的界面测试走完"扫码配对登录 → 名单 → 打开到点提醒 → 系统通知 → 点开定位"，
// 最后收摊。只在装了 Xcode 的 Mac 上能用：
//
//   bun run ios:simulate                 模拟器默认用 iPhone 17 Pro
//   ASSEMBLE_IOS_SIMULATOR="iPhone Air" bun run ios:simulate
//
// 它不打开模拟器的窗口，不碰这台 Mac 的系统钥匙串（模拟器里的钥匙串是它自己的），
// 也不动"应用程序"里装着的桌面端。测试账户只存在于一次性的数据目录里，跑完就删；
// 模拟器里的应用每回开始前先卸掉，上一回留下的登录和通知的权限不带到这一回。
// 界面测试在几个关键处各留一张截图，跑完从测试报告里导出来。
//
// 它驱动的是模拟器里的界面，不是人的手：用不着用户在场，也不会动用户的鼠标键盘。
// 真机上摄像头扫码、通知的横幅是什么样子，仍然只有真机回答得了。

import { Database } from "bun:sqlite";
import { spawn, spawnSync } from "node:child_process";
import { randomBytes } from "node:crypto";
import {
  chmodSync,
  closeSync,
  copyFileSync,
  mkdirSync,
  mkdtempSync,
  openSync,
  readdirSync,
  readFileSync,
  rmSync,
  writeFileSync,
} from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { fileURLToPath } from "node:url";

const root = fileURLToPath(new URL("../..", import.meta.url));
const ios = join(root, "apps/ios");
const artifacts = join(root, ".artifacts/ios");
const simulator = process.env.ASSEMBLE_IOS_SIMULATOR ?? "iPhone 17 Pro";
const account = "iostest";
const email = "iostest@example.test";

function run(
  command: string,
  args: string[],
  options: { cwd?: string; env?: Record<string, string> } = {},
) {
  const result = spawnSync(command, args, {
    cwd: options.cwd ?? root,
    env: { ...process.env, ...options.env },
    stdio: "inherit",
  });
  if (result.status !== 0)
    throw new Error(
      `${command} ${args.join(" ")} exited with ${result.status}`,
    );
}

/** 要用的那一台模拟器。名字对得上的里面，已经开着的优先，其次是系统版本新的。 */
function findSimulator(name: string) {
  const listing = spawnSync("xcrun", ["simctl", "list", "devices", "-j"], {
    encoding: "utf8",
  });
  const parsed = JSON.parse(listing.stdout ?? '{"devices":{}}') as {
    devices: Record<
      string,
      { name: string; state: string; udid: string; isAvailable?: boolean }[]
    >;
  };
  const versions = new Map<string, number[]>();
  const candidates = Object.entries(parsed.devices).flatMap(
    ([runtime, devices]) =>
      devices
        .filter(
          (device) => device.isAvailable !== false && device.name === name,
        )
        .map((device) => {
          versions.set(device.udid, runtime.match(/\d+/g)?.map(Number) ?? []);
          return device;
        }),
  );
  return candidates.sort((a, b) => {
    if (a.state !== b.state) return a.state === "Booted" ? -1 : 1;
    const left = versions.get(a.udid) ?? [];
    const right = versions.get(b.udid) ?? [];
    for (let i = 0; i < Math.max(left.length, right.length); i += 1) {
      if ((left[i] ?? 0) !== (right[i] ?? 0))
        return (right[i] ?? 0) - (left[i] ?? 0);
    }
    return 0;
  })[0];
}

/** 跑一条命令，输出写进一个文件；失败时把报错的几行打出来，全文留在文件里。 */
function runLogged(
  command: string,
  args: string[],
  logPath: string,
  options: { cwd?: string } = {},
) {
  const output = openSync(logPath, "w", 0o600);
  const result = spawnSync(command, args, {
    cwd: options.cwd ?? root,
    env: process.env,
    stdio: ["ignore", output, output],
  });
  closeSync(output);
  if (result.status !== 0) {
    const lines = readFileSync(logPath, "utf8").split("\n");
    for (const line of lines
      .filter((line) => line.includes("error:"))
      .slice(0, 20))
      console.error(line);
    throw new Error(
      `${command} ${args[0]} exited with ${result.status}; the full output is in ${logPath}`,
    );
  }
}

/** 找一个没人占着的回环端口。 */
function freePort(from: number): number {
  for (let port = from; port < from + 50; port += 1) {
    try {
      const server = Bun.listen({
        port,
        hostname: "127.0.0.1",
        socket: { data() {}, error() {} },
      });
      server.stop(true);
      return port;
    } catch {
      // 这个端口有人用，看下一个。
    }
  }
  throw new Error("no free loopback port found");
}

mkdirSync(artifacts, { recursive: true });
const stamp = new Date().toISOString().replace(/[:.]/g, "-");
const report = join(artifacts, stamp);
mkdirSync(report, { recursive: true });

const device = findSimulator(simulator);
if (!device) throw new Error(`no available simulator is named "${simulator}"`);
// 关着的模拟器不开窗口直接启动；之后一直留着，和 xcodebuild 自己的做法一样。
if (device.state !== "Booted") run("xcrun", ["simctl", "boot", device.udid]);
// 每次都从一台"新手机"开始。卸掉应用清的是通知的权限和本机的账本；模拟器的钥匙串不随应用卸掉，
// 所以下面还换一个端口：存在钥匙串里的登录按服务地址区分，换了地址就读不到上一回的那一条。
// `ASSEMBLE_IOS_KEEP_APP=1` 不卸：走的是"更新之后通知的权限还在、不用再问"的那条路。
if (process.env.ASSEMBLE_IOS_KEEP_APP !== "1") {
  run("xcrun", [
    "simctl",
    "uninstall",
    device.udid,
    "com.puzzlefuzzy.assemble.ios",
  ]);
}

run("cargo", ["build", "-p", "assemble-server"]);
run("xcodegen", ["generate"], { cwd: ios });

// 先把应用和测试构建好，再去起服务端：构建失败时还没建测试账户，也没有东西要收摊。
// 构建的输出写进文件，失败时只把报错的几行打出来。
const destination = `platform=iOS Simulator,id=${device.udid}`;
runLogged(
  "xcodebuild",
  [
    "build-for-testing",
    "-project",
    "Assemble.xcodeproj",
    "-scheme",
    "Assemble",
    "-derivedDataPath",
    ".build/DerivedData",
    "-destination",
    destination,
  ],
  join(report, "build.log"),
  { cwd: ios },
);

const data = mkdtempSync(join(tmpdir(), "assemble-ios-"));
// 端口从一个随机的位置往后找（原因见上面卸应用的那一段）。
const port = freePort(8792 + Math.floor(Math.random() * 400));
const origin = `http://127.0.0.1:${port}`;
const password = randomBytes(18).toString("hex");
const server = join(root, "target/debug/assemble-server");
const environment = {
  ASSEMBLE_DATA_DIR: data,
  ASSEMBLE_PUBLIC_ORIGIN: origin,
  ASSEMBLE_ADDR: `127.0.0.1:${port}`,
};
let serverProcess: ReturnType<typeof spawn> | undefined;
let serverLog: number | undefined;
// 测试配置里有测试账户的密码：无论中途出了什么事都要删掉。
let configured: string | undefined;

try {
  // 建一个测试账户（密码经标准输入，不进命令行），再把它的邮箱标成已验证：
  // 配对和桌面授权一样只对验证过的账户开放，而这里不发真实的验证邮件。
  const created = spawnSync(server, ["create-account", account], {
    cwd: root,
    env: { ...process.env, ...environment },
    input: `${password}\n`,
    encoding: "utf8",
  });
  if (created.status !== 0)
    throw new Error(
      `create-account exited with ${created.status}: ${created.stderr}`,
    );
  const database = new Database(join(data, "assemble.sqlite3"));
  database
    .query(
      "UPDATE accounts SET email = ?, email_verified_at = 1 WHERE username = ?",
    )
    .run(email, account);
  database.close();

  serverLog = openSync(join(report, "server.log"), "w", 0o600);
  serverProcess = spawn(server, ["serve"], {
    cwd: root,
    env: { ...process.env, ...environment },
    stdio: ["ignore", serverLog, serverLog],
  });
  let healthy = false;
  for (let attempt = 0; attempt < 60 && !healthy; attempt += 1) {
    await Bun.sleep(500);
    try {
      healthy = (await fetch(`${origin}/healthz`)).ok;
    } catch {
      // 还没起来。
    }
  }
  if (!healthy)
    throw new Error("the local server did not answer its health check");

  const xcodebuild = openSync(join(report, "xcodebuild.log"), "w", 0o600);
  const products = join(ios, ".build/DerivedData/Build/Products");
  // 测试要的几样东西写进测试配置。xcodebuild 在命令行上收下 `TEST_RUNNER_` 开头的变量，
  // 却没有把它们交给测试进程（2026-11-26 试过）；测试配置文件里的环境变量是它认的。
  const generated = readdirSync(products).find((name) =>
    name.endsWith(".xctestrun"),
  );
  if (!generated)
    throw new Error("build-for-testing produced no .xctestrun file");
  // 这一份里有测试账户的密码：只给本人读，跑完就删。配置里的路径是相对它自己所在目录的，
  // 所以改过的这一份要留在原处、只换个名字。
  configured = join(products, "assemble-configured.xctestrun");
  copyFileSync(join(products, generated), configured);
  chmodSync(configured, 0o600);
  const settings = {
    ASSEMBLE_ORIGIN: origin,
    ASSEMBLE_EMAIL: email,
    ASSEMBLE_PASSWORD: password,
  };
  for (const [key, value] of Object.entries(settings)) {
    run("plutil", [
      "-replace",
      `AssembleUITests.EnvironmentVariables.${key}`,
      "-string",
      value,
      configured,
    ]);
  }
  // 应用自己的日志收进报告：卡住的时候有证据可查。应用不记令牌，所以这一份里没有。
  // 默认的级别不含 info，而应用记的多数是 info。
  const appLog = openSync(join(report, "app.log"), "w", 0o600);
  const streaming = spawn(
    "xcrun",
    [
      "simctl",
      "spawn",
      device.udid,
      "log",
      "stream",
      "--style",
      "compact",
      "--level",
      "info",
      "--predicate",
      'subsystem == "com.puzzlefuzzy.assemble.ios"',
    ],
    { stdio: ["ignore", appLog, "ignore"] },
  );
  const test = spawnSync(
    "xcodebuild",
    [
      "test-without-building",
      "-xctestrun",
      configured,
      "-destination",
      destination,
      "-resultBundlePath",
      join(report, "result.xcresult"),
      "-parallel-testing-enabled",
      "NO",
      // 只跑其中一部分：`ASSEMBLE_IOS_ONLY=AssembleUITests`。不设就是单元测试和界面测试都跑。
      ...(process.env.ASSEMBLE_IOS_ONLY
        ? ["-only-testing", process.env.ASSEMBLE_IOS_ONLY]
        : []),
    ],
    { cwd: ios, stdio: ["ignore", xcodebuild, xcodebuild] },
  );
  closeSync(xcodebuild);
  streaming.kill("SIGTERM");
  closeSync(appLog);
  // 测试报告里留下的截图导出来，看的人不必打开 Xcode。
  spawnSync(
    "xcrun",
    [
      "xcresulttool",
      "export",
      "attachments",
      "--path",
      join(report, "result.xcresult"),
      "--output-path",
      join(report, "screenshots"),
    ],
    { stdio: "ignore" },
  );
  // 密码会出现在 xcodebuild 的命令行回显里：日志只给本人看，并且把那一行里的密码抹掉。
  const log = Bun.file(join(report, "xcodebuild.log"));
  writeFileSync(
    join(report, "xcodebuild.log"),
    (await log.text()).replaceAll(password, "<test password>"),
  );
  if (test.status !== 0)
    throw new Error(`xcodebuild test exited with ${test.status}`);
  console.log(`模拟器里走通了。截图和日志在 ${report}`);
} finally {
  if (serverProcess) {
    serverProcess.kill("SIGTERM");
    await new Promise<void>((resolve) => {
      serverProcess?.once("exit", () => resolve());
      setTimeout(() => {
        serverProcess?.kill("SIGKILL");
        resolve();
      }, 15_000);
    });
  }
  if (serverLog !== undefined) closeSync(serverLog);
  if (configured) rmSync(configured, { force: true });
  rmSync(data, { recursive: true, force: true });
}
