import { useCallback, useState } from "react";
import { AdminShell } from "./components/admin-shell";
import {
  AccessRequired,
  LoadingContent,
  RequestError,
} from "./components/feedback";
import type { AccessFailure, AdminSession } from "./lib/api";
import { useResource } from "./lib/use-resource";

export function App() {
  const [accessFailure, setAccessFailure] = useState<AccessFailure | null>(
    null,
  );
  const deny = useCallback(
    (failure: AccessFailure) => setAccessFailure(failure),
    [],
  );
  const { resource, retry } = useResource<AdminSession>(
    "/v1/admin/session",
    deny,
  );
  if (accessFailure !== null) {
    return (
      <main className="flex min-h-svh items-center justify-center p-4">
        <AccessRequired failure={accessFailure} />
      </main>
    );
  }
  if (resource.phase === "ready")
    return <AdminShell session={resource.value} onAccessFailure={deny} />;
  return (
    <main className="mx-auto flex min-h-svh max-w-xl flex-col justify-center gap-4 p-4">
      <h1 className="text-xl font-semibold">Assemble 管理控制台</h1>
      {resource.phase === "loading" ? (
        <LoadingContent />
      ) : (
        <RequestError failure={resource.failure} retry={retry} />
      )}
    </main>
  );
}
