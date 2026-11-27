// 备忘录独立响应契约；正文只作为纯文本使用，不解析 HTML。
export interface MemoContent {
  title: string;
  text: string;
}

export function validMemoContent(content: MemoContent): boolean {
  const control = (character: string) => {
    const code = character.codePointAt(0) ?? 0;
    return code < 32 || (code >= 127 && code <= 159);
  };
  return Boolean(
    content.title.trim() &&
      content.title.trim().length <= 200 &&
      ![...content.title].some(control) &&
      content.text.trim() &&
      content.text.length <= 100_000 &&
      ![...content.text].some(
        (character) =>
          control(character) && !["\r", "\n", "\t"].includes(character),
      ),
  );
}
export interface MemoSummary {
  id: string;
  version: number;
  title: string;
  preview: string;
  createdAt: string;
  updatedAt: string;
}
export interface Memo extends MemoContent {
  locations?: import("./types").Location[];
  id: string;
  version: number;
  createdAt: string;
  updatedAt: string;
}
export interface MemoQuery {
  q: string;
  page: number;
  pageSize: number;
}
export interface MemoList {
  items: MemoSummary[];
  total: number;
  page: number;
  pageSize: number;
}
export interface MemoBridge {
  openLocation?(id: string, label: string): Promise<void>;
  list(query: MemoQuery, signal?: AbortSignal): Promise<MemoList>;
  get(id: string, signal?: AbortSignal): Promise<Memo>;
  create(
    content: MemoContent,
    requestId: string,
    signal?: AbortSignal,
  ): Promise<Memo>;
  update(
    id: string,
    version: number,
    content: MemoContent,
    signal?: AbortSignal,
  ): Promise<Memo>;
  remove(id: string, version: number, signal?: AbortSignal): Promise<void>;
  copy(id: string, signal?: AbortSignal): Promise<void>;
}
const object = (value: unknown): value is Record<string, unknown> =>
  typeof value === "object" && value !== null;
const whole = (value: unknown): value is number =>
  typeof value === "number" && Number.isSafeInteger(value) && value >= 0;
const date = (value: unknown) =>
  typeof value === "string" && Number.isFinite(Date.parse(value));
function identity(value: unknown): value is Record<string, unknown> {
  return (
    object(value) &&
    typeof value.id === "string" &&
    value.id.length > 0 &&
    whole(value.version) &&
    value.version > 0 &&
    typeof value.title === "string" &&
    date(value.createdAt) &&
    date(value.updatedAt)
  );
}
export function isMemo(value: unknown): value is Memo {
  return (
    identity(value) &&
    typeof value.text === "string" &&
    (value.locations === undefined ||
      (Array.isArray(value.locations) &&
        value.locations.every(
          (location) =>
            object(location) &&
            typeof location.label === "string" &&
            typeof location.address === "string",
        )))
  );
}
export function isMemoList(value: unknown): value is MemoList {
  return (
    object(value) &&
    Array.isArray(value.items) &&
    value.items.every(
      (item) => identity(item) && typeof item.preview === "string",
    ) &&
    whole(value.total) &&
    whole(value.page) &&
    value.page > 0 &&
    whole(value.pageSize) &&
    value.pageSize > 0 &&
    value.pageSize <= 100
  );
}
