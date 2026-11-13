import { describe, expect, it } from "vitest";
import { parseSkillRepositoryUrl, skillRepositoryKey, skillRepositoryUrl } from "@/lib/skillRepository";
import { replaceRepoDiscovery } from "@/hooks/useSkills.helpers";
import type { DiscoverableSkill } from "@/lib/api/skills";

describe("Skill repository sources", () => {
  it("preserves GitHub shorthand and optional-field compatibility", () => {
    expect(parseSkillRepositoryUrl("owner/repo")).toEqual({ owner: "owner", name: "repo" });
    expect(parseSkillRepositoryUrl("https://github.com/owner/repo.git/")).toEqual({ owner: "owner", name: "repo" });
    expect(skillRepositoryUrl({ owner: "owner", name: "repo" })).toBe("https://github.com/owner/repo");
    expect(skillRepositoryKey({ owner: "owner", name: "repo" })).toBe("owner/repo");
  });

  it.each(["https://codeberg.org/owner/repo", "https://git.example:8443/forge/owner/repo"])("accepts Forgejo root %s", (url) => {
    expect(parseSkillRepositoryUrl(url)).toEqual({ owner: "owner", name: "repo", repoUrl: url });
  });

  it.each([
    "http://codeberg.org/owner/repo", "https://user:password@codeberg.org/owner/repo",
    "https://codeberg.org/owner/repo?token=example", "https://codeberg.org/owner/repo#main",
    "https://codeberg.org/a/../owner/repo", "https://codeberg.org/owner%2Frepo",
    "https://github.com/owner/repo/tree/main", "https://codeberg.org/owner/repo\\suffix",
  ])("rejects non-root or credential-bearing URL %s", (url) => {
    expect(parseSkillRepositoryUrl(url)).toBeNull();
  });

  it("repository cache mutations cannot remove another host's same-name repository", () => {
    const row = (key: string, repoUrl?: string): DiscoverableSkill => ({
      key, name: key, description: "", directory: "a", repoOwner: "owner",
      repoName: "repo", repoBranch: "main", ...(repoUrl ? { repoUrl } : {}),
    });
    const codeberg = "https://codeberg.org/owner/repo";
    const selfHosted = "https://git.example/owner/repo";
    const existing = {
      skills: [row("github"), row("codeberg", codeberg), row("self-hosted", selfHosted)],
      failures: [
        { owner: "owner", name: "repo", branch: "main", error: "github" },
        { owner: "owner", name: "repo", branch: "main", error: "codeberg", repoUrl: codeberg },
      ],
    };
    const result = replaceRepoDiscovery(existing, { owner: "owner", name: "repo", repoUrl: codeberg });
    expect(result?.skills.map((skill) => skill.key)).toEqual(["github", "self-hosted"]);
    expect(result?.failures.map((failure) => failure.error)).toEqual(["github"]);
  });
});
