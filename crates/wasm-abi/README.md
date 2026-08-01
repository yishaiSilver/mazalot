# wasm-abi

The raw C-ABI glue every demo's `wasm.rs` is built from: `alloc`/`dealloc` and the
opaque-handle macros. **No wasm-bindgen.** 87 lines, exports no symbols itself.

## The contract

A demo module has no imports — instantiate it with `{}` — and the JS calls its
exports by name:

```js
const m = await WebAssembly.instantiate(bytes, {})
const p = m.exports.alloc(w * h * 4)
m.exports.render(p, w, h, /* … */)
const px = new Uint8ClampedArray(m.exports.memory.buffer, p, w * h * 4)
m.exports.dealloc(p, w * h * 4)
```

Scene demos add an opaque handle (`system_new`, `system_set_view`, …) built with
this crate's macros: the Rust value stays boxed in wasm memory and JS holds only an
integer.

## Why demo crates never depend on each other

Those exports are `#[no_mangle]`, so two demo cdylibs linked together collide on
`render`/`alloc`/`dealloc`. Demo crates share code through library rlibs only. This
is why `planet-core` exists as a crate separate from `planet`.

## Checking the export set

The export list is the contract with the JS, and changing it breaks a demo
silently:

```bash
cargo build -p solar --target wasm32-unknown-unknown --release --no-default-features
node -e 'const m=new WebAssembly.Module(require("fs").readFileSync(process.argv[1]));
         console.log(WebAssembly.Module.exports(m).map(e=>e.kind+" "+e.name).sort().join("\n"))' \
     crates/solar/web/solar.wasm
```

Diff that before and after any change to a `wasm.rs`.
