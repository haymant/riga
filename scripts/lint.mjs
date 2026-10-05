import { readFile } from "node:fs/promises";

const forbidden = /rig-coding-app/i;
const files = ["README.md", "package.json", "Cargo.toml"];
for (const file of files) {
  const content = await readFile(new URL(`../${file}`, import.meta.url), "utf8");
  if (forbidden.test(content)) {
    throw new Error(`forbidden retired product name found in ${file}`);
  }
}
console.log("RIGA naming lint passed");
