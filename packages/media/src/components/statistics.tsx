import { Button } from "@assemble/ui/button";
import {
  Card,
  CardContent,
  CardDescription,
  CardHeader,
  CardTitle,
} from "@assemble/ui/card";
import { RefreshCwIcon } from "lucide-react";
import { useEffect, useState } from "react";
import type { MediaStatisticsLoader } from "../client";
import { type MediaAccessFailure, useMediaTask } from "../hooks/use-media-task";
import { useMediaTranslation } from "../i18n";
import type { MediaStatistics } from "../model";
import { MediaEmpty, MediaError, MediaLoading } from "./feedback";

export function MediaStatisticsView({
  load,
  accountKey,
  onAccessFailure,
}: {
  load: MediaStatisticsLoader;
  accountKey: string;
  onAccessFailure?: (failure: MediaAccessFailure) => void;
}) {
  return (
    <StatisticsContent
      key={accountKey}
      load={load}
      onAccessFailure={onAccessFailure}
    />
  );
}
function StatisticsContent({
  load,
  onAccessFailure,
}: {
  load: MediaStatisticsLoader;
  onAccessFailure?: (failure: MediaAccessFailure) => void;
}) {
  const { t } = useMediaTranslation();
  const [data, setData] = useState<MediaStatistics | null>(null);
  const task = useMediaTask(onAccessFailure);
  const refresh = () => {
    setData(null);
    void task.run((signal) => load(signal), setData);
  };
  useEffect(() => {
    void task.run((signal) => load(signal), setData);
  }, [load, task.run]);
  const groups = data
    ? [
        {
          name: "byYear",
          items: data.byYear.map((item) => ({
            label: String(item.year ?? t("common.unknown")),
            count: item.count,
          })),
        },
        {
          name: "byGenre",
          items: data.byGenre.map((item) => ({
            label: item.genre,
            count: item.count,
          })),
        },
        {
          name: "byMonth",
          items: data.byMonth.map((item) => ({
            label: item.month,
            count: item.count,
          })),
        },
      ]
    : [];
  return (
    <div className="flex flex-col gap-4">
      <Card>
        <CardHeader>
          <CardTitle>{t("statistics.title")}</CardTitle>
          <CardDescription>{t("statistics.hint")}</CardDescription>
        </CardHeader>
        <CardContent>
          <Button
            variant="outline"
            size="sm"
            disabled={task.pending}
            onClick={refresh}
          >
            <RefreshCwIcon data-icon="inline-start" />
            {t("common.refresh")}
          </Button>
        </CardContent>
      </Card>
      <MediaError error={task.error} retry={refresh} />
      {task.pending ? (
        <MediaLoading />
      ) : (
        data && (
          <>
            <div className="grid grid-cols-2 gap-4 lg:grid-cols-3">
              {(
                [
                  "total",
                  "planned",
                  "watching",
                  "completed",
                  "movies",
                  "tv",
                ] as const
              ).map((key) => (
                <Card key={key} size="sm">
                  <CardHeader>
                    <CardTitle>
                      {t(
                        ["planned", "watching", "completed"].includes(key)
                          ? `status.${key}`
                          : `statistics.${key}`,
                      )}
                    </CardTitle>
                  </CardHeader>
                  <CardContent>
                    <p className="text-3xl font-semibold tabular-nums">
                      {data[key]}
                    </p>
                  </CardContent>
                </Card>
              ))}
            </div>
            <div className="grid items-start gap-4 lg:grid-cols-3">
              {groups.map((group) => (
                <Card key={group.name}>
                  <CardHeader>
                    <CardTitle>{t(`statistics.${group.name}`)}</CardTitle>
                  </CardHeader>
                  <CardContent>
                    {group.items.length === 0 ? (
                      <MediaEmpty title={t("statistics.empty")} />
                    ) : (
                      <dl className="flex flex-col gap-3">
                        {group.items.map((item) => (
                          <div
                            key={item.label}
                            className="flex justify-between gap-4"
                          >
                            <dt className="break-words text-sm">
                              {item.label}
                            </dt>
                            <dd className="shrink-0 text-sm text-muted-foreground tabular-nums">
                              {t("statistics.count", { count: item.count })}
                            </dd>
                          </div>
                        ))}
                      </dl>
                    )}
                  </CardContent>
                </Card>
              ))}
            </div>
          </>
        )
      )}
    </div>
  );
}
