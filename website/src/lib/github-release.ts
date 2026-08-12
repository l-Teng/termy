import { gitConfig } from './shared';

const GITHUB_API = 'https://api.github.com';

export interface GitHubReleaseAsset {
  id: number;
  name: string;
  size: number;
  downloadUrl: string;
  contentType: string;
}

export interface GitHubRelease {
  id: number;
  name: string;
  tagName: string;
  publishedAt: string;
  prerelease: boolean;
  htmlUrl: string;
  body: string | null;
  tarballUrl: string;
  zipballUrl: string;
  assets: GitHubReleaseAsset[];
}

interface GitHubReleaseResponse {
  id: number;
  name: string | null;
  tag_name: string;
  published_at: string;
  prerelease: boolean;
  html_url: string;
  body: string | null;
  tarball_url: string;
  zipball_url: string;
  assets: Array<{
    id: number;
    name: string;
    size: number;
    browser_download_url: string;
    content_type: string;
  }>;
}

function githubHeaders(): Record<string, string> {
  const headers: Record<string, string> = {
    Accept: 'application/vnd.github+json',
    'User-Agent': `${gitConfig.repo}-website`,
  };
  if (process.env.GITHUB_TOKEN) {
    headers.Authorization = `Bearer ${process.env.GITHUB_TOKEN}`;
  }
  return headers;
}

function mapGitHubRelease(data: GitHubReleaseResponse): GitHubRelease {
  return {
    id: data.id,
    name: data.name || data.tag_name,
    tagName: data.tag_name,
    publishedAt: data.published_at,
    prerelease: data.prerelease,
    htmlUrl: data.html_url,
    body: data.body,
    tarballUrl: data.tarball_url,
    zipballUrl: data.zipball_url,
    assets: data.assets.map((asset) => ({
      id: asset.id,
      name: asset.name,
      size: asset.size,
      downloadUrl: asset.browser_download_url,
      contentType: asset.content_type,
    })),
  };
}

function isDesktopReleaseTag(tag: string): boolean {
  return /^v\d/.test(tag);
}

export interface PlatformAssetGroup {
  id: 'macos' | 'linux' | 'windows';
  title: string;
  assets: GitHubReleaseAsset[];
}

function assetPlatform(
  name: string,
): 'macos' | 'linux' | 'windows' | 'other' {
  const lower = name.toLowerCase();
  if (lower.includes('mac') || lower.includes('darwin') || lower.endsWith('.dmg'))
    return 'macos';
  if (
    lower.includes('linux') ||
    lower.includes('appimage') ||
    lower.endsWith('.deb') ||
    lower.endsWith('.rpm')
  )
    return 'linux';
  if (
    lower.includes('windows') ||
    lower.includes('win') ||
    lower.endsWith('.msi') ||
    lower.endsWith('.exe')
  )
    return 'windows';
  return 'other';
}

/** Sidecar files attached to releases; not installers. */
function isInstallableReleaseAsset(name: string): boolean {
  const lower = name.toLowerCase();
  if (lower === 'checksums.txt') return false;
  return !(
    lower.endsWith('.sha256') ||
    lower.endsWith('.metadata') ||
    lower.endsWith('.json') ||
    lower.endsWith('.log')
  );
}

export function groupReleaseAssets(
  assets: GitHubReleaseAsset[],
): PlatformAssetGroup[] {
  const groups: PlatformAssetGroup[] = [
    { id: 'macos', title: 'macOS', assets: [] },
    { id: 'linux', title: 'Linux', assets: [] },
    { id: 'windows', title: 'Windows', assets: [] },
  ];

  for (const asset of assets) {
    if (!isInstallableReleaseAsset(asset.name)) continue;
    const platform = assetPlatform(asset.name);
    if (platform === 'other') continue;
    groups.find((group) => group.id === platform)?.assets.push(asset);
  }

  return groups.filter((group) => group.assets.length > 0);
}

export function assetArch(name: string): string | null {
  const lower = name.toLowerCase();
  if (lower.includes('aarch64') || lower.includes('arm64')) return 'arm64';
  if (lower.includes('x86_64') || lower.includes('amd64') || lower.includes('x64'))
    return 'x64';
  return null;
}

