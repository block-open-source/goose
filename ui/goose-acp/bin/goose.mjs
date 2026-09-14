#!/usr/bin/env node

import { spawn } from "node:child_process";

import { resolveGooseBinary } from "../dist/index.js";

const forwardedSignals =
  process.platform === "win32"
    ? ["SIGINT", "SIGTERM"]
    : ["SIGHUP", "SIGINT", "SIGTERM"];

try {
  const binaryPath = resolveGooseBinary();
  const child = spawn(binaryPath, process.argv.slice(2), {
    stdio: "inherit",
  });
  const signalHandlers = new Map();

  for (const signal of forwardedSignals) {
    const handler = () => {
      if (child.exitCode === null && child.signalCode === null) {
        child.kill(signal);
      }
    };

    signalHandlers.set(signal, handler);
    process.on(signal, handler);
  }

  const removeSignalHandlers = () => {
    for (const [signal, handler] of signalHandlers) {
      process.off(signal, handler);
    }
  };

  child.once("error", (error) => {
    removeSignalHandlers();
    reportError(error);
    process.exitCode = 1;
  });

  child.once("exit", (code, signal) => {
    removeSignalHandlers();

    if (signal) {
      try {
        process.kill(process.pid, signal);
      } catch {
        process.exitCode = signalExitCode(signal);
      }
      return;
    }

    process.exitCode = code ?? 1;
  });
} catch (error) {
  reportError(error);
  process.exitCode = 1;
}

function reportError(error) {
  const message = error instanceof Error ? error.message : String(error);
  console.error(`goose: ${message}`);
}

function signalExitCode(signal) {
  switch (signal) {
    case "SIGHUP":
      return 129;
    case "SIGINT":
      return 130;
    case "SIGTERM":
      return 143;
    default:
      return 1;
  }
}
