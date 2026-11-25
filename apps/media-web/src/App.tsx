import {
  browserImageLoader,
  createBrowserMediaClient,
  createBrowserMediaCollectionsClient,
  getMediaSession,
  type MediaSession,
} from "@assemble/media";
import { Alert, AlertDescription, AlertTitle } from "@assemble/ui/alert";
import { Button } from "@assemble/ui/button";
import { DisplayPreferencesMenu } from "@assemble/ui/display-preferences";
import { Separator } from "@assemble/ui/separator";
import { FilmIcon, LogInIcon, UserRoundIcon } from "lucide-react";
import { useCallback, useEffect, useMemo, useRef, useState } from "react";
import {
  EmptyContentView,
  LoadingContent,
  SiteLink,
} from "./components/common";
import {
  ActorReader,
  CatalogReader,
  CollectionReader,
  CollectionsReader,
  EntryReader,
} from "./components/readers";
import { LibraryWall } from "./components/wall";
import { useSiteTranslation } from "./lib/i18n";
import {
  loginPath,
  readRoute,
  type SiteRoute,
  useSiteLocation,
} from "./lib/navigation";

type SessionState =
  | { phase: "checking" }
  | {
      phase: "ready";
      session: MediaSession;
      failed: boolean;
      checking: boolean;
      signal: AbortSignal;
    };

function useSession() {
  const [state, setState] = useState<SessionState>({ phase: "checking" });
  const controller = useRef<AbortController | null>(null);
  const current = useRef(state);
  current.current = state;
  const previousFocus = useRef<{
    element: HTMLElement;
    accountId: string | null;
  } | null>(null);
  const prepareCheck = useCallback(() => {
    const snapshot = current.current;
    if (snapshot.phase !== "ready") return;
    if (!snapshot.checking && document.activeElement instanceof HTMLElement) {
      previousFocus.current = {
        element: document.activeElement,
        accountId: snapshot.session.account?.id ?? null,
      };
    }
    // 核验期间保留同账户的草稿和焦点，但旧视图不可见、不可操作。
    setState((value) =>
      value.phase === "ready" ? { ...value, checking: true } : value,
    );
  }, []);
  const refresh = useCallback(() => {
    controller.current?.abort();
    const next = new AbortController();
    controller.current = next;
    prepareCheck();
    void getMediaSession(next.signal).then(
      (session) => {
        if (!next.signal.aborted)
          setState({
            phase: "ready",
            session,
            failed: false,
            checking: false,
            signal: next.signal,
          });
      },
      () => {
        if (!next.signal.aborted)
          setState({
            phase: "ready",
            session: { account: null },
            failed: true,
            checking: false,
            signal: next.signal,
          });
      },
    );
  }, [prepareCheck]);
  useEffect(() => {
    if (state.phase !== "ready" || state.checking) return;
    const target = previousFocus.current;
    previousFocus.current = null;
    if (
      target?.accountId === (state.session.account?.id ?? null) &&
      target.element.isConnected &&
      (document.activeElement === document.body ||
        document.activeElement === target.element)
    ) {
      target.element.focus({ preventScroll: true });
    }
  }, [state]);
  useEffect(() => {
    refresh();
    const visibility = () => {
      if (document.visibilityState === "hidden") {
        controller.current?.abort();
        prepareCheck();
      } else refresh();
    };
    window.addEventListener("focus", refresh);
    window.addEventListener("pageshow", refresh);
    document.addEventListener("visibilitychange", visibility);
    return () => {
      controller.current?.abort();
      window.removeEventListener("focus", refresh);
      window.removeEventListener("pageshow", refresh);
      document.removeEventListener("visibilitychange", visibility);
    };
  }, [prepareCheck, refresh]);
  return { state, refresh };
}

