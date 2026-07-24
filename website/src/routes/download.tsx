import { createFileRoute, Link } from '@tanstack/react-router';
import { createServerFn } from '@tanstack/react-start';
import { TriangleAlert } from 'lucide-react';
import { useRef, useState, type ReactNode } from 'react';
import {
  MarketingPageShell,
  marketingFontLinks,
  marketingLinkClass,
  marketingMono,
  marketingPanelClass,
} from '@/components/marketing-page-shell';
import {
  assetArch,
  fetchLatestGitHubRelease,
  fetchLatestNativeMacosRelease,
  formatBytes,
  formatReleaseDate,
  groupReleaseAssets,
  type GitHubRelease,
  type PlatformAssetGroup,
} from '@/lib/github-release';

type DownloadChannel = 'desktop' | 'native';
type DownloadEdition = 'native-macos';

type DownloadSearch = {
  edition?: DownloadEdition;
};

function channelFromEdition(edition: DownloadEdition | undefined): DownloadChannel {
  return edition === 'native-macos' ? 'native' : 'desktop';
}

function editionFromChannel(channel: DownloadChannel): DownloadEdition | undefined {
  return channel === 'native' ? 'native-macos' : undefined;
}

const loadDownloadReleases = createServerFn({ method: 'GET' }).handler(
  async () => {
    const [desktop, native] = await Promise.allSettled([
      fetchLatestGitHubRelease(),
      fetchLatestNativeMacosRelease(),
    ]);

    return {
      desktop:
        desktop.status === 'fulfilled'
          ? {
              release: desktop.value,
              error: null as string | null,
            }
          : {
              release: null as GitHubRelease | null,
              error:
                desktop.reason instanceof Error
                  ? desktop.reason.message
                  : 'Failed to load latest release',
            },
      native:
        native.status === 'fulfilled'
          ? {
              release: native.value,
              error: null as string | null,
            }
          : {
              release: null as GitHubRelease | null,
              error:
                native.reason instanceof Error
                  ? native.reason.message
                  : 'Failed to load native beta release',
            },
    };
  },
);

export const Route = createFileRoute('/download')({
  head: () => ({ links: marketingFontLinks }),
  validateSearch: (search: Record<string, unknown>): DownloadSearch => ({
    edition: search.edition === 'native-macos' ? 'native-macos' : undefined,
  }),
  component: DownloadPage,
  loader: () => loadDownloadReleases(),
});

