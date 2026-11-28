# Third-party notices

## uBlock Origin scriptlet resources

This distribution includes `src-tauri/resources/ublock/scriptlets.js`, derived from the
uBlock Origin resource file at revision
[`56b8201196a02e4d6c31bce7c168ad1c961ed63e`](https://github.com/gorhill/uBlock/tree/56b8201196a02e4d6c31bce7c168ad1c961ed63e/assets/resources).
Copyright (C) 2019-present Raymond Hill. It is licensed under GNU GPL version 3 or, at your
option, any later version. The complete license is provided in [LICENSE](./LICENSE).

The vendored resource format is pinned because the `adblock` crate's resource assembler supports
uBlock Origin's legacy text-template format, not the current ES-module scriptlet sources. Update
the source revision and generated resource bundle together when upgrading this integration. The
runtime filter list can reference resources newer than this bundle; unsupported or unresolved
scriptlet resources are not injected.

The application distribution is licensed under GPL-3.0-or-later.

## Oneko cat sprites

The 32 Oneko GIF frames in `src-tauri/resources/oneko/` are reused from
[`glreno/oneko`](https://github.com/glreno/oneko), whose fork is released under
the Unlicense. The images are public domain. The behavior is independently
ported to JavaScript from the fork's state machine; the original Java desktop
port was written by Werner Randelshofer (2010), and the fork's Java code was
written by Gerald Reno (2019).

## uBlock filters

The browser downloads the uBlock filters list from
[`uBlockOrigin/uAssets`](https://github.com/uBlockOrigin/uAssets) at runtime. The list identifies
its license in its header and is provided under GPL-3.0.

The built-in YouTube streaming rules in `src-tauri/src/adblock/mod.rs` adapt a small subset of
uBlock filters from [`uBlockOrigin/uAssets`](https://github.com/uBlockOrigin/uAssets), also
licensed under GPL-3.0.
