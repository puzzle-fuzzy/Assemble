import { Button } from "@assemble/ui/button";
import { Field, FieldGroup, FieldLabel } from "@assemble/ui/field";
import { Input } from "@assemble/ui/input";
import { ChevronLeftIcon, ChevronRightIcon, SearchIcon } from "lucide-react";
import { useId, useState } from "react";
import { useAdminTranslation } from "../lib/i18n";

export function useListQuery() {
  const [search, setSearch] = useState({ draft: "", query: "", page: 1 });
  return {
    ...search,
    setDraft: (draft: string) => setSearch((state) => ({ ...state, draft })),
    submit: () =>
      setSearch((state) => ({ ...state, query: state.draft.trim(), page: 1 })),
    setPage: (page: number) => setSearch((state) => ({ ...state, page })),
  };
}

export function SearchForm({
  label,
  placeholder,
  draft,
  setDraft,
  submit,
}: {
  label: string;
  placeholder: string;
  draft: string;
  setDraft: (value: string) => void;
  submit: () => void;
}) {
  const { t } = useAdminTranslation();
  const id = useId();
  return (
    <search aria-label={label}>
      <form
        onSubmit={(event) => {
          event.preventDefault();
          submit();
        }}
      >
        <FieldGroup>
          <Field>
            <FieldLabel htmlFor={id} className="sr-only">
              {label}
            </FieldLabel>
            <div className="flex items-center gap-2">
              <Input
                id={id}
                type="search"
                value={draft}
                maxLength={200}
                placeholder={placeholder}
                onChange={(event) => setDraft(event.target.value)}
              />
              <Button type="submit" variant="outline">
                <SearchIcon data-icon="inline-start" aria-hidden="true" />
                {t("common.search")}
              </Button>
            </div>
          </Field>
        </FieldGroup>
      </form>
    </search>
  );
}

export function PageControls({
  page,
  pageSize,
  total,
  setPage,
}: {
  page: number;
  pageSize: number;
  total: number;
  setPage: (page: number) => void;
}) {
  const { t } = useAdminTranslation();
  const pages = Math.max(1, Math.ceil(total / pageSize));
  return (
    <div className="flex flex-wrap items-center justify-between gap-2">
      <p
        className="text-sm text-muted-foreground"
        aria-live="polite"
        aria-atomic="true"
      >
        {t("pagination.summary", { count: total, page, pages })}
      </p>
      <nav aria-label={t("pagination.label")} className="flex gap-2">
        <Button
          variant="outline"
          size="sm"
          disabled={page <= 1}
          onClick={() => setPage(page - 1)}
        >
          <ChevronLeftIcon data-icon="inline-start" aria-hidden="true" />
          {t("pagination.previous")}
        </Button>
        <Button
          variant="outline"
          size="sm"
          disabled={page >= pages}
          onClick={() => setPage(page + 1)}
        >
          {t("pagination.next")}
          <ChevronRightIcon data-icon="inline-end" aria-hidden="true" />
        </Button>
      </nav>
    </div>
  );
}
