# dirplayer-rs-fso — the ROTL branch

[ItsRammen/dirplayer-rs-fso](https://github.com/ItsRammen/dirplayer-rs-fso) is a fork of
[igorlira/dirplayer-rs](https://github.com/igorlira/dirplayer-rs),
carrying the fixes Return of the Legends (FSOS) needs to run in the browser. The
`rotl` branch is upstream plus one commit per fix; `main` is left tracking
upstream.

Most of the fixes are Director behaviour dirplayer didn't have yet (script
`quit`, VOID into fields, thick `#rect` draws, field scrolling, 16-bit sound
byte order, `intersects` against drawn positions, …) and are worth offering
upstream as pull requests. Each commit message says what it fixes and which
part of ROTL showed it.

## Building

Needs Rust with the `wasm32-unknown-unknown` target, wasm-pack 0.15, and Node 22.

```bash
npm ci
npm run build-vm
npm run build-polyfill
```

The result is `dist-polyfill/dirplayer-polyfill.js`, one file with the VM
inlined. The ROTL game page (NodeJS-ROTL, `scripts/dirplayer-dev.js`) serves it
from its polyfill folder.

Pushing a tag named `rotl-v*` builds the file in GitHub Actions and attaches it
to a release (`.github/workflows/rotl-release.yml`). The upstream workflows are
kept unchanged so the branch rebases cleanly; disable them in the fork's
Actions settings — the CDN deploy one runs on every published release.

## Updating from upstream

```bash
git fetch upstream
git rebase upstream/main rotl
```

A conflict shows up on the commit whose fix it touches. Rebuild and play-test
before tagging.

## License

GPL-3.0, as upstream (see `LICENSE`). Anyone serving the built polyfill to
players must offer this source; linking this repository from the game page
does that.

## Secure login (optional build feature)

With `--features rotl-secure-login` the Multiuser Xtra logs in with OPAQUE: the movie still passes its password to
`connectToNetServer`, but the password is never sent. The `Logon` carries a public request, the server answers, and the
password is checked here, so the server, the CDN in front of it and every log only see values that can't be turned back
into it. The messages the movie puts the password in (`LoadChar2`, `LoginAuth`) are rewritten on their way out. The movie
sees none of this.

```bash
npm run build-vm-secure          # the same as build-vm, with the feature on
```

A build without the feature contains none of the cryptography and behaves exactly as before. A build with it still
logs in the old way unless the page sets the external param `secure_login=1`, so one build can serve both kinds of
server. `rotl-opaque/` is a copy of the crate in the (private) NodeJS-ROTL repo, which owns it and the server half;
`scripts/check-rotl-opaque.sh` verifies the copy has not drifted. The wire protocol is in NodeJS-ROTL's
`docs/secure-login-protocol.md`. The logic is in `vm-rust/src/player/xtra/multiuser/secure_login.rs`, with native tests:

```bash
cd vm-rust && cargo test --lib --features rotl-secure-login secure_login
```
