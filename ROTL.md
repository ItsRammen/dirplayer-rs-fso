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
