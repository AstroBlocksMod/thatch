# scratch-vm oracle

Runs a project on the real scratch-vm, headless, and prints what sprites say in the format of Thatch's headless host. The samples' `expected.txt` files come from here, so the tests compare Thatch with Scratch rather than with itself.

```sh
cd tools/scratch-vm-oracle
npm install
node oracle.js ../../samples/hello/project.json > ../../samples/hello/expected.txt
```

Only bubbles are written to stdout; scratch-vm's own log lines go to stderr. The optional second argument is a timeout in seconds (20 by default). A project whose scripts are still running when it expires makes the script exit with status 2, so a sample with a `forever` loop must end with `stop all`.

scratch-vm runs at 30 frames per second on the real clock here, while Thatch's host uses a virtual clock, so a sample must not print anything that depends on how many frames a wait took. Test the order of events across waits instead, with gaps of several frames between them.
