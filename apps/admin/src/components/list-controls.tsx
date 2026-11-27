import { Button } from "@assemble/ui/button";
import { Field, FieldGroup, FieldLabel } from "@assemble/ui/field";
import {
  setUrlParameter,
  urlInteger,
  useUrlState,
} from "@assemble/ui/hooks/use-url-state";
import { Input } from "@assemble/ui/input";
import {
  useCallback,
  useEffect,
  useId,
  useMemo,
  useRef,
  useState,
} from "react";
import { useAdminTranslation } from "../lib/i18n";

const NO_FILTERS: Record<string, readonly string[]> = {};

export function useListQuery(
  route?: string,
  filters: Record<string, readonly string[]> = NO_FILTERS,
) {
  const codec = useMemo(
    () => ({
      read: (params: URLSearchParams) => ({
        query: (params.get("q") ?? "")
          .replace(/[\p{Cc}]/gu, "")
          .slice(0, 200)
          .trim(),
        page: urlInteger(params.get("page"), 1, 4_294_967_295),
        pageSize: urlInteger(params.get("pageSize"), 20, 100),
        filters: Object.fromEntries(
          Object.entries(filters).map(([key, options]) => [
            key,
            options.find((value) => value === params.get(key)) ?? options[0],
          ]),
        ),
      }),
      write: (
        params: URLSearchParams,
        value: {
          query: string;
          page: number;
          pageSize: number;
          filters: Record<string, string>;
        },
      ) => {
        setUrlParameter(params, "q", value.query);
        setUrlParameter(params, "page", value.page, 1);
        setUrlParameter(params, "pageSize", value.pageSize, 20);
        for (const [key, options] of Object.entries(filters))
          setUrlParameter(params, key, value.filters[key], options[0]);
      },
    }),
    [filters],
  );
  const [search, setSearch] = useUrlState(codec, route);
  const [draft, setDraft] = useState(search.query);
  const submitted = useRef(search.query);
  useEffect(() => {
    if (submitted.current !== search.query) setDraft(search.query);
    submitted.current = search.query;
  }, [search.query]);
  const correctPage = useCallback(
    (page: number, pageSize: number) =>
      setSearch((state) => ({ ...state, page, pageSize }), true),
    [setSearch],
  );
  return {
    ...search,
    draft,
    setDraft,
    correctPage,
    submit: () => {
      submitted.current = draft.trim();
      setSearch((state) => ({ ...state, query: submitted.current, page: 1 }));
    },
    setFilter: (key: string, value: string) => {
      if (!filters[key]?.includes(value)) return;
      setSearch((state) => ({
        ...state,
        page: 1,
        filters: { ...state.filters, [key]: value },
      }));
    },
    setPagination: (page: number, pageSize: number) =>
      setSearch((state) => ({
        ...state,
        page: pageSize === state.pageSize ? page : 1,
        pageSize,
      })),
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
    <search aria-label={label} className="w-full max-w-80">
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
              <Button type="submit" variant="outline" size="default">
                {t("common.search")}
              </Button>
            </div>
          </Field>
        </FieldGroup>
      </form>
    </search>
  );
}
