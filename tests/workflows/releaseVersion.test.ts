import {
  readFileSync,
  writeFileSync,
  mkdirSync,
  mkdtempSync,
  rmSync,
} from "node:fs";
import { spawnSync } from "node:child_process";
import { tmpdir } from "node:os";
import { join, resolve } from "node:path";
import { afterEach, describe, expect, it } from "vitest";

const script = resolve("scripts/prepare-release-version.mjs");
const roots: string[] = [];

function fixture(version = "4.0.6") {
  const root = mkdtempSync(join(tmpdir(), "cc-switch-release-version-"));
  roots.push(root);
  mkdirSync(join(root, "src-tauri"));
  writeFileSync(join(root, "package.json"), JSON.stringify({ version }));
  writeFileSync(
    join(root, "src-tauri/Cargo.toml"),
    `[package]\nversion = "${version}"\n`,
  );
  const config = {
    version,
    bundle: {
      windows: { wix: { template: "wix/per-user-main.wxs" } },
      macOS: { minimumSystemVersion: "12.0" },
    },
    plugins: {
      updater: {
        pubkey: "keep-signature-key",
        endpoints: ["https://example.com/latest.json"],
      },
    },
  };
  const configPath = join(root, "src-tauri/tauri.conf.json");
  writeFileSync(configPath, JSON.stringify(config));
  return { root, configPath, config };
}

function stamp(root: string, tag: string, refType = "tag") {
  return spawnSync(process.execPath, [script], {
    cwd: root,
    env: { ...process.env, GITHUB_REF_TYPE: refType, GITHUB_REF_NAME: tag },
    encoding: "utf8",
    timeout: 10000,
  });
}

afterEach(() => {
  for (const root of roots.splice(0))
    rmSync(root, { recursive: true, force: true });
});

describe("release version stamping", () => {
  it("embeds the exact manifest version with numeric installer versions", () => {
    const { root, configPath, config } = fixture();
    for (let attempt = 0; attempt < 2; attempt++) {
      const result = stamp(root, "v4.0.6-codex-auth-75");
      expect(result.status, result.stderr).toBe(0);
      const stamped = JSON.parse(readFileSync(configPath, "utf8"));
      expect(stamped.version).toBe("4.0.6-codex-auth-75");
      expect(stamped.bundle.windows.wix).toEqual({
        template: config.bundle.windows.wix.template,
        version: "4.0.6.75",
      });
      expect(stamped.bundle.macOS).toEqual({
        minimumSystemVersion: "12.0",
        bundleVersion: "4.0.75",
      });
      expect(stamped.plugins).toEqual(config.plugins);
      expect(
        JSON.parse(readFileSync(join(root, "package.json"), "utf8")).version,
      ).toBe("4.0.6");
      expect(
        readFileSync(join(root, "src-tauri/Cargo.toml"), "utf8"),
      ).toContain('version = "4.0.6"');
    }
  });

  it("keeps branch builds and ordinary upstream tags unchanged", () => {
    const { root, configPath, config } = fixture();
    expect(stamp(root, "test/codex-chatgpt-auth-route", "branch").status).toBe(
      0,
    );
    expect(JSON.parse(readFileSync(configPath, "utf8"))).toEqual(config);
    expect(stamp(root, "v4.0.6").status).toBe(0);
    expect(JSON.parse(readFileSync(configPath, "utf8"))).toEqual(config);
  });

  it.each([
    "v4.0.5-codex-auth-75",
    "v4.0.6-codex-auth-075",
    "v4.0.6-codex-auth-65536",
    "v4.0.6-beta.1",
  ])(
    "rejects inconsistent or unsupported release %s before modifying files",
    (tag) => {
      const { root, configPath, config } = fixture();
      expect(stamp(root, tag).status).not.toBe(0);
      expect(JSON.parse(readFileSync(configPath, "utf8"))).toEqual(config);
    },
  );

  it("stamps every compilation and bundling job before Tauri runs", () => {
    const workflow = readFileSync(
      resolve(".github/workflows/release.yml"),
      "utf8",
    );
    const jobs = workflow
      .split(/^  (?=[\w-]+:\s*$)/m)
      .filter((part) => /pnpm tauri (build|bundle)/.test(part));
    expect(jobs).toHaveLength(3);
    for (const job of jobs) {
      const stampAt = job.indexOf("node scripts/prepare-release-version.mjs");
      expect(stampAt).toBeGreaterThan(0);
      expect(stampAt).toBeLessThan(job.search(/pnpm tauri (build|bundle)/));
    }
  });
});
