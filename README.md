# dirplayer-rs-fso: DirPlayer for First Star Online

A fork of [igorlira/dirplayer-rs](https://github.com/igorlira/dirplayer-rs), a Shockwave (Director) player written in Rust that runs in
the browser. This fork exists to run the **FSO (First Star Online)** games, today
**Return of the Legends** (the "ROTL" in some file and branch names), in a modern browser against our own game server.

**This is not DirPlayer, and it is not meant to be merged back into it.** It carries code that only makes sense for FSO games: the way
they log in, the extra messages our server sends, and menu commands in their movie. If you want a general Shockwave player, use
[upstream](https://github.com/igorlira/dirplayer-rs): its demo, Chrome extension, standalone debugger app and Discord are there, and none of
them are built or supported from this repository.

## What is in this fork

The `rotl` branch is upstream plus one commit per change; `main` just tracks upstream.

**FSO-specific (stays here)**

| What | Where |
|---|---|
| Secure login: the Multiuser Xtra logs in with OPAQUE, so the password is never sent. Optional build feature. See below. | `vm-rust/src/player/xtra/multiuser/secure_login.rs`, `rotl-opaque/` |
| Login with a one-use ticket from the website's account page ("Continue as ...") | same Xtra |
| Name colours the movie cannot draw (VIP and Immortal names). See below. | `vm-rust/src/player/xtra/multiuser/name_colours.rs` |
| Edit menu: Log Chat, Log Combat Chat and Take Screenshot work in the browser (saved as downloads) | the `FileIO` and `ShockFiler` Xtras, `xtra/download.rs` |
| Release workflow for the polyfill | `.github/workflows/rotl-release.yml` |

**Director behaviour the movie needed (general, and could go upstream as separate pull requests)**

Most of the commits: `quit` and `put VOID`, nested chunk writes, field text (fonts, caret, line height, scrolling, `#limit` fields, per-line and
palette colours, drop shadow), `copyPixels` and sprite colours on the movie's palette, hit testing and rollover events, 16-bit sound byte order,
drag and proximity timing, dropped connections, and similar. Each commit message says what it fixes and which part of the game showed it. A fix
like this is offered upstream on its own, without anything FSO-specific in it.

If an FSO feature is ever worth sharing it should go in as an Xtra or plugin that upstream can opt into, not as a change to the player.

## Building

You need Rust with the `wasm32-unknown-unknown` target, wasm-pack 0.15 and Node 22.

```bash
npm ci
npm run build-vm            # or build-vm-secure, to include secure login
npm run build-polyfill
```

The result is `dist-polyfill/dirplayer-polyfill.js`: one file with the VM inlined. The game page (NodeJS-ROTL, the server repository, which is
private) serves it from its polyfill folder, and loads it with `<script src="dirplayer-polyfill.js">`.

We build it locally. Pushing a tag named `rotl-v*` also builds it in GitHub Actions and attaches it to a release
(`.github/workflows/rotl-release.yml`), but that build has no secure login, so we do not use it for the live game. The upstream workflows are
kept unchanged so the branch rebases cleanly; keep them disabled in this fork's Actions settings (the CDN deploy one runs on every published
release).

## Secure login (optional build feature)

With `--features rotl-secure-login` the Multiuser Xtra logs in with OPAQUE: the movie still passes its password to
`connectToNetServer`, but the password is never sent. The `Logon` carries a public request, the server answers, and the
password is checked here, so the server, the CDN in front of it and every log only see values that can't be turned back
into it. The messages the movie puts the password in (`LoadChar2`, `LoginAuth`) are rewritten on their way out. The movie
sees none of this.

An account that only has an old bcrypt hash upgrades itself the first time: the server sends the hash's salt, the player
computes the hash locally (`bcrypt`, checked against hashes made by the server's own library) and proves it, then
registers an OPAQUE record, so even that step never sends the password.

```bash
npm run build-vm-secure          # the same as build-vm, with the feature on
```

Creating a character goes through the same Xtra: the creation message is held, the new password is registered with the
server (`OpaqueCreate`, `OpaqueCreateResponse`, `OpaqueCreateFinish`), and the message leaves with a marker where the
password was. The account is secure-login only and has no password hash at all.

A build without the feature contains none of the cryptography and behaves exactly as before. A build with it still
logs in the old way unless the page sets the external param `secure_login=1`, so one build can serve both kinds of
server. `rotl-opaque/` is a copy of the crate in the NodeJS-ROTL repository, which owns it and the server half;
`scripts/check-rotl-opaque.sh` verifies the copy has not drifted. The wire protocol is in NodeJS-ROTL's
`docs/secure-login-protocol.md`. The logic is in `vm-rust/src/player/xtra/multiuser/secure_login.rs`, with native tests:

```bash
cd vm-rust && cargo test --lib --features rotl-secure-login secure_login
```

## Name colours

The movie draws a player's name label in one of three colours. The server can also send a `NameColours` message
(`name=RRGGBB;name=RRGGBB`, the whole list each time); the Multiuser Xtra keeps it instead of passing it to the movie
(`vm-rust/src/player/xtra/multiuser/name_colours.rs`). When the movie sets the colour of a name label (a field or text member named
`Sprite<channel>Name`) the label is given the name's colour if the name is in the list, and a label that still carries such a colour for a
name that has none is put back to white. No server, or no message, and nothing changes.

```bash
cd vm-rust && cargo test --lib name_colours
```

## Updating from upstream

```bash
git fetch upstream
git rebase upstream/main rotl
```

A conflict shows up on the commit whose change it touches. This `README.md` is ours: on a conflict keep this version and leave upstream's
out. Rebuild and play-test before tagging.

## License and credits

GPL-3.0, as upstream (see `LICENSE`). Anyone serving the built polyfill to players must offer this source; linking this repository from the
game page does that.

All the Shockwave work is [Igor Lira's DirPlayer](https://github.com/igorlira/dirplayer-rs) and the reverse-engineering community behind it,
whose projects upstream credits and this fork builds on:
[Earthquake-Project/Format-Documentation](https://github.com/Earthquake-Project/Format-Documentation/),
[OpenShockwave](https://github.com/Brian151/OpenShockwave/), [ScummVM](https://www.scummvm.org/),
[earthquake-rust](https://github.com/csnover/earthquake-rust/) and [ProjectorRays](https://github.com/ProjectorRays/ProjectorRays).
