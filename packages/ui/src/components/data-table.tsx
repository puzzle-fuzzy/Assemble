// 复用 dashboard-01 的表格结构；远程列表保持服务端分页，详情中的小列表才在本机分页。
import {
  type ColumnDef,
  type ColumnVisibilityState,
  columnVisibilityFeature,
  createPaginatedRowModel,
  type PaginationState,
  type RowSelectionState,
  rowPaginationFeature,
  rowSelectionFeature,
  tableFeatures,
  type Updater,
  useTable,
} from "@tanstack/react-table";
import { cn } from "cn";
import {
  ChevronDownIcon,
  ChevronLeftIcon,
  ChevronRightIcon,
  ChevronsLeftIcon,
  ChevronsRightIcon,
  Columns3Icon,
  MoreVerticalIcon,
} from "lucide-react";
import { type ReactNode, useEffect, useId, useMemo, useState } from "react";
import { useUiTranslation } from "../lib/i18n";
import { Button } from "./button";
import { Checkbox } from "./checkbox";
import {
  Select,
  SelectContent,
  SelectGroup,
  SelectItem,
  SelectTrigger,
  SelectValue,
} from "./controls/select";
import {
  DropdownMenu,
  DropdownMenuCheckboxItem,
  DropdownMenuContent,
  DropdownMenuGroup,
  DropdownMenuItem,
  DropdownMenuRadioGroup,
  DropdownMenuRadioItem,
  DropdownMenuTrigger,
} from "./dropdown-menu";
import { Label } from "./label";
import {
  Table,
  TableBody,
  TableCaption,
  TableCell,
  TableHead,
  TableHeader,
  TableRow,
} from "./table";

const features = tableFeatures({
  columnVisibilityFeature,
  rowSelectionFeature,
  rowPaginationFeature,
  paginatedRowModel: createPaginatedRowModel(),
});

export interface DataTableColumn<T> {
  id: string;
  label: string;
  header?: ReactNode;
  cell: (item: T) => ReactNode;
  className?: string;
  hideable?: boolean;
}
export interface DataTablePagination {
  page: number;
  pageSize: number;
  total: number;
  onChange: (page: number, pageSize: number) => void;
  pending?: boolean;
}
export interface DataTableAction {
  label: string;
  onSelect: () => void;
  destructive?: boolean;
  disabled?: boolean;
}
export interface DataTableProps<T extends { id: string }> {
  label: string;
  data: T[];
  columns: DataTableColumn<T>[];
  getRowLabel: (item: T) => string;
  pagination?: DataTablePagination;
  toolbar?: ReactNode;
  actions?: ReactNode;
  rowActions?: (item: T) => DataTableAction[];
  emptyState?: ReactNode;
}

export function DataTableColumnFilter({
  label,
  value,
  options,
  onValueChange,
}: {
  label: string;
  value: string;
  options: { value: string; label: string }[];
  onValueChange: (value: string) => void;
}) {
  const selected = options.find((option) => option.value === value);
  return (
    <DropdownMenu>
      <DropdownMenuTrigger asChild>
        <Button
          type="button"
          variant="ghost"
          size="sm"
          className="-ms-2"
          aria-label={`${label}: ${selected?.label ?? label}`}
        >
          {value === "all" ? label : (selected?.label ?? label)}
          <ChevronDownIcon data-icon="inline-end" aria-hidden="true" />
        </Button>
      </DropdownMenuTrigger>
      <DropdownMenuContent align="start">
        <DropdownMenuRadioGroup value={value} onValueChange={onValueChange}>
          {options.map((option) => (
            <DropdownMenuRadioItem key={option.value} value={option.value}>
              {option.label}
            </DropdownMenuRadioItem>
          ))}
        </DropdownMenuRadioGroup>
      </DropdownMenuContent>
    </DropdownMenu>
  );
}

