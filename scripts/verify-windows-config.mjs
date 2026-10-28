#!/usr/bin/env node
// Verifies the Windows packaging configuration without a Windows host.
//
// This is the macOS/CI-runnable seam for ticket 18: it checks that the
// platform config, the native-library setup, the build script and the install
// notes agree with each other. It cannot prove that MSI/NSIS installers run,
// that WebView2 installs offline, or that SmartScreen behaves as documented;
// those require a real Windows 10 1809+/11 x64 machine or VM.
//
// Usage: node scripts/verify-windows-config.mjs

import { existsSync, readFileSync } from "node:fs";
import path from "node:path";
import { fileURLToPath } from "node:url";

const REPO_ROOT = path.resolve(
  path.dirname(fileURLToPath(import.meta.url)),
  "..",
);

const failures = [];
let checks = 0;

function check(name, condition, detail = "") {
  checks += 1;
  if (condition) {
    console.log(`ok   ${name}`);
    return;
  }
  const message = detail ? `${name} (${detail})` : name;
  failures.push(message);
  console.log(`FAIL ${message}`);
}

function readIfExists(relativePath) {
  const absolute = path.join(REPO_ROOT, relativePath);
  return existsSync(absolute) ? readFileSync(absolute, "utf8") : null;
}

function parseJson(relativePath) {
  const text = readIfExists(relativePath);
  if (text === null) {
    check(`exists ${relativePath}`, false, "file is missing");
    return null;
  }
  try {
    return JSON.parse(text);
  } catch (error) {
    check(`parses ${relativePath}`, false, error.message);
    return null;
  }
}

// --- required files --------------------------------------------------------
for (const relativePath of [
  "src-tauri/tauri.windows.conf.json",
  "scripts/build-windows.ps1",
  "docs/install-windows.md",
]) {
  check(`exists ${relativePath}`, existsSync(path.join(REPO_ROOT, relativePath)));
}

// --- product identity ------------------------------------------------------
const baseConfig = parseJson("src-tauri/tauri.conf.json");
const windowsConfig = parseJson("src-tauri/tauri.windows.conf.json");

const productName = baseConfig?.productName;
const version = baseConfig?.version;
check("base product name is PDFX", productName === "PDFX", String(productName));

const cargoToml = readIfExists("src-tauri/Cargo.toml") ?? "";
const cargoVersion = cargoToml.match(/^version\s*=\s*"([^"]+)"/m)?.[1];
check(
  "version matches src-tauri/Cargo.toml",
  Boolean(version) && version === cargoVersion,
  `${version} != ${cargoVersion}`,
);

const packageJson = parseJson("package.json");
check(
  "version matches package.json",
  Boolean(version) && version === packageJson?.version,
  `${version} != ${packageJson?.version}`,
);

const icons = baseConfig?.bundle?.icon ?? [];
check(
  "Windows .ico icon is bundled",
  icons.includes("icons/icon.ico") &&
    existsSync(path.join(REPO_ROOT, "src-tauri/icons/icon.ico")),
);

// --- PDFium resource mapping ----------------------------------------------
const resources = windowsConfig?.bundle?.resources ?? {};
check(
  "PDFium DLL maps vendor/pdfium/bin/pdfium.dll -> pdfium.dll",
  resources["vendor/pdfium/bin/pdfium.dll"] === "pdfium.dll",
  JSON.stringify(resources["vendor/pdfium/bin/pdfium.dll"]),
);
check(
  "PDFium license files are bundled",
  resources["vendor/pdfium/LICENSE"] === "pdfium-licenses/LICENSE" &&
    resources["vendor/pdfium/licenses/"] === "pdfium-licenses/",
);
check(
  "install notes are bundled as INSTALL-windows.md",
  resources["../docs/install-windows.md"] === "INSTALL-windows.md",
);

const setupNative = readIfExists("scripts/setup-native.sh") ?? "";
check(
  "setup-native.sh extracts Windows PDFium to bin/pdfium.dll",
  /MINGW|MSYS|CYGWIN|Windows_NT/.test(setupNative) &&
    setupNative.includes('PDFIUM_LIB="bin/pdfium.dll"'),
);
check(
  "setup-native.sh downloads the Windows x64 asset",
  setupNative.includes("pdfium-win-x64.tgz"),
);

const engineRs = readIfExists("src-tauri/src/engine.rs") ?? "";
check(
  "Rust engine loads pdfium.dll from resource_dir()",
  engineRs.includes("resource_dir()") &&
    /target_os = "windows"/.test(engineRs) &&
    engineRs.includes('"pdfium.dll"'),
);

// --- file association ------------------------------------------------------
const associations =
  windowsConfig?.bundle?.fileAssociations ??
  baseConfig?.bundle?.fileAssociations ??
  [];
