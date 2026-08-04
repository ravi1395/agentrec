// Downloads the prebuilt agentrec binary for this platform from the GitHub
// release matching package.json's version, verifies its sha256 against the
// release's published checksum file, and stages it under vendor/.
//
// No dependencies; requires Node >= 18 (global fetch). Same trust chain as
// install.sh: binary + checksum both come from the tagged GitHub release.
"use strict";

const fs = require("fs");
const path = require("path");
const crypto = require("crypto");

const { version } = require("./package.json");
const REPO = "ravi1395/agentrec";

function assetName() {
  const os = { darwin: "darwin", linux: "linux" }[process.platform];
  const cpu = { arm64: "arm64", x64: "x86_64" }[process.arch];
  if (!os || !cpu) {
    throw new Error(
      `unsupported platform ${process.platform}/${process.arch} — ` +
        "agentrec ships darwin/linux arm64/x86_64 binaries. " +
        "Build from source: cargo install agentrec"
    );
  }
  return `agentrec-${os}-${cpu}`;
}

async function fetchOk(url) {
  const res = await fetch(url, { redirect: "follow" });
  if (!res.ok) {
    throw new Error(`download failed: ${res.status} ${res.statusText} for ${url}`);
  }
  return res;
}

async function main() {
  const asset = assetName();
  const base = `https://github.com/${REPO}/releases/download/v${version}`;

  const shaText = await (await fetchOk(`${base}/${asset}.sha256`)).text();
  const expected = shaText.trim().split(/\s+/)[0];
  if (!/^[0-9a-f]{64}$/.test(expected)) {
    throw new Error(`malformed checksum file for ${asset}: "${shaText.trim()}"`);
  }

  const buf = Buffer.from(await (await fetchOk(`${base}/${asset}`)).arrayBuffer());
  const actual = crypto.createHash("sha256").update(buf).digest("hex");
  if (actual !== expected) {
    throw new Error(
      `checksum mismatch for ${asset}: expected ${expected}, got ${actual} — refusing to install`
    );
  }

  const vendorDir = path.join(__dirname, "vendor");
  fs.mkdirSync(vendorDir, { recursive: true });
  const dest = path.join(vendorDir, "agentrec");
  const tmp = dest + ".tmp";
  fs.writeFileSync(tmp, buf, { mode: 0o755 });
  fs.renameSync(tmp, dest);
  console.log(`agentrec ${version} installed (${asset}, checksum verified: ${expected})`);
}

module.exports = { main };

if (require.main === module) {
  main().catch((err) => {
    console.error(`agentrec install error: ${err.message}`);
    process.exit(1);
  });
}
