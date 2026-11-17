# Tanoshi extensions

Prebuilt extensions (sources) for the **[Tanoshi Mobile Server APK](https://github.com/spainclaro-debug/tanoshi-termux)** — the standalone Android app that bundles the Tanoshi manga server. This is the extension repository the app can install from at runtime.

> **Android-only.** These plugins are native `aarch64-linux-android` shared objects built for the Tanoshi server bundled in the Tanoshi Mobile Server APK (Tanoshi 0.40.0 / `tanoshi-lib` 0.38.0). They **do not work** with the Termux package, the desktop/server builds of Tanoshi, or Tachiyomi/Keiyoushi/Mihon. They are not Android APKs.

## Add this repository to the app

Open the app → **Sources** tab → **Add repository**, then paste either:

- GitHub repository link: **`https://github.com/spainclaro-debug/tanoshi-extension`**
- or the direct catalog URL: **`https://raw.githubusercontent.com/spainclaro-debug/tanoshi-extension/main/android/extensions/index.json`**

The app lists the extensions below and installs/updates them over HTTPS after verifying the size and SHA-256 of each `.so`. Installing or removing an extension requires restarting the server (the app shows a **Server restart needed** prompt). Extensions are managed **only** in the app's Sources tab; the embedded Tanoshi web UI no longer offers install/uninstall actions.

## Included extensions

| Extension | Name | Kind |
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
| MangaPill | mangapill | Rust port (this repo's `sources/`) |
| Comick | comicklive | Rust port (this repo's `sources/`) |
| Flame Comics | flamecomics | Rust port (this repo's `sources/`) |
| MangaRead.org | mangaread | Rust port (this repo's `sources/`) |
| Mangasushi | mangasushi | Rust port (this repo's `sources/`) |

Only Android `arm64-v8a` is supported.

The Rust ports are ports of [Keiyoushi](https://github.com/keiyoushi/extensions-source) Tachiyomi extensions. Every port in this repository was exercised end-to-end against the live site before being published: popular/latest/search listings, manga details, chapter lists, page lists and a real image download, through the same plugin ABI the server uses. Sites change and break over time; if a source stops working, update its port or open an issue.

## Repository layout

```
.
├── android/extensions/
│   ├── index.json                 # catalog the app fetches
│   └── plugins/lib<name>.so       # prebuilt extensions
├── tanoshi-extensions.json        # same catalog at the repo root (convenience)
├── sources/
│   ├── ported/<name>/             # Rust source of each ported extension
│   └── template/                  # skeleton crate for porting a new source
├── patches/                       # patches applied to the tanoshi-extensions workspace
├── tools/
│   ├── build_extension.sh         # build a source into plugins/lib<name>.so
│   ├── catalog-meta.json          # human catalog fields (single source of truth)
│   └── update_catalog.py          # regenerate both catalogs from the plugins
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

Do not edit the catalogs by hand. Edit `tools/catalog-meta.json` (display names,
descriptions) if needed, drop the rebuilt `.so` into `android/extensions/plugins/`,
then regenerate both catalogs with the hashes and sizes filled in:

```bash
python3 tools/update_catalog.py          # rewrite android/extensions/index.json and tanoshi-extensions.json
python3 tools/update_catalog.py --check  # verify the catalogs match the plugins (CI/pre-commit)
```

## Porting / building a new extension

The ported extensions in `sources/ported/` are the reference implementations. To add a source:

1. Copy `sources/template/` to `sources/ported/<your-source>/` and rename the crate; implement the `Extension` trait methods (see `sources/README.md` and the ported crates).
2. Build it for Android from the repository root (Termux on the phone, or a desktop with the Android NDK and the `aarch64-linux-android` Rust target):

   ```bash
   bash tools/build_extension.sh --crate sources/ported/<your-source>
   ```

   The script clones `luigi311/tanoshi-extensions`, applies `patches/`, builds a 16 KB-aligned plugin, strips it to `android/extensions/plugins/lib<name>.so`, and prints the SHA-256, size and catalog entry.
3. Add the extension to `tools/catalog-meta.json` (a `name` entry with `displayName`, `description`, `ported: true`), then run `python3 tools/update_catalog.py`.
4. Commit and push. In the app: **Sources → Refresh** (or re-add the repo) and install the new extension.

A plugin must be built against the **same `tanoshi-lib` version as the app's server** (0.38.0 for the 0.6/0.7 app line) and for `aarch64-linux-android`. Version mismatches are reported in the server logs and the extension is skipped.

## Updating an extension

Rebuild/replace the `.so` in `android/extensions/plugins/`, then run
`python3 tools/update_catalog.py` (it refreshes `sha256`/`size`), commit and push.
The app will show **Update available** on the next catalog refresh.

## Security / trust

Extensions are native code that runs inside the Tanoshi server process with the server's privileges. Only install extensions you trust. The app verifies the catalog's SHA-256 but cannot audit the code. This repository is maintained by [@spainclaro-debug](https://github.com/spainclaro-debug); third-party repositories can be added in the app at your own risk.

## Credits / license

The upstream Tanoshi extensions originate from [luigi311/tanoshi-extensions](https://github.com/luigi311/tanoshi-extensions). The Rust ports in `sources/ported/` are ports of [Keiyoushi](https://github.com/keiyoushi/extensions-source) Tachiyomi extensions, which are Apache-2.0 licensed; see each port's module header and [`THIRD_PARTY_NOTICES.md`](THIRD_PARTY_NOTICES.md). This repository's own packaging and tooling are MIT licensed (see [`LICENSE`](LICENSE)).
