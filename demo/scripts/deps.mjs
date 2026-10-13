#!/usr/bin/env node
// Switch the demo's RIGA dependencies between the in-repo sources ("local") and
// the published npmjs/crates.io releases ("published"), for parity testing.
//
//   node scripts/deps.mjs status
//   node scripts/deps.mjs local       # default: file: / path dependencies
//   node scripts/deps.mjs published   # @rigai/assistant-ui + riga-* from registries
//
// The versions used for "published" are read from the workspace
// (`Cargo.toml` `[workspace.package].version`) and
// `packages/assistant-ui/package.json`, so the switch always targets the release
// that matches the current source tree.
import { readFileSync, writeFileSync } from "node:fs";
import { dirname, join, resolve } from "node:path";
import { fileURLToPath } from "node:url";

// demo/scripts/deps.mjs -> demo/ -> repository root.
const here = dirname(fileURLToPath(import.meta.url));
const demoDir = resolve(here, "..");
const repoDir = resolve(demoDir, "..");

const NPM_PACKAGE = "@rigai/assistant-ui";
const NPM_LOCAL_SPEC = "file:../packages/assistant-ui";
const RUST_CRATES = ["riga-kernel", "riga-server", "riga-shell"];

const demoPackageJson = join(demoDir, "package.json");
const demoCargoToml = join(demoDir, "src-tauri", "Cargo.toml");

function publishedRustVersion() {
  const manifest = readFileSync(join(repoDir, "Cargo.toml"), "utf8");
  const match = manifest.match(/^\[workspace\.package\][\s\S]*?^version\s*=\s*"([^"]+)"/m);
  if (!match) throw new Error("could not read [workspace.package] version from Cargo.toml");
  return match[1];
}

function publishedNpmVersion() {
  const manifest = readFileSync(join(repoDir, "packages", "assistant-ui", "package.json"), "utf8");
  return JSON.parse(manifest).version;
}

function currentMode() {
  const pkg = JSON.parse(readFileSync(demoPackageJson, "utf8"));
  const spec = pkg.dependencies?.[NPM_PACKAGE] ?? "";
  return spec.startsWith("file:") ? "local" : "published";
}

function writeNpm(mode) {
  const pkg = JSON.parse(readFileSync(demoPackageJson, "utf8"));
  if (!pkg.dependencies?.[NPM_PACKAGE]) {
    throw new Error(`${NPM_PACKAGE} is missing from demo/package.json`);
  }
  pkg.dependencies[NPM_PACKAGE] = mode === "published" ? `^${publishedNpmVersion()}` : NPM_LOCAL_SPEC;
  writeFileSync(demoPackageJson, `${JSON.stringify(pkg, null, 2)}\n`);
}

function writeRust(mode) {
  const version = publishedRustVersion();
  let manifest = readFileSync(demoCargoToml, "utf8");
  for (const name of RUST_CRATES) {
    const line = new RegExp(`^${name} = .*$`, "m");
    if (!line.test(manifest)) {
      throw new Error(`dependency ${name} is missing from demo/src-tauri/Cargo.toml`);
    }
    const spec = mode === "published" ? `"${version}"` : `{ path = "../../crates/${name}" }`;
    manifest = manifest.replace(line, `${name} = ${spec}`);
  }
  writeFileSync(demoCargoToml, manifest);
}

const mode = process.argv[2];

if (mode === "status") {
  console.log(`demo dependencies: ${currentMode()}`);
  process.exit(0);
}

if (mode !== "local" && mode !== "published") {
  console.error("usage: node scripts/deps.mjs <local|published|status>");
  process.exit(2);
}

writeNpm(mode);
writeRust(mode);

console.log(`Switched demo dependencies to "${mode}".`);
if (mode === "published") {
  console.log(`  npm:    ${NPM_PACKAGE}@^${publishedNpmVersion()} (npmjs)`);
  console.log(`  cargo:  ${RUST_CRATES.join(", ")} ${publishedRustVersion()} (crates.io)`);
  console.log("Reinstall to resolve the registry packages:");
  console.log("  npm install --prefix demo");
  console.log("  cargo update --manifest-path demo/src-tauri/Cargo.toml");
} else {
  console.log("Reinstall to relink the local package:");
  console.log("  npm install --prefix demo");
}
