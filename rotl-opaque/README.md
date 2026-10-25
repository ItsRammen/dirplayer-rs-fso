# rotl-opaque

OPAQUE (RFC 9807) password login for Return of the Legends, built on `opaque-ke`. The player proves it knows the
password without sending it, and the server stores a record the password cannot be recovered from, so neither the
server, the edge in front of it, nor any log ever sees the password.

One crate, two uses:

* **Server** — built to WebAssembly for Node and vendored in `vendor/rotl-opaque/` (so deploying needs no Rust).
* **Player** — used as a normal Rust dependency by the dirplayer-rs fork (without the `js` feature, so no JavaScript bindings), so both sides always come from the same code.

Everything is plain bytes: see `src/lib.rs`. A login is two messages each way, and so is registering a password. The
account name (lower-cased) is part of the proof, so a stored record only works for its own account. An account that does
not exist gets an answer indistinguishable from a real one.

## Never change once records exist

`CONTEXT`, `SERVER_ID`, the cipher suite and the Argon2 settings (the defaults of `opaque_ke::argon2::Argon2`). Every
stored record and every player build would stop agreeing. The server's `serverSetupNew()` secret must be kept: lose it and
every record is useless.

## Test and rebuild

```bash
cd crypto/rotl-opaque
cargo test --release                      # 13 tests, incl. records made by the WebAssembly build
wasm-pack build --target nodejs --release --out-dir pkg --out-name rotl_opaque -- --features js
cp pkg/rotl_opaque.js pkg/rotl_opaque_bg.wasm pkg/*.d.ts ../../vendor/rotl-opaque/
node --test -r ts-node/register ../../src/tests/opaque-wasm.test.ts   # from the repo root: npm test
```

The fixtures in `tests/fixtures/` (a server secret and one stored record each, made by the native and by the WebAssembly
build) prove the two builds agree; regenerate with `cargo run --release --example fixture` and
`node scripts/make-wasm-fixture.js`.
