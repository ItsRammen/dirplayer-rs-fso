#!/bin/sh
# rotl-opaque/ is a copy of crypto/rotl-opaque in the (private) NodeJS-ROTL repo; the server and the player must run the
# same code. SHA256SUMS lists the files that must be identical in both places.
cd "$(dirname "$0")/../rotl-opaque" && sha256sum -c SHA256SUMS