const pdfAssociation = associations.find((association) =>
  (association.ext ?? [])
    .map((extension) => extension.toLowerCase())
    .includes("pdf"),
);
check("Windows registers the .pdf file association", Boolean(pdfAssociation));
check(
  "file association uses application/pdf and the Editor role",
  pdfAssociation?.mimeType === "application/pdf" &&
    pdfAssociation?.role === "Editor",
);

// --- WebView2 install mode -------------------------------------------------
// Tauri merges platform config over the base config. `embedBootstrapper` embeds
// a network-dependent bootstrapper; only `offlineInstaller` installs without a
// connection, which is the ticket's requirement.
const effectiveWebviewInstallMode = {
  ...(baseConfig?.bundle?.windows?.webviewInstallMode ?? {}),
  ...(windowsConfig?.bundle?.windows?.webviewInstallMode ?? {}),
};
check(
  "effective WebView2 mode installs offline (offlineInstaller)",
  effectiveWebviewInstallMode.type === "offlineInstaller",
  String(effectiveWebviewInstallMode.type),
);
check(
  "WebView2 installer runs silently",
  effectiveWebviewInstallMode.silent === true,
);

// --- no signing, no updater ------------------------------------------------
const signingKeys = [
  "signCommand",
  "certificateThumbprint",
  "timestampUrl",
  "digestAlgorithm",
  "tsp",
];
const baseWindows = baseConfig?.bundle?.windows ?? {};
const platformWindows = windowsConfig?.bundle?.windows ?? {};
check(
  "Windows config contains no code-signing settings",
  !signingKeys.some(
    (key) => Object.hasOwn(baseWindows, key) || Object.hasOwn(platformWindows, key),
  ),
);
check(
  "no auto-updater is configured",
  !Object.hasOwn(baseConfig?.bundle ?? {}, "createUpdaterArtifacts") &&
    !Object.hasOwn(baseConfig?.plugins ?? {}, "updater"),
);

// --- build script ----------------------------------------------------------
const buildScript = readIfExists("scripts/build-windows.ps1") ?? "";
check(
  "build script runs npm run setup:native",
  /npm\s+run\s+setup:native/.test(buildScript),
);
check(
  "build script targets x86_64-pc-windows-msvc",
  /--target/.test(buildScript) &&
    buildScript.includes("x86_64-pc-windows-msvc"),
);
check(
  "build script bundles msi and nsis",
  /--bundles/.test(buildScript) && /msi,nsis/.test(buildScript),
);
check(
  "build script creates the portable zip",
  /Compress-Archive|CreateFromDirectory/.test(buildScript) &&
    buildScript.includes("-portable"),
);
check(
  "build script writes SHA-256 checksums",
  /Get-FileHash/.test(buildScript) &&
    buildScript.includes("SHA256SUMS-windows.txt"),
);
check(
  "build script packages PDFX.exe with pdfium.dll and install notes",
  ["PDFX.exe", "pdfium.dll", "INSTALL-windows.md"].every((entry) =>
    buildScript.includes(entry),
  ),
);
check(
  "build script contains no signing commands",
  !/signtool|Set-AuthenticodeSignature/i.test(buildScript),
);

// --- install notes ---------------------------------------------------------
const installNotes = readIfExists("docs/install-windows.md") ?? "";
for (const [name, test] of [
  ["install notes cover Windows 10 1809+", /Windows 10 1809/],
  ["install notes cover Windows 11", /Windows 11/],
  ["install notes cover x64", /x64/],
  ["install notes explain the SmartScreen bypass", /SmartScreen/],
  ["install notes say 更多信息 -> 仍要运行", /更多信息/],
  ["install notes say 仍要运行", /仍要运行/],
  ["install notes cover WebView2", /WebView2/],
  ["install notes cover offline installation", /离线/],
  ["install notes cover the portable zip", /portable/i],
  ["install notes explain file association", /文件关联|打开方式/],
  ["install notes explain the unsigned trade-off", /未签名/],
  ["install notes link ADR-0009", /0009-zero-cost-distribution/],
  ["install notes name the MSI installer", /\.msi/],
  ["install notes name the NSIS installer", /-setup\.exe/],
  ["install notes name the portable zip", /-portable\.zip/],
  ["install notes explain checksum verification", /Get-FileHash|SHA-?256/i],
]) {
  check(name, test.test(installNotes));
}

// --- summary ---------------------------------------------------------------
if (failures.length > 0) {
  console.error(`\n${failures.length} of ${checks} checks failed:`);
  for (const failure of failures) {
    console.error(`  - ${failure}`);
  }
  process.exit(1);
}

console.log(`\nAll ${checks} Windows packaging config checks passed.`);
console.log(
  "Not verifiable here: MSI/NSIS install, offline WebView2 install, " +
    "portable zip launch, file-association behavior, and SmartScreen on Windows.",
);
