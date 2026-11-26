"use client";

import { registerTranslations, useTranslation } from "@assemble/display";
import { cn } from "cn";
import { BellIcon } from "lucide-react";
import { useRef, useState } from "react";
import type {
  SystemNotification,
  SystemNotificationRead,
} from "../lib/system-notifications";
import en from "../locales/system.en.json";
import zh from "../locales/system.zh-CN.json";
import { Button } from "./button";
import {
  Dialog,
  DialogContent,
  DialogDescription,
  DialogHeader,
  DialogTitle,
  DialogTrigger,
} from "./dialog";

registerTranslations("systemNotifications", { "zh-CN": zh, en });

function bytes(value: number, language: string) {
  const units = ["B", "KiB", "MiB", "GiB", "TiB", "PiB", "EiB"];
  const unit = Math.min(
    units.length - 1,
    Math.max(0, Math.floor(Math.log(Math.max(1, value)) / Math.log(1024))),
  );
  return `${new Intl.NumberFormat(language, { maximumFractionDigits: unit ? 1 : 0 }).format(value / 1024 ** unit)} ${units[unit]}`;
}

export function SystemNotificationCenter({
  items,
  loading = false,
  error = false,
  pending = false,
  readError = false,
  onRead,
  onNavigate,
  canNavigate,
  timeZone,
}: {
  items: SystemNotification[];
  loading?: boolean;
  error?: boolean;
  pending?: boolean;
  readError?: boolean;
  onRead: (items: SystemNotificationRead[]) => Promise<void> | void;
  onNavigate: (item: SystemNotification) => void;
  canNavigate?: (item: SystemNotification) => boolean;
  timeZone?: string;
}) {
  const { t, i18n } = useTranslation("systemNotifications");
  const language = i18n.resolvedLanguage || i18n.language;
  const [open, setOpen] = useState(false);
  const navigation = useRef(false);
  const unread = items.filter((item) => !item.readAt);
  const selection = (selected: SystemNotification[]) =>
    selected.map((item) => ({ id: item.id, revision: item.revision }));
  function text(item: SystemNotification) {
    const content = item.content;
    switch (content.type) {
      case "storageBudget":
        return {
          title: t(item.level === "error" ? "storageFull" : "storageBudget"),
          body: t("storageBudgetBody", {
            used: bytes(content.usedBytes, language),
            budget: bytes(content.budgetBytes, language),
            percent: content.warningPercent,
          }),
          action: t("storageAction"),
        };
      case "storageDisk":
        return {
          title: t("storageDisk"),
          body: t("storageDiskBody", {
            available: bytes(content.availableBytes, language),
          }),
          action: t("storageAction"),
        };
      case "storageUnavailable":
        return {
          title: t("storageUnavailable"),
          body: "",
          action: t("storageAction"),
        };
      case "transferFailed":
        return {
          title: t(content.localDiskFull ? "localDiskFull" : "transferFailed"),
          body: content.name,
          action: t("transferAction"),
        };
      case "downloadFailed":
        return {
          title: t(content.localDiskFull ? "localDiskFull" : "downloadFailed"),
          body: content.name,
          action: t("downloadAction"),
        };
      case "sessionExpired":
        return {
          title: t("sessionExpired"),
          body: t("sessionExpiredBody"),
          action: t(item.resolvedAt ? "accountViewAction" : "accountAction"),
        };
    }
  }
  return (
    <Dialog open={open} onOpenChange={setOpen}>
      <DialogTrigger asChild>
        <Button
          variant="ghost"
          size="icon"
          className="relative"
          aria-label={t(unread.length ? "unreadLabel" : "title")}
        >
          <BellIcon aria-hidden="true" />
          {unread.length > 0 && (
            <span
              aria-hidden="true"
              className="absolute top-1 right-1 size-1.5 rounded-full bg-primary"
            />
          )}
        </Button>
      </DialogTrigger>
      <DialogContent
        className="flex max-h-[min(40rem,calc(100dvh-2rem))] flex-col gap-0 overflow-hidden p-0 sm:max-w-lg"
        onCloseAutoFocus={(event) => {
          if (navigation.current) {
            event.preventDefault();
            navigation.current = false;
          }
        }}
      >
        <DialogHeader className="flex-row items-center justify-between gap-4 p-4 pr-12">
          <DialogTitle>{t("title")}</DialogTitle>
          <DialogDescription className="sr-only">
            {t("description")}
          </DialogDescription>
          {unread.length > 0 && (
            <Button
              variant="ghost"
              size="default"
              disabled={pending}
              onClick={() => void onRead(selection(unread))}
            >
              {t("readAll")}
            </Button>
          )}
        </DialogHeader>
        <div className="min-h-0 flex-1 overflow-y-auto overscroll-contain px-4 pb-4">
          {error && (
            <p role="status" className="py-3 text-sm text-destructive">
              {t("error")}
            </p>
          )}
          {readError && (
            <p role="status" className="py-3 text-sm text-destructive">
              {t("readError")}
            </p>
          )}
          {loading && items.length === 0 ? (
            <p role="status" className="py-6 text-sm text-muted-foreground">
              {t("loading")}
            </p>
          ) : items.length === 0 && !error ? (
            <p className="py-6 text-sm text-muted-foreground">{t("empty")}</p>
          ) : (
            <ul className="flex flex-col divide-y">
              {items.map((item) => {
                const message = text(item);
                return (
                  <li key={item.id} className="flex flex-col gap-2 py-4">
                    <div className="flex items-start gap-2">
                      {!item.readAt && (
                        <span
                          aria-hidden="true"
                          className="mt-1.5 size-1.5 shrink-0 rounded-full bg-primary"
                        />
                      )}
                      <span
                        className={cn(
                          "text-sm",
                          !item.readAt && "font-medium",
                          !item.resolvedAt &&
                            (item.level === "error"
                              ? "text-destructive"
                              : "text-warning"),
                        )}
                      >
                        {message.title}
                      </span>
                    </div>
                    {message.body &&
                      (!item.resolvedAt ||
                        item.content.type === "downloadFailed" ||
                        item.content.type === "transferFailed") && (
                        <p className="break-words text-sm text-muted-foreground">
                          {message.body}
                        </p>
                      )}
                    <div className="flex flex-wrap items-center gap-2 text-xs text-muted-foreground">
                      {item.resolvedAt && (
                        <span>
                          {t(
                            item.content.type === "transferFailed" ||
                              item.content.type === "downloadFailed"
                              ? "handled"
                              : "resolved",
                          )}
                        </span>
                      )}
                      <time dateTime={item.updatedAt}>
                        {new Intl.DateTimeFormat(language, {
                          dateStyle: "short",
                          timeStyle: "short",
                          ...(timeZone ? { timeZone } : {}),
                        }).format(new Date(item.updatedAt))}
                      </time>
                    </div>
                    <div className="flex flex-wrap items-center justify-end gap-2">
                      {(!canNavigate || canNavigate(item)) && (
                        <Button
                          variant="ghost"
                          size="default"
                          onClick={() => {
                            if (!item.readAt) void onRead(selection([item]));
                            navigation.current = true;
                            setOpen(false);
                            onNavigate(item);
                          }}
                        >
                          {message.action}
                        </Button>
                      )}
                      {!item.readAt && (
                        <Button
                          variant="ghost"
                          size="default"
                          disabled={pending}
                          onClick={() => void onRead(selection([item]))}
                        >
                          {t("read")}
                        </Button>
                      )}
                    </div>
                  </li>
                );
              })}
            </ul>
          )}
        </div>
      </DialogContent>
    </Dialog>
  );
}
