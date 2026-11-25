import { Alert, AlertDescription, AlertTitle } from "@assemble/ui/alert";
import { Button } from "@assemble/ui/button";
import {
  Empty,
  EmptyContent,
  EmptyDescription,
  EmptyHeader,
  EmptyMedia,
  EmptyTitle,
} from "@assemble/ui/empty";
import { Skeleton } from "@assemble/ui/skeleton";
import {
  AlertCircleIcon,
  InboxIcon,
  LogInIcon,
  ShieldXIcon,
} from "lucide-react";
import type { AccessFailure, RequestFailure } from "../lib/api";

export function LoadingContent() {
  return (
    <div aria-busy="true" className="flex flex-col gap-4">
      <span className="sr-only">正在加载</span>
      <Skeleton className="h-8 w-40" />
      <Skeleton className="h-20 w-full" />
      <Skeleton className="h-20 w-full" />
    </div>
  );
}

export function RequestError({
  failure,
  retry,
}: {
  failure: RequestFailure;
  retry: () => void;
}) {
  return (
    <Alert variant="destructive">
      <AlertCircleIcon aria-hidden="true" />
      <AlertTitle>
        {failure === "notFound" ? "找不到这位用户" : "暂时无法读取数据"}
      </AlertTitle>
      <AlertDescription>
        <p>
          {failure === "notFound"
            ? "这位用户可能已不存在，请返回用户列表重新查看。"
            : "请检查网络连接后重试。"}
        </p>
        <Button variant="outline" size="sm" onClick={retry}>
          重试
        </Button>
      </AlertDescription>
    </Alert>
  );
}

export function NoResults({
  title,
  description,
}: {
  title: string;
  description: string;
}) {
  return (
    <Empty>
      <EmptyHeader>
        <EmptyMedia variant="icon">
          <InboxIcon aria-hidden="true" />
        </EmptyMedia>
        <EmptyTitle>{title}</EmptyTitle>
        <EmptyDescription>{description}</EmptyDescription>
      </EmptyHeader>
    </Empty>
  );
}

export function AccessRequired({ failure }: { failure: AccessFailure }) {
  const signedOut = failure === "signedOut";
  return (
    <Empty>
      <EmptyHeader>
        <EmptyMedia variant="icon">
          {signedOut ? (
            <LogInIcon aria-hidden="true" />
          ) : (
            <ShieldXIcon aria-hidden="true" />
          )}
        </EmptyMedia>
        <EmptyTitle>{signedOut ? "登录管理控制台" : "没有管理权限"}</EmptyTitle>
        <EmptyDescription>
          {signedOut
            ? "请使用已获授权的超级管理员账户登录，继续访问网页管理控制台。"
            : "当前账户不是超级管理员。如需切换账户，请在账户页退出后，使用已获授权的账户重新登录。"}
        </EmptyDescription>
      </EmptyHeader>
      <EmptyContent>
        <Button asChild>
          <a href={signedOut ? "/web/login?next=/admin/" : "/web/account"}>
            {signedOut ? "前往登录" : "前往账户页"}
          </a>
        </Button>
      </EmptyContent>
    </Empty>
  );
}