export function DataTable<T extends { id: string }>({
  label,
  data,
  columns: definitions,
  getRowLabel,
  pagination,
  toolbar,
  actions,
  rowActions,
  emptyState,
}: DataTableProps<T>) {
  const { t } = useUiTranslation();
  const pageSizeId = useId();
  const [rowSelection, setRowSelection] = useState<RowSelectionState>({});
  const [columnVisibility, setColumnVisibility] =
    useState<ColumnVisibilityState>({});
  const [localPage, setLocalPage] = useState<PaginationState>({
    pageIndex: 0,
    pageSize: 10,
  });
  const pageState = pagination
    ? { pageIndex: pagination.page - 1, pageSize: pagination.pageSize }
    : localPage;
  // 选中仅属于当前数据页，不把前一页或旧账户的标记带到新结果里。
  useEffect(() => {
    void data;
    setRowSelection({});
  }, [data]);
  const columns = useMemo<ColumnDef<typeof features, T>[]>(
    () => [
      {
        id: "select",
        header: ({ table }) => (
          <div className="flex items-center justify-center">
            <Checkbox
              checked={
                table.getIsAllPageRowsSelected() ||
                (table.getIsSomePageRowsSelected() && "indeterminate")
              }
              onCheckedChange={(value) =>
                table.toggleAllPageRowsSelected(value === true)
              }
              aria-label={t("table.selectPage")}
              disabled={data.length === 0}
            />
          </div>
        ),
        cell: ({ row }) => (
          <div className="flex items-center justify-center">
            <Checkbox
              checked={row.getIsSelected()}
              onCheckedChange={(value) => row.toggleSelected(value === true)}
              aria-label={t("table.selectRow", {
                name: getRowLabel(row.original),
              })}
            />
          </div>
        ),
        enableHiding: false,
      },
      ...definitions.map(
        (column): ColumnDef<typeof features, T> => ({
          id: column.id,
          accessorFn: (item) => item.id,
          header: () => column.header ?? column.label,
          cell: ({ row }) => column.cell(row.original),
          enableHiding: column.hideable !== false,
        }),
      ),
      ...(rowActions
        ? [
            {
              id: "actions",
              header: () => (
                <span className="sr-only">{t("table.actions")}</span>
              ),
              cell: ({ row }) => (
                <DropdownMenu>
                  <DropdownMenuTrigger asChild>
                    <Button
                      variant="ghost"
                      size="icon"
                      className="text-muted-foreground data-[state=open]:bg-muted"
                      aria-label={t("table.rowActions", {
                        name: getRowLabel(row.original),
                      })}
                    >
                      <MoreVerticalIcon aria-hidden="true" />
                    </Button>
                  </DropdownMenuTrigger>
                  <DropdownMenuContent align="end" className="min-w-32">
                    <DropdownMenuGroup>
                      {rowActions(row.original).map((action) => (
                        <DropdownMenuItem
                          key={action.label}
                          variant={
                            action.destructive ? "destructive" : "default"
                          }
                          disabled={action.disabled}
                          onSelect={action.onSelect}
                        >
                          {action.label}
                        </DropdownMenuItem>
                      ))}
                    </DropdownMenuGroup>
                  </DropdownMenuContent>
                </DropdownMenu>
              ),
              enableHiding: false,
            } satisfies ColumnDef<typeof features, T>,
          ]
        : []),
    ],
    [data.length, definitions, getRowLabel, rowActions, t],
  );
  function updatePage(updater: Updater<PaginationState>) {
    const next = typeof updater === "function" ? updater(pageState) : updater;
    setRowSelection({});
    if (pagination) pagination.onChange(next.pageIndex + 1, next.pageSize);
    else setLocalPage(next);
  }
  const table = useTable({
    features,
    data,
    columns,
    getRowId: (row) => row.id,
    manualPagination: Boolean(pagination),
    rowCount: pagination?.total ?? data.length,
    autoResetPageIndex: false,
    enableRowSelection: true,
    state: { rowSelection, columnVisibility, pagination: pageState },
    onRowSelectionChange: setRowSelection,
    onColumnVisibilityChange: setColumnVisibility,
    onPaginationChange: updatePage,
  });
  const pages = Math.max(1, table.getPageCount());
  return (
    <div className="@container/data-table flex min-w-0 flex-col gap-4">
      <div className="flex flex-wrap items-end justify-between gap-3">
        {toolbar ? (
          <div className="min-w-0 grow basis-72">{toolbar}</div>
        ) : null}
        <div className="ms-auto flex max-w-full flex-wrap items-center gap-2">
          <DropdownMenu>
            <DropdownMenuTrigger asChild>
              <Button variant="outline" size="sm">
                <Columns3Icon data-icon="inline-start" aria-hidden="true" />
                {t("table.columns")}
                <ChevronDownIcon data-icon="inline-end" aria-hidden="true" />
              </Button>
            </DropdownMenuTrigger>
            <DropdownMenuContent align="end" className="min-w-40">
              <DropdownMenuGroup>
                {table
                  .getAllColumns()
                  .filter((column) => column.getCanHide())
                  .map((column) => (
                    <DropdownMenuCheckboxItem
                      key={column.id}
                      checked={column.getIsVisible()}
                      onCheckedChange={(value) =>
                        column.toggleVisibility(value === true)
                      }
                    >
                      {definitions.find((item) => item.id === column.id)?.label}
                    </DropdownMenuCheckboxItem>
                  ))}
              </DropdownMenuGroup>
            </DropdownMenuContent>
          </DropdownMenu>
          {actions}
        </div>
      </div>
      {/* 表格维持二维关系；键盘可在具名区域横向滚动，不挤压或藏掉列。 */}
      <section
        aria-label={label}
        aria-busy={pagination?.pending || undefined}
        // biome-ignore lint/a11y/noNoninteractiveTabindex: 表格滚动区需要键盘入口。
        tabIndex={0}
        className="max-h-[70svh] min-w-0 overflow-auto rounded-lg border outline-none focus-visible:ring-2 focus-visible:ring-ring [&_[data-slot=table-container]]:overflow-visible"
      >
        <Table>
          <TableCaption className="sr-only">{label}</TableCaption>
          <TableHeader className="sticky top-0 z-10 bg-muted">
            {table.getHeaderGroups().map((group) => (
              <TableRow key={group.id}>
                {group.headers.map((header) => (
                  <TableHead
                    key={header.id}
                    scope="col"
                    className={cn(
                      definitions.find(
                        (column) => column.id === header.column.id,
                      )?.className,
                      ["select", "actions"].includes(header.column.id) &&
                        "w-12",
                    )}
                  >
                    {header.isPlaceholder ? null : (
                      <table.FlexRender header={header} />
                    )}
                  </TableHead>
                ))}
              </TableRow>
            ))}
          </TableHeader>
          <TableBody>
            {table.getRowModel().rows.length ? (
              table.getRowModel().rows.map((row) => (
                <TableRow
                  key={row.id}
                  data-state={row.getIsSelected() ? "selected" : undefined}
                >
                  {row.getVisibleCells().map((cell) => (
                    <TableCell
                      key={cell.id}
                      className={
                        definitions.find(
                          (column) => column.id === cell.column.id,
                        )?.className
                      }
                    >
                      <table.FlexRender cell={cell} />
                    </TableCell>
                  ))}
                </TableRow>
              ))
            ) : (
              <TableRow>
                <TableCell
                  colSpan={table.getVisibleLeafColumns().length}
                  className="h-24 text-center whitespace-normal"
                >
                  {emptyState ?? t("table.empty")}
                </TableCell>
              </TableRow>
            )}
          </TableBody>
        </Table>
      </section>
      <div className="flex flex-wrap items-center justify-between gap-3 px-4">
        <p
          className="hidden flex-1 text-sm text-muted-foreground @3xl/data-table:block"
          aria-live="polite"
        >
          {t("table.selectedRows", {
            selected: table.getSelectedRowModel().rows.length,
            count: pagination?.total ?? data.length,
          })}
        </p>
        <nav
          aria-label={t("table.pagination")}
          className="flex w-full flex-wrap items-center gap-4 @3xl/data-table:w-fit @3xl/data-table:gap-8"
        >
          <div className="flex items-center gap-2">
            <Label
              htmlFor={pageSizeId}
              className="sr-only text-sm font-medium @3xl/data-table:not-sr-only"
            >
              {t("table.rowsPerPage")}
            </Label>
            <Select
              value={String(pageState.pageSize)}
              onValueChange={(value) =>
                table.setPagination({ pageIndex: 0, pageSize: Number(value) })
              }
            >
              <SelectTrigger
                id={pageSizeId}
                size="sm"
                className="w-20"
                aria-label={t("table.rowsPerPage")}
                disabled={pagination?.pending}
              >
                <SelectValue />
              </SelectTrigger>
              <SelectContent side="top">
                <SelectGroup>
                  {[10, 20, 30, 40, 50].map((size) => (
                    <SelectItem key={size} value={String(size)}>
                      {size}
                    </SelectItem>
                  ))}
                </SelectGroup>
              </SelectContent>
            </Select>
          </div>
          <p className="text-sm font-medium" aria-live="polite">
            {t("table.page", { page: pageState.pageIndex + 1, pages })}
          </p>
          <div className="ms-auto flex items-center gap-2 @3xl/data-table:ms-0">
            <Button
              variant="outline"
              size="icon"
              className="hidden @xl/data-table:flex"
              disabled={pagination?.pending || !table.getCanPreviousPage()}
              onClick={() => table.firstPage()}
            >
              <ChevronsLeftIcon aria-hidden="true" />
              <span className="sr-only">{t("table.first")}</span>
            </Button>
            <Button
              variant="outline"
              size="icon"
              disabled={pagination?.pending || !table.getCanPreviousPage()}
              onClick={() => table.previousPage()}
            >
              <ChevronLeftIcon aria-hidden="true" />
              <span className="sr-only">{t("table.previous")}</span>
            </Button>
            <Button
              variant="outline"
              size="icon"
              disabled={pagination?.pending || !table.getCanNextPage()}
              onClick={() => table.nextPage()}
            >
              <ChevronRightIcon aria-hidden="true" />
              <span className="sr-only">{t("table.next")}</span>
            </Button>
            <Button
              variant="outline"
              size="icon"
              className="hidden @xl/data-table:flex"
              disabled={pagination?.pending || !table.getCanNextPage()}
              onClick={() => table.setPageIndex(pages - 1)}
            >
              <ChevronsRightIcon aria-hidden="true" />
              <span className="sr-only">{t("table.last")}</span>
            </Button>
          </div>
        </nav>
      </div>
    </div>
  );
}
