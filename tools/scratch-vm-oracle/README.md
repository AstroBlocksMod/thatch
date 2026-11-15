# scratch-vm oracle

Runs a project on the real scratch-vm, headless, and prints what sprites say in the format of Thatch's headless host. The samples' `expected.txt` files come from here, so the tests compare Thatch with Scratch rather than with itself.

```sh
cd tools/scratch-vm-oracle
npm install
node oracle.js ../../samples/hello/project.json > ../../samples/hello/expected.txt
node oracle.js ../../samples/input/project.json ../../samples/input/input.txt > ../../samples/input/expected.txt
```

Only bubbles are written to stdout; scratch-vm's own log lines go to stderr. An argument ending in `.txt` is an input script in the format of the headless host's `--input` option (see `samples/README.md`): keys and mouse events fed before the frames it names. Any other extra argument is a timeout in seconds (20 by default). A project whose scripts are still running when it expires makes the script exit with status 2, so a sample with a `forever` loop must end with `stop all`.

Without a renderer scratch-vm cannot tell which sprite is under the mouse, so every click in an input script lands on the stage, whatever target it names; its mouse device also ignores a coordinate of exactly 0, so keep x above -240 and y below 180.

scratch-vm and the `@scratch` packages it pulls in are AGPL-3.0-only. They are a development tool here: `npm install` downloads them into `node_modules/`, which git ignores, and nothing from them is committed, linked into Thatch or shipped with it. Only their output (the `expected.txt` files) is kept. Do not copy their code into this repository.

scratch-vm runs at 30 frames per second (its compatibility mode, which the Scratch editor turns on too) on the real clock here, while Thatch's host uses a virtual clock, so a sample must not print anything that depends on how many frames a wait took. Test the order of events across waits instead, with gaps of several frames between them, and keep input events well clear of the moment a timed block ends.