function DownloadPage() {
  const { desktop, native } = Route.useLoaderData();
  const { edition } = Route.useSearch();
  const navigate = Route.useNavigate();
  const channel = channelFromEdition(edition);
  const dialogRef = useRef<HTMLDialogElement>(null);
  const [pendingDownload, setPendingDownload] = useState<{
    name: string;
    url: string;
  } | null>(null);

  const setChannel = (next: DownloadChannel) => {
    void navigate({
      search: (prev) => {
        const nextEdition = editionFromChannel(next);
        if (nextEdition) return { ...prev, edition: nextEdition };
        const { edition: _edition, ...rest } = prev;
        return rest;
      },
      replace: true,
    });
  };

  const active = channel === 'desktop' ? desktop : native;
  const release = active.release;
  const error = active.error;
  const groups = release ? groupReleaseAssets(release.assets) : [];
  const githubUrl =
    release?.htmlUrl ??
    (channel === 'native'
      ? 'https://github.com/lassejlv/termy/releases?q=macos-native'
      : 'https://github.com/lassejlv/termy/releases');

  const warnBeforeMacDownload = (name: string, url: string) => {
    setPendingDownload({ name, url });
    dialogRef.current?.showModal();
  };

  const continueMacDownload = () => {
    const url = pendingDownload?.url;
    dialogRef.current?.close();
    if (url) window.location.assign(url);
  };

  return (
    <MarketingPageShell>
      <main className="mx-auto flex w-full max-w-4xl flex-col px-6 pt-20 pb-24 md:pt-28">
        <p className="text-sm text-[#7aa2f7]" style={{ fontFamily: marketingMono }}>
          $ termy install
        </p>
        <h1
          className="mt-3 text-4xl font-medium leading-none tracking-tight text-[#e8eeff] md:text-5xl"
          style={{ fontFamily: marketingMono }}
        >
          Download
        </h1>

        <div
          className="mt-8 inline-flex w-fit items-center rounded-full border border-white/[0.08] bg-[#14141c]/70 p-1 backdrop-blur-md"
          role="tablist"
          aria-label="Download channel"
        >
          <ChannelTab
            id="desktop"
            active={channel === 'desktop'}
            onSelect={setChannel}
          >
            Desktop
          </ChannelTab>
          <ChannelTab
            id="native"
            active={channel === 'native'}
            onSelect={setChannel}
            badge="beta"
          >
            Native macOS
          </ChannelTab>
        </div>

        {channel === 'native' && (
          <p className="mt-5 max-w-2xl text-sm leading-relaxed text-[#787c99]">
            Public beta of the SwiftUI macOS host, published separately from the
            desktop app. Unsigned builds for Apple silicon and Intel — clear
            quarantine after installing.
          </p>
        )}

        {release && (
          <p
            className="mt-5 text-sm text-[#787c99]"
            style={{ fontFamily: marketingMono }}
          >
            <span className="text-[#c0caf5]">{release.tagName}</span>
            {' · '}
            {formatReleaseDate(release.publishedAt)}
            {' · '}
            {channel === 'desktop' ? (
              <Link to="/releases" className={marketingLinkClass}>
                release notes
              </Link>
            ) : (
              <a
                href={release.htmlUrl}
                target="_blank"
                rel="noreferrer"
                className={marketingLinkClass}
              >
                release notes ↗
              </a>
            )}
          </p>
        )}

        <div
          className={`${marketingPanelClass} mt-12 divide-y divide-white/[0.07] px-5 sm:px-7`}
        >
          <AssetPanel
            channel={channel}
            error={error}
            release={release}
            groups={groups}
            githubUrl={githubUrl}
            onMacDownload={warnBeforeMacDownload}
          />
        </div>

        <footer
          className="mt-8 flex flex-wrap gap-x-6 gap-y-2 text-xs text-[#787c99]"
          style={{ fontFamily: marketingMono }}
        >
          <Link to="/releases" className="hover:text-white">
            all releases →
          </Link>
          <a
            href={githubUrl}
            target="_blank"
            rel="noreferrer"
            className="hover:text-white"
          >
            GitHub ↗
          </a>
          {channel === 'desktop' && release && (
            <a href={release.tarballUrl} className="hover:text-white">
              source tarball ↓
            </a>
          )}
        </footer>

        <dialog
          ref={dialogRef}
          aria-labelledby="macos-download-warning-title"
          aria-describedby="macos-download-warning-description"
          onClose={() => setPendingDownload(null)}
          className="download-warning m-auto w-[min(34rem,calc(100%-2rem))] rounded-2xl border border-white/[0.1] bg-[#16161e] p-0 text-[#c0caf5] shadow-[0_30px_100px_rgba(0,0,0,0.55)] backdrop:bg-[#080a10]/70 backdrop:backdrop-blur-sm"
        >
          <div className="p-6 sm:p-7">
            <div className="flex items-start gap-4">
              <span className="flex size-10 shrink-0 items-center justify-center rounded-full border border-[#7aa2f7]/25 bg-[#7aa2f7]/10 text-[#7aa2f7]">
                <TriangleAlert className="size-5" />
              </span>
              <div className="min-w-0">
                <p
                  className="text-[10px] text-[#565f89]"
                  style={{ fontFamily: marketingMono }}
                >
                  macOS installation
                </p>
                <h2
                  id="macos-download-warning-title"
                  className="mt-1 text-2xl font-medium leading-tight text-[#e8eeff]"
                  style={{ fontFamily: marketingMono }}
                >
                  Termy is not signed yet.
                </h2>
              </div>
            </div>

            <p
              id="macos-download-warning-description"
              className="mt-5 text-sm leading-relaxed text-[#787c99]"
            >
              After moving Termy to Applications, macOS may prevent it from
              opening. Run this command once in Terminal to remove the
              quarantine attribute:
            </p>

            <pre
              className="mt-4 overflow-x-auto rounded-xl border border-white/[0.08] bg-[#0d0f17] px-4 py-3.5 text-xs leading-relaxed text-[#9ece6a]"
              style={{ fontFamily: marketingMono }}
            >
              <code>
                {channel === 'native'
                  ? 'xattr -dr com.apple.quarantine /Applications/Termy.app'
                  : 'sudo xattr -d com.apple.quarantine /Applications/Termy.app'}
              </code>
            </pre>

            <a
              href="https://termy.sh/docs/getting-started/troubleshooting"
              target="_blank"
              rel="noreferrer"
              className="mt-4 inline-block text-xs text-[#7aa2f7] underline decoration-[#7aa2f7]/35 underline-offset-4 hover:decoration-[#7aa2f7]"
              style={{ fontFamily: marketingMono }}
            >
              Read the troubleshooting guide ↗
            </a>

            <div className="mt-7 flex flex-col-reverse gap-3 border-t border-white/[0.08] pt-5 sm:flex-row sm:items-center sm:justify-between">
              <p
                className="min-w-0 truncate text-[10px] text-[#565f89]"
                style={{ fontFamily: marketingMono }}
              >
                {pendingDownload?.name}
              </p>
              <div className="flex shrink-0 gap-3">
                <button
                  type="button"
                  onClick={() => dialogRef.current?.close()}
                  className="rounded-full border border-white/[0.1] px-4 py-2 text-xs text-[#787c99] transition-colors hover:text-white active:scale-[0.97]"
                >
                  Cancel
                </button>
                <button
                  type="button"
                  onClick={continueMacDownload}
                  className="rounded-full bg-[#9fc0ff] px-4 py-2 text-xs font-medium text-[#10192e] transition-transform hover:scale-[1.02] active:scale-[0.97]"
                >
                  Continue download
                </button>
              </div>
            </div>
          </div>
        </dialog>
      </main>
    </MarketingPageShell>
  );
}

