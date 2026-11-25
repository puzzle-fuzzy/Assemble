import { DisplayPreferencesMenu } from "@assemble/ui/display-preferences";
import { useCallback, useEffect, useState } from "react";
import { AdminShell } from "./components/admin-shell";
import {
  AccessRequired,
  LoadingContent,
  RequestError,
} from "./components/feedback";
import type { AccessFailure, AdminSession } from "./lib/api";
import { useAdminTranslation } from "./lib/i18n";
import { useResource } from "./lib/use-resource";

export function App() {
  const { t } = useAdminTranslation();
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
  useEffect(() => {
    if (accessFailure !== null || resource.phase !== "ready") {
      document.title = t("app.title");
    }
  }, [accessFailure, resource.phase, t]);
  if (accessFailure !== null) {
    return (
      <main className="relative flex min-h-svh items-center justify-center p-4">
        <div className="absolute top-4 right-4">
          <DisplayPreferencesMenu />
        </div>
        <AccessRequired failure={accessFailure} />
      </main>
    );
  }
  if (resource.phase === "ready")
    return <AdminShell session={resource.value} onAccessFailure={deny} />;
  return (
    <main className="relative mx-auto flex min-h-svh max-w-xl flex-col justify-center gap-4 p-4">
      <div className="absolute top-4 right-4">
        <DisplayPreferencesMenu />
      </div>
      <h1 className="text-xl font-semibold">{t("app.title")}</h1>
      {resource.phase === "loading" ? (
        <LoadingContent />
      ) : (
        <RequestError failure={resource.failure} retry={retry} />
      )}
    </main>
  );
}
