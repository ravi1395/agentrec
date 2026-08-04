#!/usr/bin/env node
// Thin launcher: execs the vendored prebuilt binary, downloading it first if
// postinstall was skipped (e.g. --ignore-scripts). All arguments, stdio, and
// the exit code pass through untouched.
"use strict";

const { spawnSync } = require("child_process");
const fs = require("fs");
const path = require("path");

const bin = path.join(__dirname, "..", "vendor", "agentrec");

function run() {
  const res = spawnSync(bin, process.argv.slice(2), { stdio: "inherit" });
  if (res.error) {
    console.error(`agentrec: failed to launch binary: ${res.error.message}`);
    process.exit(1);
  }
  process.exit(res.status === null ? 1 : res.status);
}

if (fs.existsSync(bin)) {
  run();
} else {
  // postinstall was skipped; fetch on first use.
  require("../install.js")
    .main()
    .then(run)
    .catch((err) => {
      console.error(`agentrec: binary missing and download failed: ${err.message}`);
      console.error("retry with: node " + path.join(__dirname, "..", "install.js"));
      process.exit(1);
    });
}