/** Short human label for a release asset (Download page rows). */
export function assetLabel(name: string): string {
  const lower = name.toLowerCase();
  const arch = assetArch(name);
  const platform = assetPlatform(name);

  if (platform === 'macos') {
    if (arch === 'arm64') return 'Apple Silicon';
    if (arch === 'x64') return 'Intel';
    return 'macOS';
  }

  if (platform === 'linux') {
    if (lower.includes('appimage')) return 'AppImage';
    if (lower.endsWith('.tar.gz') || lower.endsWith('.tgz')) return 'Tarball';
    if (lower.endsWith('.deb')) return 'Debian';
    if (lower.endsWith('.rpm')) return 'RPM';
    return 'Linux';
  }

  if (platform === 'windows') {
    if (lower.endsWith('.msi')) return 'MSI';
    if (lower.includes('setup') || lower.endsWith('.exe')) return 'Installer';
    return 'Windows';
  }

  return name;
}

export function formatBytes(bytes: number): string {
  if (bytes < 1024) return `${bytes} B`;
  let size = bytes / 1024;
  let unit = 'KB';
  if (size >= 1024) {
    size /= 1024;
    unit = 'MB';
  }
  if (size >= 1024) {
    size /= 1024;
    unit = 'GB';
  }
  return `${size.toFixed(size >= 10 ? 0 : 1)} ${unit}`;
}

export function formatReleaseDate(iso: string): string {
  return new Date(iso).toLocaleDateString('en-US', {
    year: 'numeric',
    month: 'short',
    day: 'numeric',
  });
}

export function formatReleaseDay(iso: string): string {
  return new Date(iso).toLocaleDateString('en-US', {
    month: 'short',
    day: 'numeric',
  });
}

export interface GitHubReleaseYearGroup {
  year: string;
  releases: GitHubRelease[];
}

let releasesRequest: Promise<GitHubRelease[]> | undefined;

export function groupGitHubReleasesByYear(
  releases: GitHubRelease[],
): GitHubReleaseYearGroup[] {
  const groups: GitHubReleaseYearGroup[] = [];
  for (const release of releases) {
    const year = String(new Date(release.publishedAt).getFullYear());
    const last = groups[groups.length - 1];
    if (last?.year === year) last.releases.push(release);
    else groups.push({ year, releases: [release] });
  }
  return groups;
}

export async function fetchGitHubReleases(): Promise<GitHubRelease[]> {
  releasesRequest ??= (async () => {
    const res = await fetch(
      `${GITHUB_API}/repos/${gitConfig.user}/${gitConfig.repo}/releases?per_page=100`,
      { headers: githubHeaders() },
    );
    if (!res.ok) {
      throw new Error(`GitHub API ${res.status}: ${await res.text()}`);
    }
    const data = (await res.json()) as GitHubReleaseResponse[];
    return data
      .filter((release) => isDesktopReleaseTag(release.tag_name))
      .map(mapGitHubRelease);
  })().catch((error) => {
    releasesRequest = undefined;
    throw error;
  });
  return releasesRequest;
}

export async function fetchGitHubReleaseByTag(
  tag: string,
): Promise<GitHubRelease | null> {
  if (!isDesktopReleaseTag(tag)) return null;

  // TanStack's prerenderer crawls the entire release archive concurrently. Reuse
  // the list response (which already contains complete bodies and assets) so a
  // production build consumes one GitHub request instead of one per release.
  if (process.env.TSS_PRERENDERING === 'true') {
    return (
      (await fetchGitHubReleases()).find((release) => release.tagName === tag) ??
      null
    );
  }

  const res = await fetch(
    `${GITHUB_API}/repos/${gitConfig.user}/${gitConfig.repo}/releases/tags/${encodeURIComponent(tag)}`,
    { headers: githubHeaders() },
  );
  if (res.status === 404) return null;
  if (!res.ok) {
    throw new Error(`GitHub API ${res.status}: ${await res.text()}`);
  }
  return mapGitHubRelease((await res.json()) as GitHubReleaseResponse);
}

export async function fetchLatestGitHubRelease(): Promise<GitHubRelease> {
  const res = await fetch(
    `${GITHUB_API}/repos/${gitConfig.user}/${gitConfig.repo}/releases/latest`,
    {
      headers: githubHeaders(),
    },
  );

  if (!res.ok) {
    throw new Error(`GitHub API ${res.status}: ${await res.text()}`);
  }

  const data = (await res.json()) as GitHubReleaseResponse;

  return mapGitHubRelease(data);
}
