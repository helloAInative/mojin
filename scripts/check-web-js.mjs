import { readFileSync } from "node:fs";
import vm from "node:vm";

const html = readFileSync("apps/tauri_shell/index.html", "utf8");
const match = html.match(/<script>([\s\S]*?)<\/script>/);
if (!match) throw new Error("apps/tauri_shell/index.html has no inline script");
new vm.Script(match[1], { filename: "apps/tauri_shell/index.html" });
console.log("Web shell JavaScript syntax OK");
