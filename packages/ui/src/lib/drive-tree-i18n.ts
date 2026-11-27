import { registerTranslations, useTranslation } from "@assemble/display";

registerTranslations("driveTree", {
  "zh-CN": {
    root: "云盘",
    newFolder: "新建文件夹",
    folderName: "文件夹名称",
    name: "名称",
    size: "大小",
    createdAt: "创建时间",
    folder: "文件夹",
    upload: "上传",
    refresh: "刷新",
    search: "搜索当前文件夹",
    clear: "清空搜索",
    rename: "重命名",
    move: "移动",
    moveHere: "移动到此处",
    remove: "删除",
    confirmDelete: "确认删除",
    deleteFile: "永久删除此文件？",
    deleteFolder: "删除空文件夹？",
    deleteFileHint: "此操作无法撤销。",
    deleteFolderHint: "仅空文件夹可以删除。",
    cancel: "取消",
    save: "保存",
    create: "创建",
    empty: "暂无内容",
    loading: "正在加载",
    retry: "重试",
    download: "下载",
    preview: "预览",
    destination: "选择目标文件夹",
    loadFailed: "暂时无法读取文件夹，请重试。",
    invalid: "名称应为 1–255 个字符，不能包含路径分隔符或控制字符。",
    failed: "操作未完成，请重试。",
    missing: "文件或文件夹已不存在，请刷新后重试。",
    conflict: "内容已在别处更改，请刷新并核对后重试。",
    duplicate: "此处已有同名文件夹，请更换名称。",
    cycle: "不能移动到自身或子文件夹中。",
    depth: "文件夹最多支持 16 层，请选择更上层的位置。",
    limit: "已达到文件夹或创建请求上限。",
    notEmpty: "文件夹内仍有内容，请先移出内容。",
    signIn: "登录已失效，请重新登录。",
    owner: "按账户浏览",
    allFiles: "全部文件",
    browse: "浏览文件夹",
    updated: "操作已完成",
  },
  en: {
    root: "Cloud drive",
    newFolder: "New folder",
    folderName: "Folder name",
    name: "Name",
    size: "Size",
    createdAt: "Created",
    folder: "Folder",
    upload: "Upload",
    refresh: "Refresh",
    search: "Search this folder",
    clear: "Clear search",
    rename: "Rename",
    move: "Move",
    moveHere: "Move here",
    remove: "Delete",
    confirmDelete: "Confirm deletion",
    deleteFile: "Permanently delete this file?",
    deleteFolder: "Delete this empty folder?",
    deleteFileHint: "This cannot be undone.",
    deleteFolderHint: "Only empty folders can be deleted.",
    cancel: "Cancel",
    save: "Save",
    create: "Create",
    empty: "No items",
    loading: "Loading",
    retry: "Retry",
    download: "Download",
    preview: "Preview",
    destination: "Choose destination folder",
    loadFailed: "Unable to load this folder. Please retry.",
    invalid:
      "Use 1–255 characters without path separators or control characters.",
    failed: "The operation could not be completed. Please retry.",
    missing: "This file or folder no longer exists. Refresh and try again.",
    conflict:
      "This item changed elsewhere. Refresh and review it before trying again.",
    duplicate:
      "A folder with this name already exists here. Choose another name.",
    cycle: "A folder cannot be moved into itself or its descendants.",
    depth: "Folders can be up to 16 levels deep. Choose a higher location.",
    limit: "The folder or creation request limit has been reached.",
    notEmpty: "This folder still contains items. Move them out first.",
    signIn: "Your session expired. Please sign in again.",
    owner: "Browse by account",
    allFiles: "All files",
    browse: "Browse folders",
    updated: "Operation completed",
  },
});
export const useDriveTreeTranslation = () => useTranslation("driveTree");
export function driveTreeError(error: unknown): string {
  const code =
    typeof error === "object" && error !== null && "code" in error
      ? error.code
      : "";
  switch (code) {
    case "INVALID_REQUEST":
      return "invalid";
    case "NOT_FOUND":
      return "missing";
    case "VERSION_CONFLICT":
    case "DRIVE_REQUEST_CONFLICT":
      return "conflict";
    case "DRIVE_NAME_CONFLICT":
      return "duplicate";
    case "DRIVE_CYCLE":
      return "cycle";
    case "DRIVE_DEPTH_LIMIT":
      return "depth";
    case "DRIVE_FOLDER_LIMIT":
    case "DRIVE_REQUEST_LIMIT":
      return "limit";
    case "DRIVE_FOLDER_NOT_EMPTY":
      return "notEmpty";
    case "UNAUTHORIZED":
      return "signIn";
    default:
      return "failed";
  }
}
