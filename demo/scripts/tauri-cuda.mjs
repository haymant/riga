// Run the Tauri app with the `cuda` feature so llama.cpp offloads to an NVIDIA
// GPU (the server then auto-fits the GPU plan to the memory actually free).
//
// A plain `tauri dev --features cuda` is not enough on a typical Linux box: the
// CUDA build needs a coherent toolkit, position-independent CUDA objects for the
// cdylib link, and — on glibc >= 2.41 — a small header shim. This wrapper sets
// those up and then invokes the Tauri CLI.
//
//   npm run tauri:dev:cuda     # or: npm run tauri:build:cuda
//   CMAKE_CUDA_ARCHITECTURES=89 npm run tauri:build:cuda
import { spawnSync } from "node:child_process";
import { cpSync, existsSync, mkdirSync, readdirSync, readFileSync, rmSync, writeFileSync } from "node:fs";
import { basename, delimiter, dirname, join } from "node:path";

const mode = process.argv[2] ?? "dev";
if (!["dev", "build"].includes(mode)) {
  console.error("Usage: node scripts/tauri-cuda.mjs <dev|build> [...tauri args]");
  process.exit(2);
}

const tauriBin = join(
  process.cwd(),
  "node_modules",
  ".bin",
  process.platform === "win32" ? "tauri.cmd" : "tauri",
);

/** Compute capability of the first NVIDIA GPU, or a sane default. */
function defaultCudaArch() {
  const probe = spawnSync(
    "nvidia-smi",
    ["--query-gpu=compute_cap", "--format=csv,noheader"],
    { encoding: "utf8" },
  );
  const cap = probe.status === 0 ? probe.stdout.trim().split("\n")[0]?.trim() : "";
  // "8.6" -> "86"
  return cap && /^\d+\.\d+$/.test(cap) ? cap.replace(".", "") : "86";
}

const env = {
  ...process.env,
  // Only the GPU's own architecture: compiling every default arch (plus PTX
  // variants) dominates the build. Override with CMAKE_CUDA_ARCHITECTURES.
  CMAKE_CUDA_ARCHITECTURES: process.env.CMAKE_CUDA_ARCHITECTURES || defaultCudaArch(),
  // The static CUDA objects are linked into Tauri's cdylib, so they must be PIC.
  CMAKE_POSITION_INDEPENDENT_CODE: process.env.CMAKE_POSITION_INDEPENDENT_CODE || "ON",
};

/**
 * CMake can otherwise combine nvcc from one installation with headers/libraries
 * from another, which produces misleading errors when nvcc invokes a different
 * toolkit's ptxas. Prefer an explicitly configured toolkit, then a CUDA bin dir
 * already on PATH, then `/usr/local/cuda`.
 */
function configureCoherentCudaToolkit() {
  if (env.CUDACXX || env.CMAKE_CUDA_COMPILER) return;
  const pathCandidates = (env.PATH || "")
    .split(delimiter)
    .filter((entry) => basename(entry) === "bin" && basename(dirname(entry)).startsWith("cuda"))
    .map((entry) => dirname(entry));
  const roots = [env.CUDA_PATH, env.CUDA_HOME, ...pathCandidates, "/usr/local/cuda"];
  const root = roots.find((candidate) => candidate && existsSync(join(candidate, "bin", "nvcc")));
  if (!root) return;
  const nvcc = join(root, "bin", "nvcc");
  env.CUDA_PATH ||= root;
  env.CUDACXX = nvcc;
  env.CMAKE_CUDA_COMPILER = nvcc;
  // The build script reads only CUDA_LIBRARY_PATH for the linker search order;
  // it appends lib64/lib64-stubs, so the toolkit *root* is correct.
  env.CUDA_LIBRARY_PATH ||= root;
  env.PATH = `${join(root, "bin")}${delimiter}${env.PATH || ""}`;
}

configureCoherentCudaToolkit();

const targetRoot =
  process.env.CARGO_TARGET_DIR || join(process.cwd(), "src-tauri", "target");

// Flags that must reach every CUDA compile. CMake seeds CMAKE_CUDA_FLAGS from the
// CUDAFLAGS environment variable, so this is the only injection point that also
// reaches enable_language(CUDA)'s compiler-ID probe.
const requiredCudaFlags = [];

function addCudaFlags(...flags) {
  for (const flag of flags) {
    if (requiredCudaFlags.includes(flag)) continue;
    requiredCudaFlags.push(flag);
    env.CUDAFLAGS = `${env.CUDAFLAGS ?? ""} ${flag}`.trim();
  }
}

// nvcc rejects a bare -fPIC ("Unknown option"), so it is forwarded to the host
// compiler. Without it the final cdylib link fails with
// "relocation R_X86_64_PC32 cannot be used against symbol 'stderr'".
function applyPositionIndependentCode() {
  if (process.platform !== "linux") return;
  addCudaFlags("-Xcompiler", "-fPIC");
}

applyPositionIndependentCode();