export function App() {
  const { t } = useSiteTranslation();
  const location = useSiteLocation();
  const route = useMemo(() => readRoute(location), [location]);
  const { state, refresh } = useSession();
  const account = state.phase === "ready" ? state.session.account : null;
  const privateRoute =
    (route.kind === "wall" || route.kind === "entry") &&
    route.scope === "private";
  const collectionsRoute =
    route.kind === "collections" || route.kind === "collection";
  const baseTitle =
    route.kind === "wall" ? `${t(route.scope)} · ${t("brand")}` : t("title");
  useEffect(() => {
    document.title = baseTitle;
  }, [baseTitle]);
  return (
    <div className="site-shell">
      <a href="#main-content" className="skip-link">
        {t("skip")}
      </a>
      <header className="site-header">
        <div className="site-header-inner">
          <SiteLink href="/movies/" className="site-brand">
            <FilmIcon aria-hidden="true" />
            <span>{t("brand")}</span>
          </SiteLink>
          <nav className="site-nav" aria-label={t("navigation")}>
            <Button
              asChild
              variant={
                !privateRoute && !collectionsRoute ? "secondary" : "ghost"
              }
            >
              <SiteLink
                href="/movies/"
                aria-current={
                  !privateRoute && !collectionsRoute ? "page" : undefined
                }
              >
                {t("shared")}
              </SiteLink>
            </Button>
            <Button asChild variant={privateRoute ? "secondary" : "ghost"}>
              <SiteLink
                href="/movies/private"
                aria-current={privateRoute ? "page" : undefined}
              >
                {t("private")}
              </SiteLink>
            </Button>
            <Button asChild variant={collectionsRoute ? "secondary" : "ghost"}>
              <SiteLink
                href="/movies/collections"
                aria-current={collectionsRoute ? "page" : undefined}
              >
                {t("collections")}
              </SiteLink>
            </Button>
          </nav>
          <div className="site-account">
            <DisplayPreferencesMenu />
            <Button asChild variant="outline">
              <a href={account ? "/web/account" : loginPath}>
                {account ? (
                  <UserRoundIcon data-icon="inline-start" />
                ) : (
                  <LogInIcon data-icon="inline-start" />
                )}
                <span>{account ? account.displayName : t("signIn")}</span>
              </a>
            </Button>
          </div>
        </div>
      </header>
      <main id="main-content" className="site-main" tabIndex={-1}>
        {state.phase === "checking" ? (
          <LoadingContent />
        ) : (
          <>
            {state.checking && <LoadingContent />}
            <div
              className="site-session-content"
              hidden={state.checking}
              inert={state.checking}
              data-session-content="true"
            >
              {state.failed && (
                <Alert>
                  <AlertTitle>{t("sessionError")}</AlertTitle>
                  <AlertDescription className="flex flex-col items-start gap-3">
                    <p>{t("sessionHelp")}</p>
                    <Button variant="outline" size="sm" onClick={refresh}>
                      {t("retry")}
                    </Button>
                  </AlertDescription>
                </Alert>
              )}
              {privateRoute && !account ? (
                <EmptyContentView
                  title={t("privateAccess")}
                  description={t("privateHelp")}
                >
                  <Button asChild>
                    <a href={loginPath}>{t("signIn")}</a>
                  </Button>
                  {state.failed && (
                    <Button variant="outline" onClick={refresh}>
                      {t("retry")}
                    </Button>
                  )}
                </EmptyContentView>
              ) : (
                <SiteContent
                  key={`${account?.id ?? "anonymous"}:${location}`}
                  route={route}
                  signedIn={account !== null}
                  sessionSignal={state.signal}
                />
              )}
            </div>
          </>
        )}
      </main>
      <footer className="site-footer">
        <Separator />
        <div className="flex flex-col gap-2">
          <p>{account ? t("readOnly") : t("anonymous")}</p>
          <p>
            <a
              href="https://www.themoviedb.org/"
              target="_blank"
              rel="noreferrer"
              className="underline underline-offset-4"
            >
              TMDB
            </a>
            <span aria-hidden="true"> · </span>
            <a
              href="https://movie.douban.com/"
              target="_blank"
              rel="noreferrer"
              className="underline underline-offset-4"
            >
              豆瓣 / Douban
            </a>
          </p>
          <p>{t("tmdbNotice")}</p>
        </div>
      </footer>
    </div>
  );
}

function SiteContent({
  route,
  signedIn,
  sessionSignal,
}: {
  route: SiteRoute;
  signedIn: boolean;
  sessionSignal: AbortSignal;
}) {
  const { t } = useSiteTranslation();
  const shared = useMemo(() => createBrowserMediaClient("shared"), []);
  const privateClient = useMemo(() => createBrowserMediaClient("private"), []);
  const collections = useMemo(() => createBrowserMediaCollectionsClient(), []);
  const props = { imageLoader: browserImageLoader };
  switch (route.kind) {
    case "wall":
      return (
        <LibraryWall
          client={route.scope === "private" ? privateClient : shared}
          query={route.query}
          {...props}
        />
      );
    case "entry":
      return (
        <EntryReader
          client={route.scope === "private" ? privateClient : shared}
          id={route.id}
          signedIn={signedIn}
          sessionSignal={sessionSignal}
          {...props}
        />
      );
    case "catalog":
      return <CatalogReader client={shared} route={route} {...props} />;
    case "actor":
      return <ActorReader client={shared} route={route} {...props} />;
    case "collections":
      return (
        <CollectionsReader client={collections} page={route.page} {...props} />
      );
    case "collection":
      return <CollectionReader client={collections} id={route.id} {...props} />;
    case "missing":
      return (
        <EmptyContentView title={t("notFound")} description={t("notFoundHelp")}>
          <Button asChild variant="outline">
            <SiteLink href="/movies/">{t("back")}</SiteLink>
          </Button>
        </EmptyContentView>
      );
  }
}
