import { createFileRoute, Link } from '@tanstack/react-router';
import { createServerFn } from '@tanstack/react-start';
import {
  MarketingPageShell,
  marketingFontLinks,
  marketingMono,
} from '@/components/marketing-page-shell';
import {
  fetchGitHubReleases,
  formatReleaseDay,
  groupGitHubReleasesByYear,
  type GitHubRelease,
} from '@/lib/github-release';

const loadReleases = createServerFn({ method: 'GET' }).handler(async () => {
  try {
    return {
      releases: await fetchGitHubReleases(),
      error: null as string | null,
    };
  } catch (err) {
    return {
      releases: [] as GitHubRelease[],
      error: err instanceof Error ? err.message : 'Failed to load releases',
    };
  }
});

export const Route = createFileRoute('/releases/')({
  head: () => ({ links: marketingFontLinks }),
  component: ReleasesPage,
  loader: () => loadReleases(),
});

function ReleasesPage() {
  const { releases, error } = Route.useLoaderData();
  const groups = groupGitHubReleasesByYear(releases);
  const latestStableId = releases.find((release) => !release.prerelease)?.id;

  return (
    <MarketingPageShell>
      <main className="mx-auto flex w-full max-w-[40rem] flex-col px-6 pt-16 pb-20 md:pt-20">
        <h1
          className="text-4xl font-medium leading-none tracking-tight text-[#e8eeff] md:text-5xl"
          style={{ fontFamily: marketingMono }}
        >
          Releases
        </h1>
        <p
          className="mt-5 text-sm text-[#787c99]"
          style={{ fontFamily: marketingMono }}
        >
          Latest desktop builds
        </p>

        <div className="mt-12">
          {error && (
            <p className="py-8 font-mono text-sm text-fd-muted-foreground">
              <span className="text-fd-error">error:</span> could not load
              releases.
            </p>
          )}

          {!error && releases.length === 0 && (
            <p className="py-8 font-mono text-sm text-fd-muted-foreground">
              No releases yet.
            </p>
          )}

          {groups.map((group) => (
            <section key={group.year} className="border-t border-white/[0.08] py-7 first:border-t-0 first:pt-2">
              <h2
                className="text-[11px] font-medium tracking-[0.12em] text-[#565f89] uppercase"
                style={{ fontFamily: marketingMono }}
              >
                {group.year}
              </h2>
              <ul className="mt-3">
                {group.releases.map((release) => (
                  <li key={release.id}>
                    <Link
                      to="/releases/$slug"
                      params={{ slug: release.tagName }}
                      className="group flex items-baseline gap-4 border-b border-white/[0.06] py-3.5 transition-colors last:border-b-0 hover:text-white"
                    >
                      <time
                        dateTime={release.publishedAt}
                        className="w-14 shrink-0 text-xs text-[#565f89]"
                        style={{ fontFamily: marketingMono }}
                      >
                        {formatReleaseDay(release.publishedAt)}
                      </time>
                      <span className="min-w-0 flex-1 text-[15px] font-medium text-[#c0caf5] transition-colors group-hover:text-white">
                        {release.tagName}
                      </span>
                      {release.id === latestStableId && (
                        <span
                          className="shrink-0 text-[11px] text-[#7aa2f7]"
                          style={{ fontFamily: marketingMono }}
                        >
                          Latest
                        </span>
                      )}
                      {release.prerelease && (
                        <span
                          className="shrink-0 text-[11px] text-[#7aa2f7]"
                          style={{ fontFamily: marketingMono }}
                        >
                          prerelease
                        </span>
                      )}
                    </Link>
                  </li>
                ))}
              </ul>
            </section>
          ))}
        </div>

        <a
          href="https://github.com/lassejlv/termy/releases"
          target="_blank"
          rel="noreferrer"
          className="mt-8 text-xs text-[#787c99] hover:text-white"
          style={{ fontFamily: marketingMono }}
        >
          View releases on GitHub ↗
        </a>
      </main>
    </MarketingPageShell>
  );
}
