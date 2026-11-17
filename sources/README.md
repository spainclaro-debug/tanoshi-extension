# Tanoshi extension sources

This directory contains the **Rust sources of the extensions ported to Tanoshi
for this project**, plus a template for porting more sources. They are built
into the `aarch64-linux-android` plugins shipped in [`../android/extensions/plugins`](../android/extensions/plugins).

> The Android app itself cannot compile extensions. Build the `.so` here (or in
> Termux on the phone), publish it, then add it to this repository's catalog and
> install it in the app's **Sources** tab.

## Layout

```
sources/
├── ported/
│   ├── atsumaru/       # Atsumaru  (id 2327480808438768017)
│   ├── comicklive/     # Comick (Unoriginal, comick.live) (id 4972933717624256217)
│   ├── flamecomics/    # Flame Comics (id 8531542650987673943)
│   ├── hentainexus/    # HentaiNexus
│   ├── madaradex/      # MadaraDex (Madara theme, with the mdx auth handshake)
│   ├── mangaball/      # MangaBall (id 1448906013733277368)
│   ├── mangapill/      # MangaPill (id 8448310129093543312)
│   ├── mangaread/      # MangaRead.org (Madara theme) (id 2681651342533848540)
│   ├── mangasushi/     # Mangasushi (Madara theme) (id 3606721916149050760)
│   └── qiscans/        # QiScans (id 5806040666300479660)
├── template/           # skeleton crate for a new port
└── README.md           # this file
```

Every crate is a `cdylib` that uses the shared crates from the
[`luigi311/tanoshi-extensions`](https://github.com/luigi311/tanoshi-extensions)
workspace (`extension-utils`, `networking`, and `tanoshi-lib` v0.38.0). They are
not standalone crates: they must be built inside that workspace, which is what
[`../tools/build_extension.sh`](../tools/build_extension.sh) sets up.

## Build one (or all) for Android

From the repository root, on the phone (Termux) or on a desktop with the Android
NDK and the `aarch64-linux-android` Rust target:

```bash
# all ported extensions
bash tools/build_extension.sh

# one crate (e.g. while porting a new source)
bash tools/build_extension.sh --crate sources/ported/<name>

# or point at any exported crate directory
bash tools/build_extension.sh --crate /path/to/my-source
```

The script:

1. clones/pins `luigi311/tanoshi-extensions` and applies this repository's
   `patches/networking-helpers.patch` (+ the icon patch when present),
2. copies the crate(s) into the workspace `extensions/` directory,
3. builds with `cargo build --release` for Android (16 KB page aligned),
4. writes `lib<name>.so` stripped into `android/extensions/plugins/`, and
5. prints the SHA-256, size and a ready-to-paste catalog entry.

Useful environment variables: `EXTENSIONS_SRC_DIR` (reuse an existing
`tanoshi-extensions` checkout) and `EXTENSIONS_TARGET_DIR` (reuse a warm cargo
target directory), e.g. to skip the initial dependency build.

Note: the plugin must be built with the **same tanoshi-lib version** as the
server (0.38.0 for app releases up to 0.7.x). A version mismatch is reported by
the server logs and the extension is not loaded.

## Verify a port against the live site

A port is only published after it works through the plugin ABI the server uses:
`get_popular_manga`, `get_latest_manga`, `search_manga`, `get_manga_detail`,
`get_chapters`, `get_pages` and `get_image_bytes`. A small loader that exercises
these methods directly against the built `.so` is the quickest way to test:

```bash
# build the harness once from the tanoshi-lib sources, then
./tanoshi-harness android/extensions/plugins/lib<name>.so <a-manga-path>
```

It prints list counts, the manga detail, chapter and page counts, and the byte
size of a real image download. (The project's internal harness lives outside
this repository; `sources/template/` and the ported crates document the expected
behaviour for each method.)

## Porting a new source

1. Copy `template/` to a new directory under `ported/`, e.g.
   `sources/ported/mysource/`.
2. Rename the crate in `Cargo.toml` (`name = "mysource"`) and in `src/lib.rs`
   (`MyExtension`, `NAME`, a **unique** `ID` — use the Keiyoushi source id from
   [`keiyoushi/extensions`](https://github.com/keiyoushi/extensions); check
   [`tanoshi-extensions/docs/sources.md`](https://github.com/luigi311/tanoshi-extensions/blob/master/docs/sources.md)
   for reserved ids).
3. Implement the `Extension` trait methods (`get_source_info`,
   `get_popular_manga`, `get_latest_manga`, `search_manga`,
   `get_manga_detail`, `get_chapters`, `get_pages`, `get_image_bytes`) using
   the `networking::RateLimitedAgent` helpers (see the ported crates for
   examples).
4. Build with `tools/build_extension.sh --crate sources/ported/mysource`, verify
   it against the live site, then add the extension to
   `tools/catalog-meta.json` and run `python3 tools/update_catalog.py`.
5. Commit the source, the plugin and the regenerated catalogs.

## Licensing

The ports are derivative works of [Keiyoushi](https://github.com/keiyoushi/extensions-source)
extensions (Apache-2.0); see [`../THIRD_PARTY_NOTICES.md`](../THIRD_PARTY_NOTICES.md)
and [`LICENSE-APACHE-2.0`](LICENSE-APACHE-2.0).
