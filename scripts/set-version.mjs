const version = process.argv[2] ?? "0.1.0";
if (!/^\\d+\\.\\d+\\.\\d+$/.test(version)) {
  throw new Error(`invalid semantic version: ${version}`);
}
console.log(`RIGA version source prepared: ${version}`);
