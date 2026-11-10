# Vendored xterm.js

These files are served by the daemon from the binary (`STATIC` in
`crates/daemon/src/http/assets.rs`), so the web UI loads no script or
stylesheet from a CDN. They were taken from the npm registry tarballs below,
each checked against the registry's `dist.integrity` before unpacking. The
only change is that the trailing `//# sourceMappingURL=` comment was removed
from the three scripts; no source maps are shipped.

| Package | Version | Registry `dist.integrity` |
|---|---|---|
| `xterm` | 5.3.0 | `sha512-8QqjlekLUFTrU6x7xck1MsPzPA571K5zNqWm0M0oroYEWVOptZ0+ubQSkQ3uxIEhcIHRujJy6emDWX4A7qyFzg==` |
| `@xterm/addon-fit` | 0.10.0 | `sha512-UFYkDm4HUahf2lnEyHvio51TNGiLK66mqP2JoATy7hRZeXaGMRDr00JiSF7m63vR5WKATF605yEggJKsw0JpMQ==` |
| `@xterm/addon-canvas` | 0.7.0 | `sha512-LF5LYcfvefJuJ7QotNRdRSPc9YASAVDeoT5uyXS/nZshZXjYplGXRECBGiznwvhNL2I8bq1Lf5MzRwstsYQ2Iw==` |

| File | From | sha256 |
|---|---|---|
| `xterm.js` | `xterm/lib/xterm.js` (minified UMD bundle) | `07391aefa8d9c04855b7f2e54b47b4a8cc3baf219ad69e6a7f11727a8869841a` |
| `xterm.css` | `xterm/css/xterm.css` | `832f3f2c603b43ad4351ff04970150cc7a873014276db126a6065c6dd81e4872` |
| `addon-fit.js` | `@xterm/addon-fit/lib/addon-fit.js` | `3e9ae6f6965ee1269d5574b905cf1bf559c2f44c784255b0034a71387e7719c9` |
| `addon-canvas.js` | `@xterm/addon-canvas/lib/addon-canvas.js` | `47a9a1a3266bc06d2c9f63aaa2716a36f5fd7fe509a1995b0d8748b0251f91d1` |
| `LICENSE.xterm` | `xterm/LICENSE` (MIT) | `b569f629d00f2626a8100df2a1798210535621e42164dfd426a6fe5aac7b0ccd` |
| `LICENSE.addon-fit` | `@xterm/addon-fit/LICENSE` (MIT) | `e256f01188af527e4d06d21d06fbf785ae9c50d4b328bf03cbe0ba7f0aa4228f` |
| `LICENSE.addon-canvas` | `@xterm/addon-canvas/LICENSE` (MIT) | `689ceb10716bd7a87a804c14951d7c2b818cbd60aab83d540902470ce3b13444` |

To upgrade: fetch `https://registry.npmjs.org/<name>/<version>`, download
`dist.tarball`, verify it against `dist.integrity`, copy the same files,
strip the source map comment, and update both tables and the paths in
`assets/js/singleton.js`.