function ChannelTab({
  id,
  active,
  onSelect,
  badge,
  children,
}: {
  id: DownloadChannel;
  active: boolean;
  onSelect: (id: DownloadChannel) => void;
  badge?: string;
  children: ReactNode;
}) {
  return (
    <button
      type="button"
      role="tab"
      id={`download-tab-${id}`}
      aria-selected={active}
      onClick={() => onSelect(id)}
      className={`rounded-full px-4 py-2 text-sm transition-colors active:scale-[0.97] ${
        active
          ? 'bg-[#24283b] text-[#e8eeff] shadow-[inset_0_0_0_1px_rgba(255,255,255,0.08)]'
          : 'text-[#787c99] hover:text-[#c0caf5]'
      }`}
      style={{ fontFamily: marketingMono }}
    >
      {children}
      {badge && (
        <span
          className={`ml-2 text-[10px] uppercase tracking-wide ${
            active ? 'text-[#7aa2f7]' : 'text-[#565f89]'
          }`}
        >
          {badge}
        </span>
      )}
    </button>
  );
}

function AssetPanel({
  channel,
  error,
  release,
  groups,
  githubUrl,
  onMacDownload,
}: {
  channel: DownloadChannel;
  error: string | null;
  release: GitHubRelease | null;
  groups: PlatformAssetGroup[];
  githubUrl: string;
  onMacDownload: (name: string, url: string) => void;
}) {
  if (error) {
    return (
      <p className="py-8 font-mono text-sm text-fd-muted-foreground">
        <span className="text-fd-error">error:</span> could not reach GitHub.{' '}
        <a
          href={
            channel === 'native'
              ? 'https://github.com/lassejlv/termy/releases?q=macos-native'
              : 'https://github.com/lassejlv/termy/releases/latest'
          }
          target="_blank"
          rel="noreferrer"
          className={marketingLinkClass}
        >
          Download from GitHub →
        </a>
      </p>
    );
  }

  if (!release) {
    return (
      <p className="py-8 font-mono text-sm text-fd-muted-foreground">
        {channel === 'native'
          ? 'No native macOS beta release yet.'
          : 'No release published yet.'}{' '}
        <a
          href={githubUrl}
          target="_blank"
          rel="noreferrer"
          className={marketingLinkClass}
        >
          View on GitHub →
        </a>
      </p>
    );
  }

  if (groups.length === 0) {
    return (
      <p className="py-8 font-mono text-sm text-fd-muted-foreground">
        No binaries for this release yet.{' '}
        <a
          href={githubUrl}
          target="_blank"
          rel="noreferrer"
          className={marketingLinkClass}
        >
          View on GitHub →
        </a>
      </p>
    );
  }

  return (
    <>
      {groups.map((group) => (
        <section key={group.id} className="py-7">
          <h2
            className="text-base font-medium text-[#e8eeff]"
            style={{ fontFamily: marketingMono }}
          >
            {group.title}
          </h2>
          <ul className="mt-3 divide-y divide-white/[0.06]">
            {group.assets.map((asset) => {
              const arch = assetArch(asset.name);
              return (
                <li key={asset.id}>
                  <a
                    href={asset.downloadUrl}
                    onClick={(event) => {
                      if (
                        group.id === 'macos' &&
                        (arch === 'arm64' || arch === 'x64')
                      ) {
                        event.preventDefault();
                        onMacDownload(asset.name, asset.downloadUrl);
                      }
                    }}
                    className="group -mx-2 flex items-center gap-3 rounded-lg px-2 py-3.5 transition-colors hover:bg-white/[0.04]"
                  >
                    <span
                      className="min-w-0 flex-1 break-all text-sm text-[#c0caf5]"
                      style={{ fontFamily: marketingMono }}
                    >
                      {asset.name}
                    </span>
                    {arch && (
                      <span
                        className="hidden shrink-0 text-[10px] text-[#565f89] sm:inline"
                        style={{ fontFamily: marketingMono }}
                      >
                        {arch}
                      </span>
                    )}
                    <span
                      className="shrink-0 text-xs text-[#787c99] tabular-nums"
                      style={{ fontFamily: marketingMono }}
                    >
                      {formatBytes(asset.size)}
                    </span>
                  </a>
                </li>
              );
            })}
          </ul>
        </section>
      ))}
    </>
  );
}
