const { spawn } = require("node:child_process");
const path = require("node:path");

const targetByPlatform = {
  "linux-x64": "x86_64-unknown-linux-gnu",
  "linux-arm64": "aarch64-unknown-linux-gnu",
  "win32-x64": "x86_64-pc-windows-msvc",
  "win32-arm64": "aarch64-pc-windows-msvc",
};
const platformKey = `${process.platform}-${process.arch}`;
const args = process.argv.slice(2);
const targetFlag = args.findIndex((arg) => arg === "--target" || arg === "-t");
// An explicit `--target` (e.g. Windows from Linux) wins over the host target.
const hostTarget =
  (targetFlag >= 0 && args[targetFlag + 1]) ||
  process.env.TAURI_HOST_TARGET ||
  targetByPlatform[platformKey];
if (!hostTarget) {
  console.error(`No Tauri host target configured for ${platformKey}`);
  process.exit(1);
}

if (process.platform === "linux" && hostTarget.endsWith("-windows-msvc")) {
  // cargo-xwin needs clang-cl/lld-link/llvm-rc; Ubuntu ships them versioned
  // under /usr/lib/llvm-*/bin, so expose the newest one when none is on PATH.
  const fs = require("node:fs");
  const llvm = fs
    .readdirSync("/usr/lib")
    .filter((dir) => /^llvm-\d+$/.test(dir))
    .sort((a, b) => Number(b.slice(5)) - Number(a.slice(5)))
    .map((dir) => `/usr/lib/${dir}/bin`)
    .find((dir) => fs.existsSync(`${dir}/lld-link`));
  if (llvm) process.env.PATH = `${llvm}${path.delimiter}${process.env.PATH}`;
}

const env = {
  ...process.env,
  CARGO_BUILD_TARGET: hostTarget,
  // Cargo's inherited repository config adds firmware-only linker flags.
  CARGO_ENCODED_RUSTFLAGS: "",
  // The repository pins the ESP toolchain for firmware builds. Tauri needs
  // the normal host Rust toolchain (also on Windows when built there).
  RUSTUP_TOOLCHAIN: process.env.TAURI_RUST_TOOLCHAIN || "stable",
};

if (process.platform === "linux" && hostTarget.includes("-linux-") && !env.BINDGEN_EXTRA_CLANG_ARGS) {
  // PipeWire's bindgen run can otherwise pick a 32-bit Clang target from the
  // firmware-oriented toolchain environment while Cargo is building x86_64.
  env.BINDGEN_EXTRA_CLANG_ARGS = `--target=${hostTarget}${hostTarget.startsWith("x86_64-") ? " -m64" : ""}`;
}

if (process.platform === "linux") {
  // Snap-packaged editors can inject GTK modules built against another glibc.
  delete env.GTK_PATH;
  delete env.GIO_MODULE_DIR;
}

const cli = path.resolve(__dirname, "../node_modules/@tauri-apps/cli/tauri.js");
const child = spawn(process.execPath, [cli, ...args], {
  cwd: path.resolve(__dirname, ".."),
  env,
  stdio: "inherit",
});

child.on("error", (error) => {
  console.error(`Could not start Tauri CLI: ${error.message}`);
  process.exit(1);
});
child.on("exit", (code, signal) => {
  if (signal) {
    console.error(`Tauri CLI stopped by signal ${signal}`);
    process.exit(1);
  }
  process.exit(code ?? 1);
});
