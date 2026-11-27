import { Fragment, type RefObject, useEffect, useRef } from "react";
import {
  Breadcrumb,
  BreadcrumbItem,
  BreadcrumbLink,
  BreadcrumbList,
  BreadcrumbPage,
  BreadcrumbSeparator,
} from "./breadcrumb";

export interface PageBreadcrumbAncestor {
  id: string;
  label: string;
  href: string;
  onNavigate?: () => void;
  hideOnNarrow?: boolean;
}

/** 页面层级由业务提供；两端共享链接语义、窄屏排布和跳转后的焦点。 */
export function PageBreadcrumbs({
  ancestors,
  current,
  currentRef,
}: {
  ancestors: PageBreadcrumbAncestor[];
  current: string;
  currentRef?: RefObject<HTMLSpanElement | null>;
}) {
  const localRef = useRef<HTMLSpanElement>(null);
  const pageRef = currentRef ?? localRef;
  const focusFrame = useRef<number | null>(null);
  useEffect(
    () => () => {
      if (focusFrame.current !== null)
        window.cancelAnimationFrame(focusFrame.current);
    },
    [],
  );
  return (
    <Breadcrumb className="min-w-0 flex-1">
      <BreadcrumbList className="min-w-0 flex-nowrap">
        {ancestors.map((item) => (
          <Fragment key={item.id}>
            <BreadcrumbItem
              className={
                item.hideOnNarrow
                  ? "hidden shrink-0 md:inline-flex"
                  : "shrink-0"
              }
            >
              <BreadcrumbLink
                href={item.href}
                className="whitespace-nowrap rounded-sm hover:underline focus-visible:outline-2 focus-visible:outline-offset-2 focus-visible:outline-ring focus-visible:underline"
                onClick={(event) => {
                  if (
                    !item.onNavigate ||
                    event.button !== 0 ||
                    event.metaKey ||
                    event.ctrlKey ||
                    event.shiftKey ||
                    event.altKey
                  )
                    return;
                  event.preventDefault();
                  item.onNavigate();
                  if (focusFrame.current !== null)
                    window.cancelAnimationFrame(focusFrame.current);
                  focusFrame.current = window.requestAnimationFrame(() => {
                    pageRef.current?.focus({ preventScroll: true });
                    focusFrame.current = null;
                  });
                }}
              >
                {item.label}
              </BreadcrumbLink>
            </BreadcrumbItem>
            <BreadcrumbSeparator
              className={
                item.hideOnNarrow ? "hidden shrink-0 md:block" : "shrink-0"
              }
            />
          </Fragment>
        ))}
        <BreadcrumbItem className="min-w-0">
          <BreadcrumbPage
            ref={pageRef}
            role="heading"
            aria-level={1}
            tabIndex={-1}
            title={current}
            className="block max-w-full overflow-x-auto whitespace-nowrap rounded-sm focus-visible:outline-2 focus-visible:outline-offset-2 focus-visible:outline-ring"
          >
            {current}
          </BreadcrumbPage>
        </BreadcrumbItem>
      </BreadcrumbList>
    </Breadcrumb>
  );
}
