import { readFileSync } from "node:fs";
import { resolve } from "node:path";
import { describe, expect, it } from "vitest";

const readRepoFile = (path: string) =>
  readFileSync(resolve(process.cwd(), path), "utf8");

const forkReleases = "https://github.com/hexsen929/cc-switch/releases";
const workflow = readRepoFile(".github/workflows/release.yml");
const publishStart = workflow.indexOf("      - name: Upload Release Assets");
if (publishStart < 0) {
  throw new Error("Release publication step not found");
}
const publication = workflow.slice(publishStart);

describe("fork updater release routing", () => {
  it("checks the fork's latest updater manifest", () => {
    const config = JSON.parse(readRepoFile("src-tauri/tauri.conf.json"));
    expect(config.plugins.updater.endpoints).toEqual([
      `${forkReleases}/latest/download/latest.json`,
    ]);
  });

  it("publishes a non-prerelease so GitHub's latest endpoint can find it", () => {
    expect(publication).toMatch(/^\s+prerelease:\s+false\s*$/m);
    expect(publication).toMatch(/^\s+make_latest:\s+true\s*$/m);
  });

  it("builds asset URLs from the repository running the release", () => {
    const start = workflow.indexOf("      - name: Generate latest.json");
    expect(start).toBeGreaterThanOrEqual(0);
    expect(start).toBeLessThan(publishStart);
    const manifest = workflow.slice(start, publishStart);
    expect(manifest).toContain("REPO: ${{ github.repository }}");
    expect(manifest).toContain("TAG: ${{ github.ref_name }}");
    expect(manifest).toContain(
      'base_url="https://github.com/$REPO/releases/download/$TAG"',
    );
    expect(manifest).toContain('mv "$tmp_json" release-assets/latest.json');
    expect(publication).toContain("files: release-assets/*");
  });

  it("keeps the manual update fallback on the fork", () => {
    const source = readRepoFile("src-tauri/src/commands/misc.rs");
    const body = source.match(
      /pub async fn check_for_updates\b[\s\S]*?\n\}/,
    )?.[0];
    expect(body).toBeDefined();
    expect(body).toContain(`${forkReleases}/latest`);
    expect(body).not.toContain("farion1231/cc-switch");
  });

  it("uses the same revision-aware comparator for UI checks and backend installs", () => {
    expect(readRepoFile("src-tauri/src/lib.rs")).toContain(
      ".plugin(app_updater::builder().build())",
    );
    expect(readRepoFile("src-tauri/src/app_updater.rs")).toContain(
      ".default_version_comparator(",
    );
    const backend = readRepoFile("src-tauri/src/commands/settings.rs");
    for (const command of [
      "install_update_and_restart",
      "check_app_update_available",
    ]) {
      const body = backend.match(
        new RegExp(`pub async fn ${command}\\b[\\s\\S]*?\\n\\}`),
      )?.[0];
      expect(body).toContain(".updater_builder()");
      expect(body).not.toContain(".version_comparator(");
    }
    expect(readRepoFile("src/lib/updater.ts")).not.toContain("allowDowngrades");
  });
});
