import { registerTranslations, useTranslation } from "@assemble/display";

registerTranslations("driveTrash", {
  "zh-CN": {
    files: "文件",
    trash: "回收站",
    name: "名称",
    size: "大小",
    deletedAt: "删除时间",
    originalLocation: "原位置",
    root: "云盘",
    folder: "文件夹",
    search: "搜索回收站",
    clear: "清空搜索",
    empty: "暂无内容",
    loading: "正在加载",
    retry: "重试",
    refresh: "刷新",
    move: "移入回收站",
    moveTitle: "移入回收站？",
    restore: "恢复",
    restoreTitle: "恢复到此位置？",
    purge: "永久删除",
    purgeTitle: "永久删除这些内容？",
    cancel: "取消",
    confirm: "确认",
    purgeHint: "此操作无法撤销。",
    moveHint: "仍计入云盘空间，可在回收站恢复。",
    destination: "恢复位置",
    renamed: "恢复后的名称",
    viewLocation: "查看位置",
    contents: "内容",
    fileCount: "{{count}} 个文件",
    folderCount: "{{count}} 个文件夹",
    counts: "{{files}} · {{folders}}",
    summary: "{{files}} · {{folders}} · {{size}}",
    restored: "已恢复",
    purged: "已永久删除",
    moved: "已移入回收站",
    unavailable: "服务端尚不支持回收站，请更新服务端后再操作。",
    failed: "操作未完成，请重试。",
    missing: "内容已不存在，请刷新后重试。",
    conflict: "内容或目标位置已更改，请重新核对后确认。",
    limit: "已达到回收记录上限，已有内容仍可恢复或永久清理。",
    tooLarge: "此文件夹内容过多，请分批处理。",
    signIn: "登录已失效，请重新登录。",
    parentUnavailable: "原文件夹已不可用，将恢复到云盘根目录。",
    depthLimit: "原位置层级过深，将恢复到云盘根目录。",
    trashBytes: "回收站逻辑用量",
    reclaimableBytes: "预计可释放内容空间",
  },
  en: {
    files: "Files",
    trash: "Trash",
    name: "Name",
    size: "Size",
    deletedAt: "Deleted",
    originalLocation: "Original location",
    root: "Cloud drive",
    folder: "Folder",
    search: "Search trash",
    clear: "Clear search",
    empty: "No items",
    loading: "Loading",
    retry: "Retry",
    refresh: "Refresh",
    move: "Move to trash",
    moveTitle: "Move to trash?",
    restore: "Restore",
    restoreTitle: "Restore to this location?",
    purge: "Delete permanently",
    purgeTitle: "Permanently delete these items?",
    cancel: "Cancel",
    confirm: "Confirm",
    purgeHint: "This cannot be undone.",
    moveHint: "Still counts towards storage. Restore it from Trash.",
    destination: "Restore location",
    renamed: "Restored name",
    viewLocation: "Show location",
    contents: "Contents",
    fileCount_one: "{{count}} file",
    fileCount_other: "{{count}} files",
    folderCount_one: "{{count}} folder",
    folderCount_other: "{{count}} folders",
    counts: "{{files}} · {{folders}}",
    summary: "{{files}} · {{folders}} · {{size}}",
    restored: "Restored",
    purged: "Permanently deleted",
    moved: "Moved to trash",
    unavailable: "Update the server to use Trash.",
    failed: "The operation could not be completed. Please retry.",
    missing: "This item no longer exists. Refresh and try again.",
    conflict:
      "The content or destination changed. Review it before confirming again.",
    limit:
      "The trash record limit has been reached. Existing items can still be restored or permanently deleted.",
    tooLarge: "This folder contains too many items. Process smaller groups.",
    signIn: "Your session expired. Please sign in again.",
    parentUnavailable:
      "The original folder is unavailable. This will restore to the cloud drive root.",
    depthLimit:
      "The original location is too deeply nested. This will restore to the cloud drive root.",
    trashBytes: "Trash logical usage",
    reclaimableBytes: "Estimated reclaimable content",
  },
});
export const useTrashTranslation = () => useTranslation("driveTrash");
export function trashError(error: unknown): string {
  const code =
    typeof error === "object" && error !== null && "code" in error
      ? error.code
      : "";
  if (code === "NOT_FOUND") return "missing";
  if (code === "UNAUTHORIZED") return "signIn";
  if (code === "DRIVE_TRASH_UPGRADE_REQUIRED") return "unavailable";
  if (code === "DRIVE_TRASH_TOO_LARGE") return "tooLarge";
  if (code === "DRIVE_TRASH_LIMIT") return "limit";
  if (
    [
      "VERSION_CONFLICT",
      "DRIVE_TRASH_CHANGED",
      "DRIVE_TRASH_FINISHED",
      "DRIVE_REQUEST_CONFLICT",
    ].includes(String(code))
  )
    return "conflict";
  return "failed";
}
