import { text } from "@/lib/text-i18n";
import type { BuildInfo } from "./build-identity";

export type { BuildChannel, BuildInfo } from "./build-identity";

export function formatBuildInfo(info: BuildInfo): string {
  return text("build.identity", {
    channel: text(`build.channels.${info.channel}`),
    version: info.version,
    commit: info.commit,
  });
}

/** 关于页在渲染时调用，官网地址与版本值仍由构建元数据原样提供。 */
export function buildInfoLabels(): { version: string; website: string } {
  return {
    version: text("build.currentVersion"),
    website: text("build.website"),
  };
}