// ---------------------------------------------------------------------------
// glibc >= 2.41 vs CUDA headers.
//
// bits/libc-header-start.h enables the C23 IEC 60559 math functions whenever
// `__USE_GNU` is set, and g++ always defines `_GNU_SOURCE`. glibc therefore
// declares `rsqrt`/`rsqrtf` with `__THROW` (noexcept(true)), while CUDA's
// crt/math_functions.h declares the same names as device builtins with no
// exception specification. The second declaration is ill-formed, so any .cu file
// that pulls in both headers fails, and CMake's enable_language(CUDA) probe dies
// before llama.cpp is reached.
//
// The toolkit is root-owned, so its include tree is shadowed rather than edited:
// nvcc resolves <cuda_runtime.h> from -I paths before its built-in ones, and the
// quoted includes inside the copy stay within the copy.
// ---------------------------------------------------------------------------

function cudaIncludeDir(root) {
  const hostTriple = `${process.arch === "x64" ? "x86_64" : process.arch}-linux`;
  const candidates = [join(root, "targets", hostTriple, "include"), join(root, "include")];
  const targets = join(root, "targets");
  if (existsSync(targets)) {
    for (const entry of readdirSync(targets)) candidates.push(join(targets, entry, "include"));
  }
  return candidates.find((dir) => existsSync(join(dir, "crt", "math_functions.h")));
}

function conflictingSymbols(nvcc, includeDir) {
  const probeDir = join(targetRoot, "cuda-header-probe");
  const source = join(probeDir, "probe.cu");
  mkdirSync(probeDir, { recursive: true });
  writeFileSync(
    source,
    [
      "#include <cuda_runtime.h>",
      "#include <cmath>",
      "#include <mutex>",
      "#include <locale>",
      "__global__ void k(float* p) {",
      "  p[threadIdx.x] = rsqrtf(p[threadIdx.x]) + std::sqrt(2.0f) + rsqrt((double)p[threadIdx.x]);",
      "}",
      'int main() { std::mutex m; std::locale l("C"); (void)m; (void)l; return 0; }',
      "",
    ].join("\n"),
  );
  const args = ["-c", source, "-o", join(probeDir, "probe.o")];
  if (includeDir) args.unshift(`-I${includeDir}`);
  const probe = spawnSync(nvcc, args, { encoding: "utf8" });
  if (probe.status === 0) return [];
  const output = `${probe.stdout ?? ""}${probe.stderr ?? ""}`;
  const symbols = new Set(
    [...output.matchAll(/previous function "([A-Za-z_][A-Za-z0-9_]*)"/g)].map((match) => match[1]),
  );
  if (symbols.size === 0) {
    console.error(`Could not compile the CUDA header probe with ${nvcc}:\n${output.trim()}`);
    process.exit(1);
  }
  return [...symbols].sort();
}

/** Add glibc's `noexcept (true)` to CUDA's `extern` declaration of `symbol`. */
function addNoexcept(shimInclude, symbol) {
  const escaped = symbol.replace(/[.*+?^${}()|[\]\\]/g, "\\$&");
  const declaration = new RegExp(`^(\\s*extern\\b.*?\\b${escaped}\\s*\\()(.*?)(\\)\\s*;)\\s*$`);
  let patched = 0;
  for (const entry of readdirSync(join(shimInclude, "crt"))) {
    if (!entry.endsWith(".h") && !entry.endsWith(".hpp")) continue;
    const file = join(shimInclude, "crt", entry);
    const lines = readFileSync(file, "utf8").split("\n");
    let changed = false;
    for (let i = 0; i < lines.length; i += 1) {
      const match = declaration.exec(lines[i]);
      if (!match) continue;
      lines[i] = `${match[1]}${match[2]}) noexcept(true);`;
      changed = true;
      patched += 1;
    }
    if (changed) writeFileSync(file, lines.join("\n"));
  }
  return patched;
}

function shimLayout(nvcc, root) {
  const source = cudaIncludeDir(root);
  if (!source) return null;
  const version = spawnSync(nvcc, ["--version"], { encoding: "utf8" }).stdout?.match(/release ([\d.]+)/)?.[1];
  const shimRoot = join(targetRoot, "cuda-header-shim", `${basename(root)}-${version ?? "unknown"}`);
  return { source, shimRoot, shimInclude: join(shimRoot, "include"), marker: join(shimRoot, "patched.json") };
}

function buildCudaHeaderShim(nvcc, layout) {
  const { source, shimRoot, shimInclude, marker } = layout;
  console.log(`Preparing a CUDA header shim for glibc (${basename(shimRoot)})`);
  rmSync(shimRoot, { recursive: true, force: true });
  mkdirSync(shimRoot, { recursive: true });
  cpSync(source, shimInclude, { recursive: true });
  const patched = [];
  for (let round = 0; round < 16; round += 1) {
    const symbols = conflictingSymbols(nvcc, shimInclude);
    if (symbols.length === 0) {
      writeFileSync(marker, JSON.stringify({ source, patched }, null, 2));
      console.log(`CUDA header shim ready: ${patched.join(", ")}`);
      return shimInclude;
    }
    for (const symbol of symbols) {
      const sites = addNoexcept(shimInclude, symbol);
      if (sites > 0) patched.push(`${symbol} (${sites})`);
      else {
        console.error(`Could not find a CUDA declaration to patch for "${symbol}".`);
        process.exit(1);
      }
    }
  }
  console.error(`CUDA header shim did not converge after 16 rounds: ${patched.join(", ")}`);
  process.exit(1);
}

