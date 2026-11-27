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
  GripVerticalIcon,
  MoreVerticalIcon,
  MousePointer2Icon,
} from "lucide-react";
import {
  type ReactNode,
  useEffect,
  useId,
  useMemo,
  useRef,
  useState,
} from "react";
import { useTableReorder } from "../hooks/use-table-reorder";
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
interface DataTableActionOptions {
  /** 有翻译文案的操作可提供稳定标识，切换语言时保留原按钮和返回焦点。 */
  id?: string;
  label: string;
  destructive?: boolean;
  disabled?: boolean;
}
export type DataTableAction = DataTableActionOptions &
  (
    | { onSelect: () => void; href?: never; download?: never }
    | { href: string; download?: boolean; onSelect?: never }
  );

function actionLink(action: DataTableAction) {
  return (
    <a
      href={action.disabled ? undefined : action.href}
      download={action.download || undefined}
      aria-disabled={action.disabled || undefined}
      tabIndex={action.disabled ? -1 : undefined}
      onClick={(event) => {
        if (action.disabled) event.preventDefault();
      }}
    >
      {action.label}
    </a>
  );
}
export interface DataTableProps<T extends { id: string }> {
  variant?: "default" | "management";
  label: string;
  data: T[];
  columns: DataTableColumn<T>[];
  getRowLabel: (item: T) => string;
  pagination?: DataTablePagination;
  /** 未提供服务端分页时的初始条数；不同应用可独立设置。 */
  defaultPageSize?: number;
  toolbar?: ReactNode;
  actions?: ReactNode;
  rowActions?: (item: T) => DataTableAction[];
  emptyState?: ReactNode;
  /** 控件列始终保留；禁用只影响功能，不能从布局移除。 */
  selectable?: boolean | ((item: T) => boolean);
  reorderable?: boolean | ((item: T) => boolean);
  /** 顺序由调用方保存；没有真实顺序写入时，拖拽柄保持禁用。 */
  onReorder?: (items: T[]) => void;
  /** 弹窗内的完整本地名单可不分页。 */
  paginate?: boolean;
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
          size="default"
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
  variant = "default",
  label,
  data,
  columns: definitions,
  getRowLabel,
  pagination,
  defaultPageSize = 10,
  toolbar,
  actions,
  rowActions,
  emptyState,
  selectable = true,
  reorderable = true,
  onReorder,
  paginate = true,
}: DataTableProps<T>) {
  const { t } = useUiTranslation();
  const management = variant === "management";
  const pageSizeId = useId();
  const [rowSelection, setRowSelection] = useState<RowSelectionState>({});
  const reordered = useRef<T[] | null>(null);
  const [columnVisibility, setColumnVisibility] =
    useState<ColumnVisibilityState>({});
  const [localPage, setLocalPage] = useState<PaginationState>({
    pageIndex: 0,
    pageSize: defaultPageSize,
  });
  const pageState = pagination
    ? { pageIndex: pagination.page - 1, pageSize: pagination.pageSize }
    : localPage;
  // 选中仅属于当前数据页，不把前一页或旧账户的标记带到新结果里。
  useEffect(() => {
    if (data !== reordered.current) setRowSelection({});
    reordered.current = null;
  }, [data]);
  useEffect(() => {
    setRowSelection((previous) => {
      const valid = Object.fromEntries(
        Object.entries(previous).filter(
          ([id, selected]) =>
            selected &&
            data.some(
              (item) =>
                item.id === id &&
                (typeof selectable === "function"
                  ? selectable(item)
                  : selectable),
            ),
        ),
      );
      return Object.keys(valid).length === Object.keys(previous).length
        ? previous
        : valid;
    });
  }, [data, selectable]);
  // 直接组合单元格内容，避免列定义更新时重建按钮并丢失弹窗返回焦点。
  function inlineActions(item: T) {
    return (
      <div className="flex items-center gap-1">
        {rowActions?.(item).map((action) =>
          action.href !== undefined ? (
            <Button
              key={action.id ?? action.label}
              asChild
              variant={action.destructive ? "destructive" : "ghost"}
              size="default"
              aria-label={t("table.actionFor", {
                action: action.label,
                name: getRowLabel(item),
              })}
            >
              {actionLink(action)}
            </Button>
          ) : (
            <Button
              key={action.id ?? action.label}
              type="button"
              variant="ghost"
              size="default"
              disabled={action.disabled}
              aria-label={t("table.actionFor", {
                action: action.label,
                name: getRowLabel(item),
              })}
              onClick={action.onSelect}
            >
              {action.label}
            </Button>
          ),
        )}
      </div>
    );
  }
  const columns = useMemo<ColumnDef<typeof features, T>[]>(
    () => [
      { id: "drag", header: () => null, cell: () => null, enableHiding: false },
      {
        id: "select",
        header: () => null,
        cell: () => null,
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
                <span className={management ? undefined : "sr-only"}>
                  {t("table.actions")}
                </span>
              ),
              cell: ({ row }) =>
                management || rowActions(row.original).length === 0 ? null : (
                  <DropdownMenu>
                    <DropdownMenuTrigger asChild>
                      <Button
                        type="button"
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
                            asChild={action.href !== undefined}
                            key={action.id ?? action.label}
                            variant={
                              action.destructive ? "destructive" : "default"
                            }
                            disabled={action.disabled}
                            onSelect={action.onSelect}
                          >
                            {action.href !== undefined
                              ? actionLink(action)
                              : action.label}
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
    [definitions, getRowLabel, management, rowActions, t],
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
    manualPagination: Boolean(pagination) || !paginate,
    rowCount: pagination?.total ?? data.length,
    autoResetPageIndex: false,
    enableRowSelection: (row) =>
      !pagination?.pending &&
      (typeof selectable === "function"
        ? selectable(row.original)
        : selectable),
    state: { rowSelection, columnVisibility, pagination: pageState },
    onRowSelectionChange: setRowSelection,
    onColumnVisibilityChange: setColumnVisibility,
    onPaginationChange: updatePage,
  });
  const visibleRows = table.getRowModel().rows;
  const canReorder = (item: T) =>
    Boolean(onReorder) &&
    !pagination?.pending &&
    (typeof reorderable === "function" ? reorderable(item) : reorderable) &&
    visibleRows.length > 1;
  const reorder = useTableReorder({
    ids: visibleRows.map((row) => row.id),
    dataIdentity: data,
    onMove: (id, targetId) => {
      const from = data.findIndex((item) => item.id === id);
      const to = data.findIndex((item) => item.id === targetId);
      if (!onReorder || from < 0 || to < 0 || !canReorder(data[from])) return;
      const next = [...data];
      next.splice(from, 1);
      next.splice(to, 0, data[from]);
      reordered.current = next;
      onReorder(next);
    },
    announce: (id, position) =>
      t("table.moved", {
        name: data.find((item) => item.id === id)
          ? getRowLabel(data.find((item) => item.id === id) as T)
          : id,
        position,
        count: visibleRows.length,
      }),
    cancelled: t("table.moveCancelled"),
  });
  const selectedCount = table.getSelectedRowModel().rows.length;
  const pages = Math.max(1, table.getPageCount());
  return (
    <div
      // 为绝对定位的 sr-only 文案提供就近定位范围，避免其静态位置撑高外层页面。
      className="@container/data-table relative flex min-w-0 flex-col gap-4"
      data-table-variant={variant}
    >
      <div className="flex flex-wrap items-center justify-between gap-3">
        {toolbar ? (
          <div className="min-w-0 grow basis-72">{toolbar}</div>
        ) : null}
        <div className="ms-auto flex max-w-full flex-wrap items-center gap-2">
          <DropdownMenu>
            <DropdownMenuTrigger asChild>
              <Button type="button" variant="outline" size="default">
                <Columns3Icon data-icon="inline-start" aria-hidden="true" />
                {t(management ? "table.showColumns" : "table.columns")}
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
        className={cn(
          "min-w-0 overflow-auto rounded-lg border outline-none focus-visible:outline-2 focus-visible:outline-offset-2 focus-visible:outline-ring [&_[data-slot=table-container]]:overflow-visible",
          !management && "max-h-[70svh]",
        )}
      >
        <Table>
          <TableCaption className="sr-only">{label}</TableCaption>
          <TableHeader className="sticky top-0 z-10 bg-muted">
            {table.getHeaderGroups().map((group) => (
              <TableRow key={group.id}>
                {group.headers.map((header) => (
                  <TableHead
                    key={header.id}
                    data-table-column={header.column.id}
                    scope="col"
                    className={cn(
                      definitions.find(
                        (column) => column.id === header.column.id,
                      )?.className,
                      ["drag", "select"].includes(header.column.id) &&
                        "w-10 text-center",
                      !management && header.column.id === "actions" && "w-12",
                    )}
                  >
                    {header.column.id === "drag" ? (
                      <span
                        className="flex items-center justify-center"
                        title={t("table.reorder")}
                      >
                        <MousePointer2Icon
                          className="size-4"
                          aria-hidden="true"
                        />
                        <span className="sr-only">{t("table.reorder")}</span>
                      </span>
                    ) : header.column.id === "select" ? (
                      <div className="flex items-center justify-center">
                        <Checkbox
                          checked={
                            table.getIsAllPageRowsSelected() ||
                            (table.getIsSomePageRowsSelected() &&
                              "indeterminate")
                          }
                          onCheckedChange={(value) =>
                            table.toggleAllPageRowsSelected(value === true)
                          }
                          aria-label={t("table.selectPage")}
                          aria-description={
                            selectable === false
                              ? t("table.selectionUnavailable")
                              : undefined
                          }
                          disabled={visibleRows.every(
                            (row) => !row.getCanSelect(),
                          )}
                        />
                      </div>
                    ) : header.isPlaceholder ? null : (
                      (definitions.find(
                        (column) => column.id === header.column.id,
                      )?.header ??
                      definitions.find(
                        (column) => column.id === header.column.id,
                      )?.label ?? <table.FlexRender header={header} />)
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
                  data-table-row={row.id}
                  data-table-drop={
                    reorder.drop?.id === row.id ? reorder.drop.edge : undefined
                  }
                  className={management ? "h-12" : undefined}
                  data-state={row.getIsSelected() ? "selected" : undefined}
                >
                  {row.getVisibleCells().map((cell) => (
                    <TableCell
                      key={cell.id}
                      data-table-column={cell.column.id}
                      className={cn(
                        ["drag", "select"].includes(cell.column.id) &&
                          "w-10 text-center",
                        definitions.find(
                          (column) => column.id === cell.column.id,
                        )?.className,
                      )}
                    >
                      {cell.column.id === "drag" ? (
                        <Button
                          type="button"
                          variant="ghost"
                          size="icon"
                          className="touch-none cursor-grab select-none active:cursor-grabbing"
                          data-table-drag-handle=""
                          aria-label={t("table.reorderRow", {
                            name: getRowLabel(row.original),
                          })}
                          aria-description={t(
                            canReorder(row.original)
                              ? "table.reorderHelp"
                              : "table.reorderUnavailable",
                          )}
                          disabled={!canReorder(row.original)}
                          {...reorder.handle(row.id)}
                        >
                          <GripVerticalIcon aria-hidden="true" />
                        </Button>
                      ) : cell.column.id === "select" ? (
                        <div className="flex items-center justify-center">
                          <Checkbox
                            checked={row.getIsSelected()}
                            onCheckedChange={(value) =>
                              row.toggleSelected(value === true)
                            }
                            disabled={!row.getCanSelect()}
                            aria-label={t("table.selectRow", {
                              name: getRowLabel(row.original),
                            })}
                            aria-description={
                              !row.getCanSelect()
                                ? t("table.selectionUnavailable")
                                : undefined
                            }
                          />
                        </div>
                      ) : (
                        (definitions
                          .find((column) => column.id === cell.column.id)
                          ?.cell(row.original) ??
                        (management && cell.column.id === "actions" ? (
                          inlineActions(row.original)
                        ) : (
                          <table.FlexRender cell={cell} />
                        )))
                      )}
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
      <p role="status" aria-atomic="true" className="sr-only">
        {reorder.message}
      </p>
      {(paginate || selectedCount > 0) && (
        <div className="flex flex-wrap items-center justify-end gap-3 px-4">
          {selectable !== false && (!management || selectedCount > 0) && (
            <p
              className="flex-1 text-sm text-muted-foreground"
              aria-live="polite"
            >
              {t("table.selectedRows", {
                selected: selectedCount,
                count: pagination?.total ?? data.length,
              })}
            </p>
          )}
          {paginate && (
            <nav
              aria-label={t("table.pagination")}
              className={cn(
                "flex flex-wrap items-center gap-4 @3xl/data-table:gap-8",
                management
                  ? "ms-auto w-fit max-w-full justify-end"
                  : "w-full @3xl/data-table:w-fit",
              )}
            >
              <div className="flex items-center gap-2">
                <Label
                  htmlFor={pageSizeId}
                  className={
                    management
                      ? undefined
                      : "sr-only text-sm font-medium @3xl/data-table:not-sr-only"
                  }
                >
                  {t("table.rowsPerPage")}
                </Label>
                <Select
                  value={String(pageState.pageSize)}
                  onValueChange={(value) =>
                    table.setPagination({
                      pageIndex: 0,
                      pageSize: Number(value),
                    })
                  }
                >
                  <SelectTrigger
                    id={pageSizeId}
                    size="default"
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
                {t(management ? "table.managementPage" : "table.page", {
                  page: pageState.pageIndex + 1,
                  pages,
                })}
              </p>
              <div className="ms-auto flex items-center gap-2 @3xl/data-table:ms-0">
                <Button
                  type="button"
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
                  type="button"
                  variant="outline"
                  size="icon"
                  disabled={pagination?.pending || !table.getCanPreviousPage()}
                  onClick={() => table.previousPage()}
                >
                  <ChevronLeftIcon aria-hidden="true" />
                  <span className="sr-only">{t("table.previous")}</span>
                </Button>
                <Button
                  type="button"
                  variant="outline"
                  size="icon"
                  disabled={pagination?.pending || !table.getCanNextPage()}
                  onClick={() => table.nextPage()}
                >
                  <ChevronRightIcon aria-hidden="true" />
                  <span className="sr-only">{t("table.next")}</span>
                </Button>
                <Button
                  type="button"
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
          )}
        </div>
      )}
    </div>
  );
}
