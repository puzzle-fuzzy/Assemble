// 构建脚本只依赖这些纯类型，不加载界面的语言和浏览器偏好模块。
export type BuildChannel = "stable" | "preview" | "local";

export interface BuildInfo {
  channel: BuildChannel;
  version: string;
  commit: string;
  website: string;
}