function applyCudaHeaderShim() {
  if (process.platform !== "linux") return;
  if (process.env.RIGA_CUDA_HEADER_SHIM === "off") return;
  const nvcc = env.CUDACXX || env.CMAKE_CUDA_COMPILER;
  const root = env.CUDA_PATH || env.CUDA_HOME;
  if (!nvcc || !root) return;
  const layout = shimLayout(nvcc, root);
  if (!layout) return;
  const cached =
    existsSync(layout.marker) && conflictingSymbols(nvcc, layout.shimInclude).length === 0
      ? layout.shimInclude
      : null;
  if (cached) {
    addCudaFlags(`-I${cached}`);
    return;
  }
  if (conflictingSymbols(nvcc, null).length === 0) return;
  addCudaFlags(`-I${buildCudaHeaderShim(nvcc, layout)}`);
}

applyCudaHeaderShim();

// ---------------------------------------------------------------------------
// llama-cpp-sys-2 builds with `always_configure(false)`, so an existing CMake
// cache is reused verbatim and new CUDAFLAGS never reach it. A cache recorded
// before a flag was added keeps compiling with the stale value (e.g. a non-PIC
// archive), and Cargo's fingerprint still matches, so the build script is
// skipped and the stale archive is linked unchanged. Remove the crate's own
// artifacts so the script re-runs with the flags just computed.
// ---------------------------------------------------------------------------

const LLAMA_SYS = "llama-cpp-sys-2";

function removeLlamaSysArtifacts(profile) {
  const profileRoot = join(targetRoot, profile);
  const prefixed = [LLAMA_SYS, `lib${LLAMA_SYS.replace(/-/g, "_")}-`];
  for (const dir of ["build", "deps", ".fingerprint"]) {
    const root = join(profileRoot, dir);
    if (!existsSync(root)) continue;
    for (const entry of readdirSync(root)) {
      if (!prefixed.some((prefix) => entry.startsWith(prefix))) continue;
      rmSync(join(root, entry), { recursive: true, force: true });
    }
  }
}

function staleReason(profile) {
  const profileRoot = join(targetRoot, profile);
  const depsRoot = join(profileRoot, "deps");
  const hasArtifacts =
    existsSync(depsRoot) &&
    readdirSync(depsRoot).some((entry) => entry.startsWith(`lib${LLAMA_SYS.replace(/-/g, "_")}-`));

  const buildRoot = join(profileRoot, "build");
  const trees = (existsSync(buildRoot) ? readdirSync(buildRoot) : [])
    .filter((entry) => entry.startsWith(`${LLAMA_SYS}-`))
    .map((entry) => join(buildRoot, entry, "out", "build"))
    .filter((dir) => existsSync(dir));

  if (trees.length === 0) return hasArtifacts ? "archive without a CMake tree" : null;
  for (const tree of trees) {
    const hasBuildSystem = existsSync(join(tree, "Makefile")) || existsSync(join(tree, "build.ninja"));
    if (!hasBuildSystem) return "half-configured CMake tree";
    const cache = readFileSync(join(tree, "CMakeCache.txt"), "utf8");
    // A plain `tauri dev` shares this CMake tree and reconfigures it with
    // `GGML_CUDA=OFF`, so a CUDA run that reused it would silently fall back to
    // CPU. Treat a cache without CUDA as stale and rebuild.
    if (!/^GGML_CUDA:BOOL=ON$/m.test(cache)) return "CMake cache built without CUDA";
    const cudaFlags = cache.match(/^CMAKE_CUDA_FLAGS:STRING=(.*)$/m)?.[1] ?? "";
    const missing = requiredCudaFlags.filter((flag) => !cudaFlags.includes(flag));
    if (missing.length > 0) return `CMake cache missing ${missing.join(" ")}`;
  }
  return null;
}

function resetStaleLlamaCpp() {
  const stale = [];
  for (const profile of ["debug", "release"]) {
    const reason = staleReason(profile);
    if (reason) stale.push([profile, reason]);
  }
  for (const [profile, reason] of stale) {
    console.log(`Rebuilding llama.cpp for ${profile}: ${reason}`);
    removeLlamaSysArtifacts(profile);
  }
}

resetStaleLlamaCpp();

const result = spawnSync(tauriBin, [mode, "--features", "cuda", ...process.argv.slice(3)], {
  env,
  stdio: "inherit",
});

if (result.error) {
  console.error(`Could not start the Tauri CLI: ${result.error.message}`);
  process.exit(1);
}
process.exit(result.status ?? 1);
