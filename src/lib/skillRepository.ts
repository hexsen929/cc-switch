export interface SkillRepositoryRef {
  owner: string;
  name: string;
  repoUrl?: string;
}

export const skillRepositoryUrl = (repo: SkillRepositoryRef) =>
  repo.repoUrl ?? `https://github.com/${repo.owner}/${repo.name}`;

/** Missing source means GitHub, preserving existing keys and persisted caches. */
export const skillRepositoryKey = (repo: SkillRepositoryRef) =>
  (repo.repoUrl ?? `${repo.owner}/${repo.name}`).toLowerCase();

export const skillRepositoryLabel = (repo: SkillRepositoryRef) =>
  repo.repoUrl?.replace(/^https:\/\//, "") ?? `${repo.owner}/${repo.name}`;

/** Only repository roots: never accept embedded credentials, refs or query URLs. */
export function parseSkillRepositoryUrl(
  value: string,
): SkillRepositoryRef | null {
  let raw = value
    .trim()
    .replace(/\/+$/, "")
    .replace(/\.git$/, "");
  if (!raw || /[\\%\x00-\x20\x7f]/.test(raw)) return null;
  if (!raw.includes("://")) raw = `https://github.com/${raw}`;
  if (raw.split("/").some((part) => part === "." || part === "..")) return null;
  try {
    const url = new URL(raw);
    if (
      url.protocol !== "https:" ||
      url.username ||
      url.password ||
      url.search ||
      url.hash
    )
      return null;
    const parts = url.pathname.slice(1).split("/");
    if (
      parts.length < 2 ||
      parts.some((part) => !/^[A-Za-z0-9_.-]+$/.test(part))
    )
      return null;
    const [owner, name] = parts.slice(-2);
    if (url.hostname === "github.com") {
      if (parts.length !== 2 || url.port || !/^[A-Za-z0-9-]{1,39}$/.test(owner))
        return null;
      return { owner, name };
    }
    return { owner, name, repoUrl: url.toString().replace(/\/$/, "") };
  } catch {
    return null;
  }
}
