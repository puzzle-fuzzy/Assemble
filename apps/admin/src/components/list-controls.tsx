import { Button } from "@assemble/ui/button";
import { Field, FieldGroup, FieldLabel } from "@assemble/ui/field";
import { Input } from "@assemble/ui/input";
import { useId, useState } from "react";
import { useAdminTranslation } from "../lib/i18n";

export function useListQuery() {
  const [search, setSearch] = useState({
    draft: "",
    query: "",
    page: 1,
    pageSize: 10,
  });
  return {
    ...search,
    setDraft: (draft: string) => setSearch((state) => ({ ...state, draft })),
    submit: () =>
      setSearch((state) => ({ ...state, query: state.draft.trim(), page: 1 })),
    setPagination: (page: number, pageSize: number) =>
      setSearch((state) => ({ ...state, page, pageSize })),
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
