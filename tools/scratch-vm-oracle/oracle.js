// Runs a project on scratch-vm, headless at 30 fps, and prints what sprites say in the format of
// Thatch's headless host: the bubble text as scratch-vm would display it, one line per bubble.
//
//   npm install            (once)
//   node oracle.js <project.json or .sb3> [timeout seconds]
//
// scratch-vm's logger writes to stdout too, so take only the lines that look like bubbles:
//   node oracle.js project.json | grep -E '^[^ ].* (says|thinks): ' > expected.txt
"use strict";
const fs = require("fs");

const [projectFile, timeoutArg] = process.argv.slice(2);
const TIMEOUT_MS = Number(timeoutArg || 20) * 1000;
const VirtualMachine = require("@scratch/scratch-vm/src/index.js");

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
    if (shown !== "") console.log(`${target.getName()} ${type === "think" ? "thinks" : "says"}: ${shown}`);
  });
  const bytes = fs.readFileSync(projectFile);
  await vm.loadProject(projectFile.endsWith(".json") ? bytes.toString("utf8") : bytes);
  vm.start();
  vm.greenFlag();
  const started = Date.now();
  while (vm.runtime.threads.length > 0) {
    if (Date.now() - started > TIMEOUT_MS) {
      console.error("oracle: timed out");
      break;
    }
    await new Promise((resolve) => setTimeout(resolve, 20));
  }
  vm.stopAll();
  vm.runtime.stop();
  process.exit(0);
}

main().catch((e) => {
  console.error(e);
  process.exit(1);
});
