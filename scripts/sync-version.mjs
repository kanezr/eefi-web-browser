import { readFile, writeFile } from "node:fs/promises";
import path from "node:path";
import { fileURLToPath } from "node:url";
import { normalizeVersion } from "./version.mjs";

const root = path.resolve(path.dirname(fileURLToPath(import.meta.url)), "..");
const version = normalizeVersion(await readFile(path.join(root, "VERSION"), "utf8"));

async function readJson(relativePath) {
  const content = await readFile(path.join(root, relativePath), "utf8");
  return { content, value: JSON.parse(content) };
}

function serializeJson(content, value) {
  const newline = content.includes("\r\n") ? "\r\n" : "\n";
  return `${JSON.stringify(value, null, 2).replace(/\n/g, newline)}${newline}`;
}

function setPackageVersion(content, relativePath) {
  const start = content.indexOf("[package]");
  if (start < 0) throw new Error(`${relativePath} has no [package] section.`);
  const nextSection = content.indexOf("\n[", start + "[package]".length);
  const end = nextSection < 0 ? content.length : nextSection + 1;
  const section = content.slice(start, end);
  const pattern = /^version = "[^"]+"(?=\r?$)/m;
  if (!pattern.test(section)) {
    throw new Error(`${relativePath} has no version field in [package].`);
  }
  return `${content.slice(0, start)}${section.replace(pattern, `version = "${version}"`)}${content.slice(end)}`;
}

function setCargoLockVersion(content) {
  const pattern =
    /(\[\[package\]\]\r?\nname = "eefi-browser"\r?\nversion = ")[^"]+(")/g;
  const matches = [...content.matchAll(pattern)];
  if (matches.length !== 1) {
    throw new Error("Cargo.lock must contain exactly one eefi-browser package entry.");
  }
  return content.replace(pattern, `$1${version}$2`);
}

async function writeIfChanged(relativePath, original, updated) {
  if (original === updated) return;
  await writeFile(path.join(root, relativePath), updated, "utf8");
  console.log(`Synchronized ${relativePath} to ${version}.`);
}

const updates = [];

const packageJson = await readJson("package.json");
packageJson.value.version = version;
updates.push([
  "package.json",
  packageJson.content,
  serializeJson(packageJson.content, packageJson.value),
]);

const packageLock = await readJson("package-lock.json");
if (!packageLock.value.packages?.[""]) {
  throw new Error("package-lock.json is missing its root package entry.");
}
packageLock.value.version = version;
packageLock.value.packages[""].version = version;
updates.push([
  "package-lock.json",
  packageLock.content,
  serializeJson(packageLock.content, packageLock.value),
]);

const cargoManifestPath = path.join(root, "src-tauri", "Cargo.toml");
const cargoManifest = await readFile(cargoManifestPath, "utf8");
updates.push([
  path.join("src-tauri", "Cargo.toml"),
  cargoManifest,
  setPackageVersion(cargoManifest, "src-tauri/Cargo.toml"),
]);

const cargoLockPath = path.join(root, "src-tauri", "Cargo.lock");
const cargoLock = await readFile(cargoLockPath, "utf8");
updates.push([
  path.join("src-tauri", "Cargo.lock"),
  cargoLock,
  setCargoLockVersion(cargoLock),
]);

const tauriConfig = await readJson(path.join("src-tauri", "tauri.conf.json"));
tauriConfig.value.version = version;
updates.push([
  path.join("src-tauri", "tauri.conf.json"),
  tauriConfig.content,
  serializeJson(tauriConfig.content, tauriConfig.value),
]);

for (const [relativePath, original, updated] of updates) {
  await writeIfChanged(relativePath, original, updated);
}
