# Third-party notices

## Keiyoushi extensions (Apache-2.0)

The Rust crates under `sources/ported/` are ports of source extensions from
[`keiyoushi/extensions-source`](https://github.com/keiyoushi/extensions-source),
which is licensed under the Apache License, Version 2.0. The ported files keep
the original extension and site names in their module headers, and the original
licence text is reproduced in [`sources/LICENSE-APACHE-2.0`](sources/LICENSE-APACHE-2.0).

Ported sources, with their upstream modules:

| Crate | Upstream source |
| --- | --- |
| `atsumaru` | `src/en/atsumaru` |
| `qiscans` | `src/en/qiscans` (EZManhwa theme) |
| `mangaball` | `src/en/mangaball` |
| `madaradex` | `src/en/madaradex` (Madara theme) |
| `hentainexus` | `src/en/hentainexus` |
| `mangapill` | `src/en/mangapill` |
| `comicklive` | `src/all/comicklive` |
| `flamecomics` | `src/en/flamecomics` |
| `mangaread` | `src/en/mangareadorg` (Madara theme) |
| `mangasushi` | `src/en/mangasushi` (Madara theme) |

The ports are re-implementations in Rust: no Kotlin code is vendored and the
plugins do not include the upstream APK/JAR artifacts.

## Tanoshi extensions (MIT)

The upstream extensions `mangadex`, `guya`, `manhwa18cc` and `weebcentral` come
from [`luigi311/tanoshi-extensions`](https://github.com/luigi311/tanoshi-extensions),
which is MIT licensed.
