# Tanoshi extensions

Prebuilt extensions (sources) for the **[Tanoshi Mobile Server APK](https://github.com/spainclaro-debug/tanoshi-termux)** — the standalone Android app that bundles the Tanoshi manga server. This is the extension repository the app can install from at runtime.

> **Android-only.** These plugins are native `aarch64-linux-android` shared objects built for the Tanoshi server bundled in the Tanoshi Mobile Server APK (Tanoshi 0.40.0 / `tanoshi-lib` 0.38.0). They **do not work** with the Termux package, the desktop/server builds of Tanoshi, or Tachiyomi/Keiyoushi/Mihon. They are not Android APKs.

## Add this repository to the app

Open the app → **Sources** tab → **Add repository**, then paste either:

- GitHub repository link: **`https://github.com/spainclaro-debug/tanoshi-extension`**
- or the direct catalog URL: **`https://raw.githubusercontent.com/spainclaro-debug/tanoshi-extension/main/android/extensions/index.json`**

The app lists the extensions below and installs/updates them over HTTPS after verifying the size and SHA-256 of each `.so`. Installing or removing an extension requires restarting the server (the app shows a **Server restart needed** prompt). Extensions are managed **only** in the app's Sources tab; the embedded Tanoshi web UI no longer offers install/uninstall actions.

## Included extensions

| Extension | Source | Kind |
| --- | --- | --- |
| MangaDex | mangadex | upstream Tanoshi extension |
| Guya | guya | upstream Tanoshi extension |
| Manhwa18.cc | manhwa18cc | upstream Tanoshi extension |
| WeebCentral | weebcentral | upstream Tanoshi extension |
| Atsumaru | atsumaru | Rust port (this repo's `sources/`) |
| QiScans | qiscans | Rust port (this repo's `sources/`) |
| MangaBall | mangaball | Rust port (this repo's `sources/`) |
| MadaraDex | madaradex | Rust port (this repo's `sources/`) |
| HentaiNexus | hentainexus | Rust port (this repo's `sources/`) |

Only Android `arm64-v8a` is supported.

## Repository layout

```
.
├── android/extensions/
│   ├── index.json                 # catalog the app fetches
│   └── plugins/lib<name>.so       # prebuilt extensions
├── tanoshi-extensions.json        # same catalog at the repo root (convenience)
├── sources/                       # Rust sources of the five ported extensions + template
├── patches/                       # patches applied to the tanoshi-extensions workspace
├── tools/build_extension.sh       # build a source into plugins/lib<name>.so
└── LICENSE
```

The app accepts this repository as a GitHub link because it looks for
`android/extensions/index.json` (or `tanoshi-extensions.json`) on `main`/`master`.

## Catalog format (`android/extensions/index.json`)

```json
{
  "version": 1,
  "updated": "2026-11-17",
  "extensions": [
    {
      "name": "mangadex",
      "displayName": "MangaDex",
      "description": "MangaDex source for the Tanoshi mobile server.",
      "url": "https://raw.githubusercontent.com/spainclaro-debug/tanoshi-extension/main/android/extensions/plugins/libmangadex.so",
      "sha256": "<64 lowercase hex characters>",
      "size": 4461728,
      "ported": false
    }
  ]
}
```

- `name` must be `^[a-z0-9][a-z0-9_-]{0,63}$`; it is also the installed file name.
- `url` must be HTTPS and point at the exact `.so`.
- `sha256` is optional in the app but should always be set here; the app verifies it when present.
- `size` must be the exact byte size.

## Porting / building a new extension

The five ported extensions are the reference implementations. To add a source:

1. Copy `sources/template/` to `sources/ported/<your-source>/` and rename the crate; implement the `Extension` trait methods (see `sources/README.md` and the ported crates).
2. Build it for Android from the repository root (Termux on the phone, or a desktop with the Android NDK and the `aarch64-linux-android` Rust target):

   ```bash
   bash tools/build_extension.sh --crate sources/ported/<your-source>
   ```

   The script clones `luigi311/tanoshi-extensions`, applies `patches/`, builds a 16 KB-aligned plugin, strips it to `android/extensions/plugins/lib<name>.so`, and prints the SHA-256, size and catalog entry.
3. Add the printed entry to `android/extensions/index.json` (and `tanoshi-extensions.json` if you keep both), commit and push.
4. In the app: **Sources → Refresh** (or re-add the repo) and install the new extension.

A plugin must be built against the **same `tanoshi-lib` version as the app's server** (0.38.0 for the 0.6/0.7 app line) and for `aarch64-linux-android`. Version mismatches are reported in the server logs and the extension is skipped.

## Updating an extension

Rebuild/replace the `.so` in `android/extensions/plugins/`, update its `sha256` and `size` in `android/extensions/index.json`, commit and push. The app will show **Update available** on the next catalog refresh.

## Security / trust

Extensions are native code that runs inside the Tanoshi server process with the server's privileges. Only install extensions you trust. The app verifies the catalog's SHA-256 but cannot audit the code. This repository is maintained by [@spainclaro-debug](https://github.com/spainclaro-debug); third-party repositories can be added in the app at your own risk.

## Credits / license

The Tanoshi extensions originate from [luigi311/tanoshi-extensions](https://github.com/luigi311/tanoshi-extensions) (MIT). The five Rust ports in `sources/ported/` are ports of Keiyoushi Tachiyomi extensions; see the source headers. This repository's `LICENSE` applies to the packaging/tooling.
