#!/usr/bin/env node
// Stamp the release tag into Tauri's embedded version before compilation AND
// bundling. Keep platform installer versions numeric where the OS requires it.
import { readFileSync, writeFileSync } from "node:fs";
import { resolve } from "node:path";
import { pathToFileURL } from "node:url";

export function prepareReleaseVersion(root, refType, refName) {
  if (refType !== "tag") return null;
  const match = /^v(\d+)\.(\d+)\.(\d+)(?:-codex-auth-([1-9]\d*))?$/.exec(
    refName ?? "",
  );
  if (!match) throw new Error(`Unsupported release tag: ${refName}`);

  const [, major, minor, patch, revision] = match;
  const base = `${major}.${minor}.${patch}`;
  const version = refName.slice(1);
  const packageJson = JSON.parse(
    readFileSync(resolve(root, "package.json"), "utf8"),
  );
  const cargo = readFileSync(resolve(root, "src-tauri/Cargo.toml"), "utf8");
  const cargoVersion = cargo.match(/^version\s*=\s*"([^"]+)"/m)?.[1];
  const configPath = resolve(root, "src-tauri/tauri.conf.json");
  const config = JSON.parse(readFileSync(configPath, "utf8"));
  if (
    packageJson.version !== base ||
    cargoVersion !== base ||
    ![base, version].includes(config.version)
  ) {
    throw new Error(
      `Release ${refName} does not match package, Cargo and Tauri versions`,
    );
  }
  if (
    [major, minor].some((value) => Number(value) > 255) ||
    Number(patch) > 65535 ||
    (revision && Number(revision) > 65535)
  ) {
    throw new Error("Release version exceeds Windows installer version limits");
  }

  config.version = version;
  if (revision) {
    config.bundle ??= {};
    config.bundle.windows ??= {};
    config.bundle.windows.wix ??= {};
    config.bundle.windows.wix.version = `${base}.${revision}`;
    config.bundle.macOS ??= {};
    config.bundle.macOS.bundleVersion = `${major}.${minor}.${revision}`;
  }
  writeFileSync(configPath, `${JSON.stringify(config, null, 2)}\n`);
  return version;
}

if (
  process.argv[1] &&
  import.meta.url === pathToFileURL(resolve(process.argv[1])).href
) {
  const version = prepareReleaseVersion(
    process.cwd(),
    process.env.GITHUB_REF_TYPE,
    process.env.GITHUB_REF_NAME,
  );
  console.log(
    version
      ? `Embedded release version: ${version}`
      : "Branch build: keeping the development version",
  );
}
