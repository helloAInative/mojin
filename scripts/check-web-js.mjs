import { readFileSync } from "node:fs";
import { dirname, resolve } from "node:path";
import { fileURLToPath } from "node:url";
import vm from "node:vm";

const root = resolve(dirname(fileURLToPath(import.meta.url)), "..");
const htmlPath = resolve(root, "apps/tauri_shell/index.html");
const html = readFileSync(htmlPath, "utf8");
const match = html.match(/<script>([\s\S]*?)<\/script>/);
if (!match) throw new Error("apps/tauri_shell/index.html has no inline script");
new vm.Script(match[1], { filename: htmlPath });
console.log("Web shell JavaScript syntax OK");
