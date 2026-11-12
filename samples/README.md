# Samples

Each directory holds a `project.json` and the `expected.txt` a headless run must print. The expected
output comes from running the same project on scratch-vm, so the tests check Thatch against Scratch
itself, not against what Thatch happened to print. `thatch run samples/<name>/project.json` runs one.
