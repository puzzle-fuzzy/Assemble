import { useSiteTranslation } from "../lib/i18n";

const slots = Array.from({ length: 12 }, (_, index) => index);

export function ShelfSkeleton() {
  return (
    <div className="poster-grid shelf-grid" aria-hidden="true">
      {slots.map((slot) => (
        <div className="poster-card" key={slot}>
          <div className="poster-art shelf-placeholder" />
          <div className="poster-caption">
            <div className="shelf-placeholder skeleton-title" />
          </div>
        </div>
      ))}
    </div>
  );
}

export function WallSkeleton() {
  const { t } = useSiteTranslation();
  return (
    <div className="home-shelves" aria-busy="true">
      <span className="sr-only" role="status">
        {t("loading")}
      </span>
      <div
        className="wall-filter-skeleton shelf-placeholder"
        aria-hidden="true"
      />
      <div className="home-shelf" aria-hidden="true">
        <div className="shelf-heading skeleton-heading shelf-placeholder" />
        <ShelfSkeleton />
      </div>
    </div>
  );
}
