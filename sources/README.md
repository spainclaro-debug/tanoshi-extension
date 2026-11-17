# Tanoshi extension sources (exported)

This directory contains the **Rust sources of the five extensions ported to
Tanoshi for this project**, plus a template for porting more sources and a
build script. They are the basis used to build the `aarch64-linux-android`
plugins shipped in the Android app.

> The Android app itself cannot compile extensions. Build the `.so` here (or in
> Termux on the phone), publish it, then add it to an extension repository (see
> [`../repo-template`](../repo-template)) and add that repository's link in the
> app's **Sources → Add repository** dialog.

## Layout

```
sources/
├── ported/                  # Rust ports of the five Keiyoushi extensions
│   ├── atsumaru/            # Atsumaru  (id 2327480808438768017)
│   ├── hentainexus/         # HentaiNexus
│   ├── madaradex/           # MadaraDex
│   ├── mangaball/           # MangaBall
│   └── qiscans/             # QiScans
├── template/                # skeleton crate for a new port
└── README.md                # this file
```

Each extension crate is a `cdylib` that uses the shared crates from the
[`luigi311/tanoshi-extensions`](https://github.com/luigi311/tanoshi-extensions)
workspace (`extension-utils`, `networking`, and `tanoshi-lib` v0.38.0). They are
not standalone crates: they must be built inside that workspace, which is what
[`../../native/build_extension.sh`](../../native/build_extension.sh) sets up.

## Build one (or all) for Android

From the repository root, on the phone (Termux) or on a desktop with the Android
NDK and the `aarch64-linux-android` Rust target:

```bash
# all five ported extensions
bash android/native/build_extension.sh

# one crate (e.g. while porting a new source)
bash android/native/build_extension.sh --crate android/extensions/sources/template

# or point at any exported crate directory
bash android/native/build_extension.sh --crate /path/to/my-source
```

The script:

1. clones/pins `luigi311/tanoshi-extensions` and applies this repository's
   `patches/networking-helpers.patch` (+ the icon patch when present),
2. copies the crate(s) into the workspace `extensions/` directory,
3. builds with `cargo build --release` for Android (16 KB page aligned),
4. writes `lib<name>.so` stripped into `android/extensions/plugins/`, and
5. prints the SHA-256, size and a ready-to-paste catalog entry.

Note: the plugin must be built with the **same tanoshi-lib version** as the
server (0.38.0 for app releases up to 0.6.x). A version mismatch is reported by
the server logs and the extension is not loaded.

## Porting a new source

1. Copy `template/` to a new directory, e.g.
   `android/extensions/sources/ported/mysource/`.
2. Rename the crate in `Cargo.toml` (`name = "mysource"`) and in `src/lib.rs`
   (`MyExtension`, `NAME`, a **unique** `ID` — check
   [`tanoshi-extensions/docs/sources.md`](https://github.com/luigi311/tanoshi-extensions/blob/master/docs/sources.md)
   for reserved IDs).
3. Implement the `Extension` trait methods (`get_source_info`,
   `get_popular_manga`, `get_latest_manga`, `search_manga`,
   `get_manga_detail`, `get_chapters`, `get_pages`, `get_image_bytes`) using
   the `networking::RateLimitedAgent` helpers (see the ported crates for
   examples).
4. Build with `build_extension.sh`, test against the server, then publish.

## Publishing to the app (repository format)

The app installs `.so` plugins listed by a repository's JSON catalog. See
[`../repo-template`](../repo-template) for the exact layout:

- `android/extensions/index.json` — catalog (`version`, `updated`, `extensions[]`
  with `name`, `displayName`, `description`, `url`, `sha256`, `size`,
  `ported`),
- `.so` files next to it (or anywhere reachable by HTTPS — the URL is explicit
  in the catalog).

Host it on GitHub and add the repository link in the app: either the GitHub
repository URL (`https://github.com/<owner>/<repo>`, the app looks for
`android/extensions/index.json` or `tanoshi-extensions.json` on `main`/`master`)
or the direct raw `index.json` URL.

A zip snapshot of this directory is committed as
`tanoshi-extension-sources-<date>.zip` at `android/extensions/` for easy
download/sharing.
