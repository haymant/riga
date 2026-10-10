#!/usr/bin/env node
import fs from "node:fs";
import path from "node:path";
import { spawnSync } from "node:child_process";

const root = process.cwd();
const env = { ...process.env };
const commandArgs = process.argv.slice(2);
if (commandArgs[0] === "--") commandArgs.shift();
const targetRoot = env.CARGO_TARGET_DIR || path.join(root, "target");

function which(command) {
  const result = spawnSync("sh", ["-lc", `command -v ${command}`], {
    encoding: "utf8",
    stdio: ["ignore", "pipe", "ignore"],
  });
  return result.status === 0 ? result.stdout.trim() : null;
}

const cmake = which("cmake");
const compiler = which("c++") || which("g++");
const nvcc = env.CUDACXX || env.CMAKE_CUDA_COMPILER || which("nvcc");
if (!cmake || !compiler || !nvcc) {
  console.error("CUDA CLI build requires cmake, a C++ compiler, and nvcc from the CUDA Toolkit.");
  console.error("Install build-essential cmake ninja-build and the NVIDIA CUDA Toolkit, then retry.");
  process.exit(2);
}

const cudaRoot = env.CUDA_PATH || env.CUDA_HOME || path.dirname(path.dirname(nvcc));
env.CUDA_PATH ||= cudaRoot;
env.CUDA_HOME ||= cudaRoot;
env.CUDACXX ||= nvcc;
env.CMAKE_CUDA_COMPILER ||= nvcc;
env.CUDA_LIBRARY_PATH ||= cudaRoot;
env.CMAKE_CUDA_ARCHITECTURES ||= "86";
env.CMAKE_POSITION_INDEPENDENT_CODE ||= "ON";
env.CMAKE_CUDA_FLAGS = `${env.CMAKE_CUDA_FLAGS || ""} -Xcompiler=-fPIC`.trim();
env.CUDAFLAGS = `${env.CUDAFLAGS || ""} -Xcompiler -fPIC`.trim();

function includeDir(rootDir) {
  const candidates = [
    path.join(rootDir, "targets", "x86_64-linux", "include"),
    path.join(rootDir, "include"),
  ];
  const targets = path.join(rootDir, "targets");
  if (fs.existsSync(targets)) {
    for (const entry of fs.readdirSync(targets)) candidates.push(path.join(targets, entry, "include"));
  }
  return candidates.find((dir) => fs.existsSync(path.join(dir, "crt", "math_functions.h")));
}

function conflictingSymbols(include) {
  const probeDir = path.join(targetRoot, "cuda-header-probe");
  fs.mkdirSync(probeDir, { recursive: true });
  const source = path.join(probeDir, "probe.cu");
  fs.writeFileSync(
    source,
    "#include <cuda_runtime.h>\n#include <cmath>\n#include <mutex>\n#include <locale>\n__global__ void k(float* p) { p[threadIdx.x] = rsqrtf(p[threadIdx.x]) + std::sqrt(2.0f) + rsqrt((double)p[threadIdx.x]); }\nint main() { std::mutex m; std::locale l(\"C\"); (void)m; (void)l; return 0; }\n",
  );
  const args = ["-c", source, "-o", path.join(probeDir, "probe.o")];
  if (include) args.unshift(`-I${include}`);
  const probe = spawnSync(nvcc, args, { encoding: "utf8" });
  if (probe.status === 0) return [];
  const output = `${probe.stdout || ""}${probe.stderr || ""}`;
  return [...output.matchAll(/previous function "([A-Za-z_][A-Za-z0-9_]*)"/g)].map((match) => match[1]);
}

