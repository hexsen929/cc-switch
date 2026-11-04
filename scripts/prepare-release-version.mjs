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

// RPM disallows '-' in Version. Override packaging metadata only, after the
// binary has been built with the full fork version used by the updater.
export function prepareRpmBundleConfig(root, refType, refName, outputPath) {
  prepareReleaseVersion(root, refType, refName);
  const config = JSON.parse(
    readFileSync(resolve(root, "src-tauri/tauri.conf.json"), "utf8"),
  );
  const match = /^(\d+\.\d+\.\d+)-codex-auth-([1-9]\d*)$/.exec(config.version);
  const override = match
    ? {
        version: match[1],
        bundle: { linux: { rpm: { release: match[2] } } },
      }
    : {};
  writeFileSync(outputPath, `${JSON.stringify(override, null, 2)}\n`);
  return override;
}

if (
  process.argv[1] &&
  import.meta.url === pathToFileURL(resolve(process.argv[1])).href
) {
  if (process.argv[2] === "--rpm-config") {
    if (!process.argv[3]) throw new Error("Missing RPM config output path");
    const override = prepareRpmBundleConfig(
      process.cwd(),
      process.env.GITHUB_REF_TYPE,
      process.env.GITHUB_REF_NAME,
      process.argv[3],
    );
    console.log(`RPM packaging override: ${JSON.stringify(override)}`);
  } else {
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
}
