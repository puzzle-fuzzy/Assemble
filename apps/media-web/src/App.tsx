import {
  browserImageLoader,
  createBrowserMediaClient,
  createBrowserMediaCollectionsClient,
  getMediaSession,
} from "@assemble/media/browser-client";
import type { MediaSession } from "@assemble/media/model";
import { Alert, AlertDescription, AlertTitle } from "@assemble/ui/alert";
import { Button } from "@assemble/ui/button";
import { DisplayPreferencesMenu } from "@assemble/ui/display-preferences";
import {
  lazy,
  Suspense,
  useCallback,
  useEffect,
  useMemo,
  useRef,
  useState,
} from "react";
import {
  EmptyContentView,
  LoadingContent,
  SiteLink,
} from "./components/common";
import { ReaderBoundary } from "./components/reader-boundary";
import { WallSkeleton } from "./components/shelf-skeleton";
import { LibraryWall } from "./components/wall";
import { createHomePreloadFetch, discardHomePreload } from "./lib/home-preload";
import { useSiteTranslation } from "./lib/i18n";
import {
  loginPath,
  readRoute,
  type SiteRoute,
  useSiteLocation,
} from "./lib/navigation";

const EntryReader = lazy(() =>
  import("./components/readers").then((module) => ({
    default: module.EntryReader,
  })),
);
const CatalogReader = lazy(() =>
  import("./components/readers").then((module) => ({
    default: module.CatalogReader,
  })),
);
const ActorReader = lazy(() =>
  import("./components/readers").then((module) => ({
    default: module.ActorReader,
  })),
);
const CollectionsReader = lazy(() =>
  import("./components/readers").then((module) => ({
    default: module.CollectionsReader,
  })),
);
const CollectionReader = lazy(() =>
  import("./components/readers").then((module) => ({
    default: module.CollectionReader,
  })),
);

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
    discardHomePreload();
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
        if (!next.signal.aborted) {
          if (session.account !== null) discardHomePreload();
          setState({
            phase: "ready",
            session,
            failed: false,
            checking: false,
            signal: next.signal,
          });
        }
      },
      () => {
        if (!next.signal.aborted) {
          discardHomePreload();
          setState({
            phase: "ready",
            session: { account: null },
            failed: true,
            checking: false,
            signal: next.signal,
          });
        }
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
        discardHomePreload();
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
      <main id="main-content" className="site-main" tabIndex={-1}>
        {state.phase === "checking" ? (
          route.kind === "wall" ? (
            <WallSkeleton />
          ) : (
            <LoadingContent />
          )
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
                    <Button variant="outline" size="default" onClick={refresh}>
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
                <ReaderBoundary
                  key={`${account?.id ?? "anonymous"}:${location.split("?")[0]}`}
                  retry={() => window.location.reload()}
                >
                  <Suspense fallback={<LoadingContent />}>
                    <SiteContent
                      route={route}
                      signedIn={account !== null}
                      confirmedAnonymous={
                        !state.failed && state.session.account === null
                      }
                      sessionSignal={state.signal}
                    />
                  </Suspense>
                </ReaderBoundary>
              )}
            </div>
          </>
        )}
      </main>
      <footer className="site-footer">
        <div className="site-footer-row">
          <nav className="site-footer-links" aria-label={t("navigation")}>
            <SiteLink
              href="/movies/"
              aria-current={
                !privateRoute && !collectionsRoute ? "page" : undefined
              }
            >
              {t("shared")}
            </SiteLink>
            <SiteLink
              href="/movies/private"
              aria-current={privateRoute ? "page" : undefined}
            >
              {t("private")}
            </SiteLink>
            <SiteLink
              href="/movies/collections"
              aria-current={collectionsRoute ? "page" : undefined}
            >
              {t("collections")}
            </SiteLink>
            <a href={account ? "/web/account" : loginPath}>
              {account ? t("account") : t("signIn")}
            </a>
          </nav>
          <DisplayPreferencesMenu />
        </div>
        <div className="site-attribution">
          <a
            href="https://www.themoviedb.org/"
            target="_blank"
            rel="noreferrer"
            className="underline underline-offset-4"
          >
            TMDB
          </a>
          <p>{t("tmdbNotice")}</p>
        </div>
      </footer>
    </div>
  );
}

function SiteContent({
  route,
  signedIn,
  confirmedAnonymous,
  sessionSignal,
}: {
  route: SiteRoute;
  signedIn: boolean;
  confirmedAnonymous: boolean;
  sessionSignal: AbortSignal;
}) {
  const { t } = useSiteTranslation();
  const shared = useMemo(
    () =>
      createBrowserMediaClient(
        "shared",
        confirmedAnonymous
          ? { fetch: createHomePreloadFetch({ account: null }) }
          : undefined,
      ),
    [confirmedAnonymous],
  );
  const privateClient = useMemo(() => createBrowserMediaClient("private"), []);
  const collections = useMemo(() => createBrowserMediaCollectionsClient(), []);
  const props = { imageLoader: browserImageLoader };
  switch (route.kind) {
    case "wall":
      return (
        <LibraryWall
          client={route.scope === "private" ? privateClient : shared}
          query={route.query}
          pages={route.pages}
          sessionSignal={sessionSignal}
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
