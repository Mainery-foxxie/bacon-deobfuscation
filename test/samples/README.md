`safe_vm.lua` is a small, self-authored Bacon Guard-style **format fixture**. It is
not output from the proprietary obfuscator and contains no downloaded code. Its
readable interpreter and explicit records are the reproducible source; there is
no external generator, protected input, or network dependency.

The payload is equivalent to:

```lua
local value = tonumber("41")
print("safe fixture", value)
return value
```

The fixture supplies the interpreter layout expected by `loader.py`, one
prototype, four string constants, and seven records. The first record expands an
XOR-encoded pattern into GETIMPORT and LOADK; the remaining records exercise plain
simple dispatch, CALL control dispatch, MOVE, and a hot RETURN. Empty dedicated
tables and unused fallback branches provide the complete loader scaffold. The
behavioral classifier identifies the handlers from their actual implementations.
The fixture has no Bacon Guard banner so detection also covers structural
recognition without a brand string.

Engine tests check detection, dumping, classification, decompilation, fidelity,
determinism, annotation, renaming, rejection of damaged/truncated input, and the
known print and return values. Existing tracer sample tests compare the raw VM
with its recovered source in the fake Roblox environment. Deadline tests derive
a copy with a bounded 30-second busy loop at module initialization; the worker
must stop it at the existing shorter deadlines and remove its temporary files.

From the project root, run `cargo test --locked --test parity` to compare this fixture against the saved reference. This fixture covers its documented small VM subset, not every Bacon Guard build.
