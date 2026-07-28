import { createFileRoute, Link, notFound } from '@tanstack/react-router';
import { createServerFn } from '@tanstack/react-start';
import {
  MarketingPageShell,
  marketingFontLinks,
  marketingMono,
} from '@/components/marketing-page-shell';
import {
  fetchGitHubReleaseByTag,
  formatReleaseDate,
} from '@/lib/github-release';
import { Markdown } from '@/components/markdown';

const loadRelease = createServerFn({ method: 'GET' })
  .inputValidator((slug: string) => slug)
  .handler(async ({ data: slug }) => {
    const release = await fetchGitHubReleaseByTag(slug);
    if (!release) return { release: null, notFound: true as const };
    return { release, notFound: false as const };
  });

export const Route = createFileRoute('/releases/$slug')({
  head: () => ({ links: marketingFontLinks }),
  component: ReleaseDetail,
  loader: async ({ params }) => {
    const result = await loadRelease({ data: params.slug });
    if (result.notFound) throw notFound();
    return result;
  },
});

function ReleaseDetail() {
  const { release } = Route.useLoaderData();
  if (!release) return null;

  return (
    <MarketingPageShell>
      <main className="mx-auto flex w-full max-w-[40rem] flex-col px-6 pt-16 pb-20 md:pt-20">
        <Link
          to="/releases"
          className="text-xs text-[#787c99] hover:text-white"
          style={{ fontFamily: marketingMono }}
        >
          ← all releases
        </Link>

        <article className="mt-10">
          <time
            dateTime={release.publishedAt}
            className="text-xs text-[#787c99]"
            style={{ fontFamily: marketingMono }}
          >
            {formatReleaseDate(release.publishedAt)}
          </time>
          <h1
            className="mt-3 text-4xl font-medium leading-none tracking-tight text-[#e8eeff] md:text-5xl"
            style={{ fontFamily: marketingMono }}
          >
            {release.tagName}
          </h1>

          <div className="release-notes prose prose-invert mt-10 max-w-none">
            <Markdown
              text={release.body || '_No release notes were provided._'}
            />
          </div>
        </article>

        <div
          className="mt-8 flex flex-wrap gap-x-6 gap-y-2 text-xs text-[#787c99]"
          style={{ fontFamily: marketingMono }}
        >
          <a
            href={release.htmlUrl}
            target="_blank"
            rel="noreferrer"
            className="hover:text-white"
          >
            View on GitHub ↗
          </a>
          <a href={release.tarballUrl} className="hover:text-white">
            Source tarball ↓
          </a>
        </div>
      </main>
    </MarketingPageShell>
  );
}
