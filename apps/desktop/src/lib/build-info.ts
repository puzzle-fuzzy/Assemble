/** 安装包的构建身份，与连接哪一套服务相互独立。 */
export type BuildChannel = "stable" | "preview" | "local";

export interface BuildInfo {
  channel: BuildChannel;
  version: string;
  commit: string;
}

const CHANNEL_NAMES: Record<BuildChannel, string> = {
  stable: "正式版本",
  preview: "预览版本",
  local: "本地测试",
};

export function formatBuildInfo(info: BuildInfo): string {
  return `${CHANNEL_NAMES[info.channel]} - ${info.version}.${info.commit}`;
}
