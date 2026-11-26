// 身份页复用真实 shadcn 输出；只在构建时渲染，浏览器不加载 React 或接触表单凭据的脚本。

import { createHash } from "node:crypto";
import { mkdir } from "node:fs/promises";
import { dirname, join } from "node:path";
import { fileURLToPath } from "node:url";
import tailwind from "@tailwindcss/vite";
import {
  CircleCheck,
  GalleryVerticalEnd,
  GitFork,
  Info,
  LogIn,
  LogOut,
  Mail,
  ShieldCheck,
} from "lucide-react";
import { createElement, type ReactElement } from "react";
import { renderToStaticMarkup } from "react-dom/server";
import { build } from "vite";
import { Alert, AlertDescription } from "../src/components/alert";
import { Avatar, AvatarFallback } from "../src/components/avatar";
import { Button } from "../src/components/button";
import {
  Card,
  CardContent,
  CardDescription,
  CardHeader,
  CardTitle,
} from "../src/components/card";
import {
  Empty,
  EmptyContent,
  EmptyDescription,
  EmptyHeader,
  EmptyMedia,
  EmptyTitle,
} from "../src/components/empty";
import {
  Field,
  FieldDescription,
  FieldGroup,
  FieldLabel,
  FieldSeparator,
} from "../src/components/field";
import { Input } from "../src/components/input";
import { Separator } from "../src/components/separator";

const root = fileURLToPath(new URL("..", import.meta.url));
const destination = join(root, "../../apps/server/src/web_ui/generated");
const check = Bun.argv.includes("--check");
const samples: Record<string, ReactElement> = {
  card: createElement(Card, { className: "[--card-spacing:--spacing(6)]" }),
  header: createElement(CardHeader),
  auth_header: createElement(CardHeader, { className: "text-center" }),
  description: createElement(CardDescription),
  title: createElement(CardTitle),
  content: createElement(CardContent, { className: "flex flex-col gap-6" }),
  group: createElement(FieldGroup),
  field: createElement(Field),
  label: createElement(FieldLabel),
  input: createElement(Input, { className: "h-10" }),
  button: createElement(Button),
  submit: createElement(Button, { className: "h-10 w-full" }),
  auth_outline: createElement(Button, {
    variant: "outline",
    className: "h-10 w-full",
  }),
  field_description: createElement(FieldDescription, {
    className: "text-center",
  }),
  empty: createElement(Empty, { className: "flex-none" }),
  empty_header: createElement(EmptyHeader),
  empty_media: createElement(EmptyMedia, { variant: "icon" }),
  empty_title: createElement(EmptyTitle),
  empty_description: createElement(EmptyDescription, {
    className: "break-words",
  }),
  empty_content: createElement(EmptyContent),
  outline: createElement(Button, { variant: "outline" }),
  outline_full: createElement(Button, {
    variant: "outline",
    className: "w-full",
  }),
  destructive: createElement(Button, { variant: "destructive" }),
  alert: createElement(Alert, { variant: "destructive" }),
  status: createElement(Alert, { role: "status" }),
  alert_description: createElement(AlertDescription),
  separator: createElement(Separator),
  avatar: createElement(Avatar, { className: "size-10" }),
  avatar_fallback: createElement(Avatar, null, createElement(AvatarFallback)),
};
const attributes: Record<string, string> = {};
for (const [name, element] of Object.entries(samples)) {
  const html = renderToStaticMarkup(element);
  const match = (
    name === "avatar_fallback"
      ? /<span\s+([^>]*data-slot="avatar-fallback"[^>]*)>/
      : /^<[a-z][\w-]*\s+([\s\S]*?)(?:\/?>)/
  ).exec(html);
  if (!match?.[1] || /\s(?:on\w+|style)=/i.test(match[1]))
    throw new Error(`Server UI cannot export component ${name}`);
  attributes[name] = match[1];
}
const result = await build({
  configFile: false,
  root,
  base: "/web/ui/",
  plugins: [tailwind()],
  logLevel: "error",
  build: {
    write: false,
    assetsInlineLimit: 0,
    rolldownOptions: {
      input: join(root, "src/server.css"),
      output: { assetFileNames: "[name][extname]" },
    },
  },
});
if (Array.isArray(result) || !("output" in result))
  throw new Error("Unexpected server UI build output");
const files = new Map<string, Uint8Array>();
const markup = {
  brand: renderToStaticMarkup(
    createElement(GalleryVerticalEnd, {
      className: "size-4",
      "aria-hidden": true,
    }),
  ),
  github: renderToStaticMarkup(
    createElement(GitFork, {
      ...{ "data-icon": "inline-start" },
      "aria-hidden": true,
    }),
  ),
  info: renderToStaticMarkup(createElement(Info, { "aria-hidden": true })),
  login: renderToStaticMarkup(createElement(LogIn, { "aria-hidden": true })),
  logout: renderToStaticMarkup(createElement(LogOut, { "aria-hidden": true })),
  mail: renderToStaticMarkup(createElement(Mail, { "aria-hidden": true })),
  verified: renderToStaticMarkup(
    createElement(ShieldCheck, { "aria-hidden": true }),
  ),
  success: renderToStaticMarkup(
    createElement(CircleCheck, { "aria-hidden": true }),
  ),
  email_separator: renderToStaticMarkup(
    createElement(
      FieldSeparator,
      { className: "*:data-[slot=field-separator-content]:bg-card" },
      "或",
    ),
  ),
};
files.set(
  "markup.json",
  new TextEncoder().encode(`${JSON.stringify(markup, null, 2)}\n`),
);
files.set(
  "attributes.json",
  new TextEncoder().encode(`${JSON.stringify(attributes, null, 2)}\n`),
);
for (const asset of result.output) {
  if (
    asset.type !== "asset" ||
    !/^[\w.-]+\.(?:css|woff2)$/.test(asset.fileName)
  )
    throw new Error("Server UI build must contain only local CSS and fonts");
  files.set(
    asset.fileName,
    typeof asset.source === "string"
      ? new TextEncoder().encode(asset.source)
      : asset.source,
  );
}
const fontPackage = dirname(
  fileURLToPath(import.meta.resolve("@fontsource-variable/geist/package.json")),
);
files.set(
  "OFL.txt",
  new Uint8Array(await Bun.file(join(fontPackage, "LICENSE")).arrayBuffer()),
);
const stale: string[] = [];
for (const [name, bytes] of files) {
  const path = join(destination, name);
  if (check) {
    const old = Bun.file(path);
    if (
      !(await old.exists()) ||
      createHash("sha256")
        .update(new Uint8Array(await old.arrayBuffer()))
        .digest("hex") !== createHash("sha256").update(bytes).digest("hex")
    )
      stale.push(name);
  } else {
    await mkdir(destination, { recursive: true });
    await Bun.write(path, bytes);
  }
}
if (stale.length)
  throw new Error(
    `Server UI is stale (${stale.join(", ")}); run bun run ui:server`,
  );
console.log(
  `Server UI ${check ? "checked" : "generated"}: ${files.size} files`,
);
