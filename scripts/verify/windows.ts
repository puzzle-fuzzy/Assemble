// Mac 只交叉检查纯 Rust 规则；远控的 H.264/Opus 需要 MSVC 本机工具链，留给 Windows 完整检查。
import { fileURLToPath } from "node:url";

const command = [
  "cargo",
  "clippy",
  ...["protocol", "transfer", "store", "device", "schedule", "remote"].flatMap(
    (name) => ["-p", `assemble-${name}`],
  ),
];
if (process.platform === "win32") {
  command.push("--features", "assemble-remote/native");
} else {
  console.log(
    "Windows 目标：检查六个基础库的规则；含 C/C++ 的远控原生适配由 Windows CI 检查。",
  );
}
command.push(
  "--target",
  "x86_64-pc-windows-msvc",
  "--all-targets",
  "--",
  "-D",
  "warnings",
);
const child = Bun.spawn(command, {
  cwd: fileURLToPath(new URL("../..", import.meta.url)),
  stdin: "ignore",
  stdout: "inherit",
  stderr: "inherit",
});
process.exit(await child.exited);
