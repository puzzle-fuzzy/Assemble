import type {
  ImageLoader,
  MediaClient,
  MediaQuery,
  MediaScope,
} from "@assemble/media";
import { Button } from "@assemble/ui/button";
import {
  Field,
  FieldGroup,
  FieldLabel,
  FieldLegend,
  FieldSet,
} from "@assemble/ui/field";
import { Input } from "@assemble/ui/input";
import { Select } from "@assemble/ui/select";
import { SearchIcon, XIcon } from "lucide-react";
import { useEffect, useState } from "react";
import { useSiteTranslation } from "../lib/i18n";
import { navigate, readQuery, wallPath } from "../lib/navigation";
import { useResource } from "../lib/use-resource";
import {
  EmptyContentView,
  LoadingContent,
  Pagination,
  RequestError,
} from "./common";
import { PosterCard } from "./poster";

export function LibraryWall({
  client,
  query,
  imageLoader,
}: {
  client: MediaClient;
  query: MediaQuery;
  imageLoader: ImageLoader;
}) {
  const { t } = useSiteTranslation();
  const { resource, retry } = useResource(
    `${client.scope}:${JSON.stringify(query)}`,
    (signal) => client.list(query, signal),
  );
  const filtered = Boolean(
    query.q || query.genre || query.year || query.status || query.mediaType,
  );
  useEffect(() => {
    if (resource.phase !== "ready") return;
    const pages = Math.max(
      1,
      Math.ceil(resource.value.total / resource.value.pageSize),
    );
    if ((query.page ?? 1) > pages)
      navigate(wallPath(client.scope, { ...query, page: pages }), true);
  }, [client.scope, query, resource]);
  return (
    <section className="flex min-w-0 flex-col gap-7">
      <header className="flex flex-col gap-2">
        <h1 className="site-heading">{t(client.scope)}</h1>
        <p className="text-sm text-muted-foreground">{t("readOnly")}</p>
      </header>
      <Filters
        key={wallPath(client.scope, query)}
        scope={client.scope}
        query={query}
      />
      {resource.phase === "loading" ? (
        <LoadingContent wall />
      ) : resource.phase === "failed" ? (
        <RequestError status={resource.status} retry={retry} />
      ) : (
        <>
          {resource.value.items.length === 0 ? (
            <EmptyContentView
              title={t(filtered ? "noResults" : "noEntries")}
              description={t(filtered ? "noResultsHelp" : "noEntriesHelp")}
            >
              {filtered && (
                <Button
                  variant="outline"
                  onClick={() => navigate(wallPath(client.scope))}
                >
                  {t("reset")}
                </Button>
              )}
            </EmptyContentView>
          ) : (
            <div className="poster-grid">
              {resource.value.items.map((entry, index) => (
                <PosterCard
                  key={entry.id}
                  item={entry}
                  imageLoader={imageLoader}
                  priority={index < 4}
                />
              ))}
            </div>
          )}
          <Pagination
            page={resource.value.page}
            total={resource.value.total}
            pageSize={resource.value.pageSize}
            onPage={(page) =>
              navigate(wallPath(client.scope, { ...query, page }))
            }
          />
        </>
      )}
    </section>
  );
}

function Filters({ scope, query }: { scope: MediaScope; query: MediaQuery }) {
  const { t } = useSiteTranslation();
  const [draft, setDraft] = useState({
    q: query.q ?? "",
    genre: query.genre ?? "",
    year: query.year?.toString() ?? "",
    mediaType: query.mediaType ?? "",
    status: query.status ?? "",
    sort: query.sort ?? "updated",
  });
  const update = (key: keyof typeof draft, value: string) =>
    setDraft((current) => ({ ...current, [key]: value }));
  return (
    <form
      onSubmit={(event) => {
        event.preventDefault();
        const next = readQuery(new URLSearchParams(draft));
        navigate(wallPath(scope, { ...next, page: 1 }));
      }}
    >
      <FieldSet>
        <FieldLegend className="sr-only">{t("filters")}</FieldLegend>
        <FieldGroup className="media-filters">
          <Field className="media-search">
            <FieldLabel htmlFor="film-search">{t("searchLabel")}</FieldLabel>
            <Input
              id="film-search"
              type="search"
              maxLength={200}
              placeholder={t("searchPlaceholder")}
              value={draft.q}
              onChange={(event) => update("q", event.target.value)}
            />
          </Field>
          <Field>
            <FieldLabel htmlFor="film-type">{t("mediaType")}</FieldLabel>
            <Select
              id="film-type"
              value={draft.mediaType}
              onChange={(event) => update("mediaType", event.target.value)}
            >
              <option value="">{t("all")}</option>
              <option value="movie">{t("movie")}</option>
              <option value="tv">{t("tv")}</option>
            </Select>
          </Field>
          <Field>
            <FieldLabel htmlFor="film-status">{t("status")}</FieldLabel>
            <Select
              id="film-status"
              value={draft.status}
              onChange={(event) => update("status", event.target.value)}
            >
              <option value="">{t("all")}</option>
              <option value="planned">{t("planned")}</option>
              <option value="watching">{t("watching")}</option>
              <option value="completed">{t("completed")}</option>
            </Select>
          </Field>
          <Field>
            <FieldLabel htmlFor="film-year">{t("year")}</FieldLabel>
            <Input
              id="film-year"
              type="number"
              min={1800}
              max={9999}
              step={1}
              placeholder={t("yearPlaceholder")}
              value={draft.year}
              onChange={(event) => update("year", event.target.value)}
            />
          </Field>
          <Field>
            <FieldLabel htmlFor="film-genre">{t("genre")}</FieldLabel>
            <Input
              id="film-genre"
              maxLength={80}
              placeholder={t("genrePlaceholder")}
              value={draft.genre}
              onChange={(event) => update("genre", event.target.value)}
            />
          </Field>
          <Field>
            <FieldLabel htmlFor="film-sort">{t("sort")}</FieldLabel>
            <Select
              id="film-sort"
              value={draft.sort}
              onChange={(event) => update("sort", event.target.value)}
            >
              <option value="updated">{t("updated")}</option>
              <option value="title">{t("titleSort")}</option>
              <option value="year">{t("yearSort")}</option>
              <option value="watched">{t("watched")}</option>
            </Select>
          </Field>
          <div className="media-filter-actions">
            <Button type="submit">
              <SearchIcon data-icon="inline-start" />
              {t("search")}
            </Button>
            <Button
              type="button"
              variant="ghost"
              onClick={() => {
                setDraft({
                  q: "",
                  genre: "",
                  year: "",
                  mediaType: "",
                  status: "",
                  sort: "updated",
                });
                navigate(wallPath(scope));
              }}
            >
              <XIcon data-icon="inline-start" />
              {t("reset")}
            </Button>
          </div>
        </FieldGroup>
      </FieldSet>
    </form>
  );
}
