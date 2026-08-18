// Writes a `package.json` marker into dist/cjs so Node treats the emitted
// `.js` files there as CommonJS (the package root is `"type": "module"`).
const fs = require("node:fs");
const path = require("node:path");

const root = path.join(__dirname, "..");
const cjsDir = path.join(root, "dist", "cjs");
fs.mkdirSync(cjsDir, { recursive: true });
fs.writeFileSync(path.join(cjsDir, "package.json"), JSON.stringify({ type: "commonjs" }));
console.log("Wrote dist/cjs/package.json with type=commonjs");