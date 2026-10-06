import { readFileSync } from "node:fs";
import { spawnSync } from "node:child_process";
import { resolve } from "node:path";
import { describe, expect, it } from "vitest";

const workflow = readFileSync(
  resolve(process.cwd(), ".github/workflows/release.yml"),
  "utf8",
);
const start = workflow.indexOf("          # Build the argument vector");
const end = workflow.indexOf('\n          rm -rf "$DMG_STAGE_DIR"', start);
if (start < 0 || end < start) {
  throw new Error("Release DMG argument block not found");
}
const invocation = workflow.slice(start, end).replace(/^ {10}/gm, "");

describe("macOS release DMG arguments", () => {
  it.each([undefined, "", "Developer ID Application: Fork Test (ABC123)"])(
    "works under nounset with signing identity %s",
    (identity) => {
      const env: NodeJS.ProcessEnv = {
        ...process.env,
        NEW_DMG: "CC-Switch-fixture-macOS.dmg",
        DMG_STAGE_DIR: "/tmp/cc switch packaging fixture",
      };
      delete env.APPLE_SIGNING_IDENTITY;
      delete env.BASH_ENV;
      if (identity !== undefined) env.APPLE_SIGNING_IDENTITY = identity;

      // Run the actual workflow argument block, not a copy of its implementation.
      // create-dmg is a capture-only stub: no files, builds, signing or downloads.
      // /bin/bash is Apple's Bash 3.2 on macOS, exercising the original failure.
      const result = spawnSync(
        process.platform === "win32" ? "bash" : "/bin/bash",
        [
          "-c",
          'set -euo pipefail\ncreate-dmg() { printf "%s\\0" "$@"; }\n' +
            invocation,
        ],
        { env, encoding: "utf8", timeout: 10000 },
      );
      expect(result.error).toBeUndefined();
      expect(result.status, result.stderr).toBe(0);
      expect(result.stdout.split("\0").slice(0, -1)).toEqual([
        "--volname",
        "CC Switch",
        "--background",
        "src-tauri/icons/dmg-background.png",
        "--window-size",
        "660",
        "400",
        "--window-pos",
        "200",
        "120",
        "--icon-size",
        "80",
        "--icon",
        "CC Switch.app",
        "180",
        "220",
        "--hide-extension",
        "CC Switch.app",
        "--app-drop-link",
        "480",
        "220",
        ...(identity ? ["--codesign", identity] : []),
        "--no-internet-enable",
        "release-assets/CC-Switch-fixture-macOS.dmg",
        env.DMG_STAGE_DIR,
      ]);
    },
  );
});
