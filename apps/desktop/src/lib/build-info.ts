/** 安装包的构建身份，以及本次构建对应的官网地址。 */
export type BuildChannel = "stable" | "preview" | "local";

export interface BuildInfo {
  channel: BuildChannel;
  version: string;
  commit: string;
  website: string;
}

const CHANNEL_NAMES: Record<BuildChannel, string> = {
  stable: "正式版本",
  preview: "预览版本",
  local: "本地测试",
};

export function formatBuildInfo(info: BuildInfo): string {
  return `${CHANNEL_NAMES[info.channel]} - ${info.version}.${info.commit}`;
}
