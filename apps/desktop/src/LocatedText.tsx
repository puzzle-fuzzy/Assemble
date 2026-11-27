// 仅组合 shadcn 的链接按钮；不解析 HTML、不猜测地址，长地名优先且不拆开原文 URL。
import { Button } from "@/components/ui/button";
import type { Location } from "@/lib/types";

export function locatedSegments(text: string, locations: readonly Location[]) {
  const byLabel = new Map(
    locations.filter((l) => l.label).map((l) => [l.label, l]),
  );
  if (!byLabel.size) return [{ text }];
  const pattern = [...byLabel.keys()]
    .sort((a, b) => b.length - a.length)
    .map((label) => label.replace(/[.*+?^${}()|[\]\\]/g, "\\$&"))
    .join("|");
  const labels = new RegExp(pattern, "gu");
  const segments: { text: string; location?: Location }[] = [];
  const urls = /https?:\/\/[^\s<>"，。！？；、（）【】《》〈〉「」『』]+/giu;
  let offset = 0;
  const append = (from: number, to: number) => {
    for (const match of text.slice(from, to).matchAll(labels)) {
      const index = from + match.index;
      if (index > offset) segments.push({ text: text.slice(offset, index) });
      segments.push({ text: match[0], location: byLabel.get(match[0]) });
      offset = index + match[0].length;
    }
  };
  let start = 0;
  for (let url = urls.exec(text); url; url = urls.exec(text)) {
    append(start, url.index);
    const quote = text[url.index - 1];
    if (quote === "“" || quote === "‘") {
      const end = url[0].indexOf(quote === "“" ? "”" : "’");
      if (end !== -1) urls.lastIndex = url.index + end;
    }
    start = urls.lastIndex;
  }
  append(start, text.length);
  if (offset < text.length) segments.push({ text: text.slice(offset) });
  return segments;
}

export function LocatedText({
  text,
  locations = [],
  onOpen,
}: {
  text: string;
  locations?: readonly Location[];
  onOpen: (label: string) => void;
}) {
  let offset = 0;
  return (
    <>
      {locatedSegments(text, locations).map((segment) => {
        const key = offset;
        offset += segment.text.length;
        if (!segment.location) return segment.text;
        const url = new URL("https://maps.apple.com/directions");
        url.searchParams.set("destination", segment.location.address);
        return (
          <Button
            key={key}
            variant="link"
            asChild
            className="inline h-auto p-0 text-inherit underline underline-offset-4 whitespace-normal"
          >
            <a
              href={url.href}
              onClick={(event) => {
                event.preventDefault();
                onOpen(segment.location?.label ?? "");
              }}
            >
              {segment.text}
            </a>
          </Button>
        );
      })}
    </>
  );
}
