// 一键验证：lint → 类型检查 → 测试 → 前端构建 → Rust 格式、静态检查与测试。
//
// 任一步失败立即停止，并把它的退出码返回给调用方。
// 所有命令都从仓库根目录执行；根目录由本文件的位置推出，不依赖当前工作目录。
//
//   bun run verify
//
// 打包安装包不在这里：那是发布时的事，用 `bun run build`。

import { fileURLToPath } from "node:url";

const root = fileURLToPath(new URL("../..", import.meta.url));

interface Step {
  name: string;
  command: string[];
  /** 只在条件满足时才跑；不满足时如实说明并跳过，不算失败。 */
  onlyWhen?: () => Promise<boolean>;
  skipped?: string;
}

/**
 * 装了 Windows 的 Rust 目标时，顺便给能在本机上检查的 crate 做一次针对 Windows 的静态检查：
 * 不依赖窗口，也不需要交叉编译 C 代码的六个基础库规则。远控原生适配（H.264/Opus）、客户端
 * （TLS）与服务端（SQLite）带本机依赖，从 macOS 上检查不了 Windows 版本，必须到 Windows 环境验。
 */
async function windowsTargetInstalled(): Promise<boolean> {
  const listed = Bun.spawn(["rustup", "target", "list", "--installed"], {
    cwd: root,
    stdout: "pipe",
    stderr: "ignore",
    stdin: "ignore",
  });
  const output = await new Response(listed.stdout).text();
  await listed.exited;
  return output.split("\n").includes("x86_64-pc-windows-msvc");
}

const STEPS: Step[] = [
  { name: "Biome：lint 与格式", command: ["bun", "run", "lint"] },
  {
    name: "服务端页面共享组件产物",
    command: ["bun", "run", "ui:server:check"],
  },
  { name: "类型检查：界面与工具脚本", command: ["bun", "run", "typecheck"] },
  {
    name: "界面与工具脚本的测试",
    command: ["bun", "run", "test"],
  },
  {
    name: "前端构建",
    command: ["bun", "run", "build:frontend"],
  },
  { name: "Rust 格式", command: ["bun", "run", "rust:fmt"] },
  { name: "Rust 静态检查", command: ["bun", "run", "rust:clippy"] },
  {
    name: "Rust 静态检查：Windows 目标（能在本机检查的 crate）",
    command: ["bun", "run", "rust:clippy:windows"],
    onlyWhen: windowsTargetInstalled,
    skipped:
      "没有安装 Windows 的 Rust 目标，跳过。装上它就能在 macOS 上给 Windows 专用的代码做类型检查：rustup target add x86_64-pc-windows-msvc",
  },
  { name: "Rust 测试", command: ["bun", "run", "rust:test"] },
];

async function run(step: Step): Promise<number> {
  console.log(`\n▶ ${step.name}`);
  console.log(`  ${step.command.join(" ")}`);
  const child = Bun.spawn(step.command, {
    cwd: root,
    stdin: "ignore",
    stdout: "inherit",
    stderr: "inherit",
  });
  return await child.exited;
}

const started = Date.now();
for (const step of STEPS) {
  if (step.onlyWhen && !(await step.onlyWhen())) {
    console.log(`\n▶ ${step.name}`);
    console.log(`  — ${step.skipped}`);
    console.log(`— ${step.name}（跳过）`);
    continue;
  }
  const code = await run(step);
  if (code !== 0) {
    console.error(
      `\n✗ ${step.name} 失败（退出码 ${code}），后面的步骤没有执行。`,
    );
    process.exit(code);
  }
  console.log(`✓ ${step.name}`);
}
console.log(
  `\n全部通过，用时 ${((Date.now() - started) / 1000).toFixed(1)} 秒。`,
);
