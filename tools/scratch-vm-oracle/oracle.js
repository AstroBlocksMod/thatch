// Runs a project on scratch-vm, headless at 30 fps, and prints what sprites say in the format of
// Thatch's headless host: the bubble text as scratch-vm would display it, one line per bubble.
//
//   npm install            (once)
//   node oracle.js <project.json or .sb3> [timeout seconds] > expected.txt
//
// Only bubbles go to stdout. scratch-vm's own logging goes to stderr, and a run that does not
// finish within the timeout exits with status 2.
"use strict";
const fs = require("fs");
const path = require("path");

// scratch-vm logs through console.log and friends; keep stdout for bubbles alone.
const toStderr = (...args) => process.stderr.write(args.map(String).join(" ") + "\n");
console.log = console.info = console.debug = console.warn = toStderr;

const [projectFile, timeoutArg] = process.argv.slice(2);
if (!projectFile) {
  process.stderr.write("usage: node oracle.js <project.json or .sb3> [timeout seconds]\n");
  process.exit(64);
}
const TIMEOUT_MS = Number(timeoutArg || 20) * 1000;
// The package's Node bundle reads browser files that are not published, so load the sources. A
// path rather than a package specifier gets past the package's "exports" list.
const VirtualMachine = require(path.join(__dirname, "node_modules", "@scratch", "scratch-vm", "src", "index.js"));

// scratch-vm's _formatBubbleText.
function bubble(text) {
  if (text === "") return text;
  if (typeof text === "number" && Math.abs(text) >= 0.01 && text % 1 !== 0) text = text.toFixed(2);
  return String(text).substr(0, 330);
}

async function main() {
  const vm = new VirtualMachine();
  vm.runtime.on("SAY", (target, type, text) => {
    const shown = bubble(text);
    if (shown !== "") process.stdout.write(`${target.getName()} ${type === "think" ? "thinks" : "says"}: ${shown}\n`);
  });
  const bytes = fs.readFileSync(projectFile);
  await vm.loadProject(projectFile.endsWith(".json") ? bytes.toString("utf8") : bytes);
  vm.start();
  vm.greenFlag();
  const started = Date.now();
  let status = 0;
  while (vm.runtime.threads.length > 0) {
    if (Date.now() - started > TIMEOUT_MS) {
      process.stderr.write("oracle: timed out\n");
      status = 2;
      break;
    }
    await new Promise((resolve) => setTimeout(resolve, 20));
  }
  vm.stopAll();
  vm.quit();
  // Let stdout drain before exiting.
  process.stdout.write("", () => process.exit(status));
}

main().catch((e) => {
  process.stderr.write(`${e && e.stack ? e.stack : e}\n`);
  process.exit(1);
});
