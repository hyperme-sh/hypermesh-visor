# Official WireGuardKit source

Source: https://git.zx2c4.com/wireguard-apple
Revision: `2fec12a6e1f6e3460b6ee483aa00ad29cddadab1`
License: MIT, retained in `COPYING` and source headers.

Copied paths: `Package.swift`, `COPYING`, `Sources/WireGuardKit`,
`Sources/WireGuardKitC`, and `Sources/WireGuardKitGo`.

Local compatibility changes only:
- Raise the manifest tools version from 5.3 to 5.9 so its platform constants exist.
- Include `<sys/types.h>` in `WireGuardKitC.h` before its Darwin type declarations.

The consuming app still builds and links the upstream Go library. These sources
are not a linked VPN extension and do not enable signing or app entitlements.
