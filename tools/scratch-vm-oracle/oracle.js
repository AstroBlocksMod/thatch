// Runs a project on scratch-vm, headless at 30 fps, and prints what sprites say in the format of
// Thatch's headless host: the bubble text as scratch-vm would display it, one line per bubble.
//
//   npm install            (once)
//   node oracle.js <project.json or .sb3> [input.txt] [timeout seconds] > expected.txt
//
// Only bubbles go to stdout. scratch-vm's own logging goes to stderr, and a run that does not
// finish within the timeout exits with status 2. An input script in the format of the headless
// host's --input option (see samples/README.md) feeds keys and mouse events before the given frames.
"use strict";
const fs = require("fs");
const path = require("path");

// scratch-vm logs through console.log and friends; keep stdout for bubbles alone.
const toStderr = (...args) => process.stderr.write(args.map(String).join(" ") + "\n");
console.log = console.info = console.debug = console.warn = toStderr;

const [projectFile, ...restArgs] = process.argv.slice(2);
if (!projectFile) {
  process.stderr.write("usage: node oracle.js <project.json or .sb3> [input.txt] [timeout seconds]\n");
  process.exit(64);
}
const inputFile = restArgs.find((a) => a.endsWith(".txt"));
const timeoutArg = restArgs.find((a) => !a.endsWith(".txt"));
const TIMEOUT_MS = Number(timeoutArg || 20) * 1000;

// The first whitespace-separated word of a string and the trimmed rest, as the headless host
// splits its input lines, so a key or target name keeps the spaces inside it.
function firstWord(s) {
  const m = /^\s*(\S*)\s*([\s\S]*?)\s*$/.exec(s);
  return [m[1], m[2]];
}

// The input script: "FRAME key down|up NAME", "FRAME mouse X Y", "FRAME mouse down|up [TARGET]",
// with words separated by any whitespace.
function parseInput(text) {
  const events = [];
  text.split("\n").forEach((raw, i) => {
    const line = raw.trim();
    if (line === "" || line.startsWith("#")) return;
    const bad = () => new Error(`input line ${i + 1}: cannot read ${JSON.stringify(raw)}`);
    const [frame, afterFrame] = firstWord(line);
    const [kind, rest] = firstWord(afterFrame);
    const [first, afterFirst] = firstWord(rest);
    const event = { frame: Number(frame), kind };
    if (kind === "key" && (first === "down" || first === "up") && afterFirst !== "") {
      event.down = first === "down";
      event.key = afterFirst;
    } else if (kind === "mouse" && (first === "down" || first === "up")) {
      event.kind = "button";
      event.down = first === "down";
      event.target = afterFirst;
    } else if (kind === "mouse") {
      const [y, extra] = firstWord(afterFirst);
      event.x = Number(first);
      event.y = Number(y);
      if (first === "" || y === "" || Number.isNaN(event.x) || Number.isNaN(event.y) || extra !== "") throw bad();
    } else {
      throw bad();
    }
    if (Number.isNaN(event.frame) || frame === "") throw bad();
    events.push(event);
  });
  return events.sort((a, b) => a.frame - b.frame);
}

// Scratch key names back to the KeyboardEvent.key names scratch-vm's keyboard device expects.
const BROWSER_KEYS = { space: " ", "left arrow": "ArrowLeft", "up arrow": "ArrowUp", "right arrow": "ArrowRight", "down arrow": "ArrowDown", enter: "Enter" };
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

  // Feed the input script before the frames it names, counting frames from the first step after
  // the green flag, as the headless host does. The mouse device wants client coordinates on a
  // 480 by 360 canvas, so Scratch coordinates are converted; since it ignores a coordinate of 0
  // and clicks on the canvas edge, scripts should keep x above -240 and y below 180. Without a
  // renderer every click lands on the stage, whatever target the script names.
  const events = inputFile ? parseInput(fs.readFileSync(inputFile, "utf8")) : [];
  let frame = 0;
  let nextEvent = 0;
  const mouse = { x: 240, y: 180 };
  const apply = (e) => {
    if (e.kind === "key") {
      vm.postIOData("keyboard", { key: BROWSER_KEYS[e.key] || e.key, isDown: e.down });
    } else if (e.kind === "mouse") {
      mouse.x = e.x + 240;
      mouse.y = 180 - e.y;
      vm.postIOData("mouse", { x: mouse.x, y: mouse.y, canvasWidth: 480, canvasHeight: 360 });
    } else {
      vm.postIOData("mouse", { x: mouse.x, y: mouse.y, canvasWidth: 480, canvasHeight: 360, isDown: e.down });
    }
  };
  const step = vm.runtime._step.bind(vm.runtime);
  vm.runtime._step = () => {
    while (nextEvent < events.length && events[nextEvent].frame <= frame) apply(events[nextEvent++]);
    frame++;
    step();
  };

  // Scratch runs at 30 frames per second; scratch-vm alone would step 60 times a second. The
  // headless host's virtual clock counts a thirtieth of a second per frame, so this has to match
  // for input scripts and timed blocks to line up.
  vm.setCompatibilityMode(true);
  vm.start();
  vm.greenFlag();
  const started = Date.now();
  let status = 0;
  while (vm.runtime.threads.length > 0 || nextEvent < events.length) {
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
