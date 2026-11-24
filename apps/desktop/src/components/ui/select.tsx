// 下拉框。
//
// 用的是系统自己的菜单：键盘、读屏与深浅色都交给系统，不必在测试里模拟一层浮层。
// 样式与 `Input` 一致。
import { cn } from "cn";
import type * as React from "react";

function Select({ className, ...props }: React.ComponentProps<"select">) {
  return (
    <select
      data-slot="select"
      className={cn(
        "h-8 w-full min-w-0 rounded-lg border border-input bg-transparent px-2 py-1 text-base transition-colors outline-none focus-visible:border-ring focus-visible:ring-3 focus-visible:ring-ring/50 disabled:pointer-events-none disabled:cursor-not-allowed disabled:bg-input/50 disabled:opacity-50 dark:bg-input/30 dark:disabled:bg-input/80 md:text-sm",
        className,
      )}
      {...props}
    />
  );
}

export { Select };
