import assert from "node:assert/strict";
import { spawn, spawnSync } from "node:child_process";
import {
  chmodSync,
  copyFileSync,
  cpSync,
  mkdirSync,
  mkdtempSync,
  rmSync,
  writeFileSync,
} from "node:fs";
import { tmpdir } from "node:os";
import { dirname, join, resolve } from "node:path";
import { after, test } from "node:test";
import { fileURLToPath } from "node:url";

const packageRoot = resolve(dirname(fileURLToPath(import.meta.url)), "..");
const tempDirs = [];

const platformPackageNames = {
  "darwin-arm64": "@aaif/goose-binary-darwin-arm64",
  "darwin-x64": "@aaif/goose-binary-darwin-x64",
  "linux-arm64": "@aaif/goose-binary-linux-arm64",
  "linux-x64": "@aaif/goose-binary-linux-x64",
  "win32-x64": "@aaif/goose-binary-win32-x64",
};

function createLauncherHarness() {
  const root = mkdtempSync(join(tmpdir(), "goose-acp-launcher-"));
  tempDirs.push(root);

  const scopeDir = join(root, "node_modules", "@aaif");
  const wrapperDir = join(scopeDir, "goose-acp");
  mkdirSync(wrapperDir, { recursive: true });

  cpSync(join(packageRoot, "dist"), join(wrapperDir, "dist"), {
    recursive: true,
  });
  mkdirSync(join(wrapperDir, "bin"));
  copyFileSync(
    join(packageRoot, "bin", "goose.mjs"),
    join(wrapperDir, "bin", "goose.mjs"),
  );

  const platformKey = `${process.platform}-${process.arch}`;
  const packageName = platformPackageNames[platformKey];
  assert.ok(packageName, `test does not support ${platformKey}`);

  const platformPackageDir = join(scopeDir, packageName.slice("@aaif/".length));
  const platformBinDir = join(platformPackageDir, "bin");
  const executableName = process.platform === "win32" ? "goose.exe" : "goose";
  const executablePath = join(platformBinDir, executableName);
  mkdirSync(platformBinDir, { recursive: true });
  writeFileSync(
    join(platformPackageDir, "package.json"),
    JSON.stringify({ name: packageName, version: "0.20.2" }),
  );
  if (process.platform === "win32") {
    copyFileSync(process.execPath, executablePath);
  } else {
    writeFileSync(
      executablePath,
      '#!/bin/sh\nexec "$GOOSE_ACP_TEST_NODE" "$@"\n',
    );
  }
  chmodSync(executablePath, 0o755);

  return join(wrapperDir, "bin", "goose.mjs");
}

function runLauncher(args, input) {
  const launcherPath = createLauncherHarness();
  const result = spawnSync(process.execPath, [launcherPath, ...args], {
    encoding: "utf8",
    env: {
      ...process.env,
      GOOSE_BINARY: "",
      GOOSE_ACP_TEST_NODE: process.execPath,
    },
    input,
  });

  assert.equal(result.error, undefined);
  return result;
}

function waitForExit(child, timeoutMs = 5_000) {
  return new Promise((resolvePromise, reject) => {
    const timeout = setTimeout(() => {
      child.kill("SIGKILL");
      reject(new Error(`launcher did not exit within ${timeoutMs}ms`));
    }, timeoutMs);

    child.once("exit", (code, signal) => {
      clearTimeout(timeout);
      resolvePromise({ code, signal });
    });
  });
}

function waitForOutput(stream, expected, timeoutMs = 5_000) {
  return new Promise((resolvePromise, reject) => {
    let output = "";
    const timeout = setTimeout(() => {
      reject(new Error(`did not receive ${JSON.stringify(expected)}`));
    }, timeoutMs);

    stream.setEncoding("utf8");
    stream.on("data", (chunk) => {
      output += chunk;
      if (output.includes(expected)) {
        clearTimeout(timeout);
        resolvePromise(output);
      }
    });
  });
}

after(() => {
  for (const dir of tempDirs) {
    rmSync(dir, { recursive: true, force: true });
  }
});

test("forwards arguments, stdin, stdout, and stderr", () => {
  const script = [
    'const fs = require("node:fs");',
    'const input = fs.readFileSync(0, "utf8");',
    "console.log(JSON.stringify({ args: process.argv.slice(1), input }));",
    'console.error("native stderr");',
  ].join("");
  const result = runLauncher(
    ["-e", script, "first", "two words"],
    "native stdin",
  );

  assert.equal(result.status, 0);
  assert.deepEqual(JSON.parse(result.stdout), {
    args: ["first", "two words"],
    input: "native stdin",
  });
  assert.equal(result.stderr.trim(), "native stderr");
});

test("preserves a nonzero native exit status", () => {
  const result = runLauncher(["-e", "process.exit(37)"]);

  assert.equal(result.status, 37);
});

test(
  "forwards termination and preserves signal termination",
  { skip: process.platform === "win32" },
  async (t) => {
    const launcherPath = createLauncherHarness();
    const child = spawn(
      process.execPath,
      [launcherPath, "-e", 'console.log("ready"); setInterval(() => {}, 1000)'],
      {
        env: {
          ...process.env,
          GOOSE_BINARY: "",
          GOOSE_ACP_TEST_NODE: process.execPath,
        },
        stdio: ["ignore", "pipe", "pipe"],
      },
    );
    t.after(() => {
      if (child.exitCode === null && child.signalCode === null) {
        child.kill("SIGKILL");
      }
    });

    await waitForOutput(child.stdout, "ready");
    child.kill("SIGTERM");
    const result = await waitForExit(child);

    assert.equal(result.code, null);
    assert.equal(result.signal, "SIGTERM");
  },
);