function patchDeclarations(shimInclude, symbols) {
  const declaration = (symbol) => new RegExp(`^(\\s*extern\\b.*?\\b${symbol}\\s*\\()(.*?)(\\)\\s*;)\\s*$`);
  let patched = 0;
  for (const entry of fs.readdirSync(path.join(shimInclude, "crt"))) {
    if (!entry.endsWith(".h") && !entry.endsWith(".hpp")) continue;
    const file = path.join(shimInclude, "crt", entry);
    const lines = fs.readFileSync(file, "utf8").split("\n");
    let changed = false;
    for (let index = 0; index < lines.length; index += 1) {
      for (const symbol of symbols) {
        const match = declaration(symbol).exec(lines[index]);
        if (!match) continue;
        lines[index] = `${match[1]}${match[2]}) noexcept(true);`;
        changed = true;
        patched += 1;
      }
    }
    if (changed) fs.writeFileSync(file, lines.join("\n"));
  }
  return patched;
}

// CUDA 12.6 headers conflict with glibc 2.41's noexcept math declarations.
// Shadow the toolkit include tree and add noexcept(true) to only the conflicting
// CUDA declarations. This is the same compatibility strategy used by the Tauri
// CUDA launcher, but is applied to the CLI's Cargo build as well.
let cudaShimInclude = null;
if (process.platform === "linux" && env.RIGA_CUDA_HEADER_SHIM !== "off") {
  const sourceInclude = includeDir(cudaRoot);
  const symbols = conflictingSymbols(null);
  if (symbols.length > 0 && sourceInclude) {
    const version = spawnSync(nvcc, ["--version"], { encoding: "utf8" }).stdout?.match(/release ([\d.]+)/)?.[1] || "unknown";
    const shimInclude = path.join(targetRoot, "cuda-header-shim", `${path.basename(cudaRoot)}-${version}`, "include");
    cudaShimInclude = shimInclude;
    fs.rmSync(path.dirname(shimInclude), { recursive: true, force: true });
    fs.mkdirSync(path.dirname(shimInclude), { recursive: true });
    fs.cpSync(sourceInclude, shimInclude, { recursive: true });
    const patched = patchDeclarations(shimInclude, [...new Set(symbols)]);
    if (patched === 0 || conflictingSymbols(shimInclude).length > 0) {
      console.error("CUDA header compatibility shim could not resolve the glibc/CUDA math declarations.");
      process.exit(1);
    }
    console.log(`Using CUDA/glibc header compatibility shim for: ${[...new Set(symbols)].join(", ")}`);
    env.CUDAFLAGS = `${env.CUDAFLAGS} -I${shimInclude}`;
    env.CMAKE_CUDA_FLAGS = `${env.CMAKE_CUDA_FLAGS} -I${shimInclude}`;
  }
}

// A killed or interrupted CMake configure can leave out/build without a
// Makefile/build.ninja. Remove only that stale output so Cargo reconfigures.
for (const profile of ["debug", "release"]) {
  const buildRoot = path.join(targetRoot, profile, "build");
  if (!fs.existsSync(buildRoot)) continue;
  for (const entry of fs.readdirSync(buildRoot)) {
    if (!entry.startsWith("llama-cpp-sys-2-")) continue;
    const out = path.join(buildRoot, entry, "out");
    const cmakeBuild = path.join(out, "build");
    const hasBuildSystem = fs.existsSync(path.join(cmakeBuild, "Makefile")) || fs.existsSync(path.join(cmakeBuild, "build.ninja"));
    const cache = path.join(cmakeBuild, "CMakeCache.txt");
    const cacheText = fs.existsSync(cache) ? fs.readFileSync(cache, "utf8") : "";
    const cacheFlags = cacheText.match(/^CMAKE_CUDA_FLAGS:STRING=(.*)$/m)?.[1] || "";
    const missingShim = cudaShimInclude && !cacheFlags.includes(cudaShimInclude);
    const missingCuda = cacheText && !/^GGML_CUDA:BOOL=ON$/m.test(cacheText);
    if (fs.existsSync(cmakeBuild) && (!hasBuildSystem || missingShim || missingCuda)) {
      console.log(`Removing stale CUDA CMake output: ${cmakeBuild}`);
      fs.rmSync(out, { recursive: true, force: true });
    }
  }
}

const result = spawnSync("cargo", ["run", "-p", "riga-cli", "--features", "riga-server/cuda", "--", ...commandArgs], {
  cwd: root,
  env,
  stdio: "inherit",
});
process.exit(result.status ?? 1);
