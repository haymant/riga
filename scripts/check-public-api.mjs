import { readFile } from "node:fs/promises";

const source = await readFile(new URL("../crates/riga-kernel/src/lib.rs", import.meta.url), "utf8");
if (!source.includes("PROTOCOL_VERSION")) {
  throw new Error("kernel protocol marker is missing");
}
console.log("public API scaffold check passed");
