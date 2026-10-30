# UwUNews

RSS-/Atom-Reader der UwUSuite: Rust-Kern (UniFFI) plus iOS-26-App in SwiftUI; Windows später per Tauri.

## Layout
- `crates/uwunews-feed`: Feeds holen, parsen, filtern (RSS/Atom/JSON, OPML); Parser für fremde Daten
- `crates/uwunews-proto`: gemeinsame Typen Client/Sync-Server
- `crates/uwunews-vault`: Schlüsselableitung, Verschlüsselung der Records
- `crates/uwunews-store`: lokale SQLite (Quellen, Sammlungen, Marks, Artikelcache, Sync-Status)
- `crates/uwunews-sync`: Offline-first-Sync (Outbox, Clocks, Konflikte); Backends UwUSync/UwULock
- `crates/uwunews-ffi`: UniFFI-Engine für die Apps
- `bindings/swift`: Swift-Paket `UwUNewsCore`; `ios/`: App (XcodeGen, `ios/project.yml`)
- `docs/plan.md` (Plan/Stand), `docs/sync.md` (Sync-Design); kein CHANGELOG, noch keine Tags

## Befehle
- Gezielt: `cargo test -p uwunews-<crate> [filter]`; Integrationstests in `crates/{feed,ffi,store,sync}/tests/integration`
- iOS (nur Mac mit Xcode 27, XcodeGen): `scripts/build-apple.sh` -> `cd ios && xcodegen generate` -> Build/Simulator
- Nur Bindings neu erzeugen: `scripts/bindings.sh`
- `scripts/strings.py` (Übersetzungen), `scripts/art.py` (Nyu-Sticker, App-Icon; braucht cairosvg)

## Fallen
- Nie von Hand editieren: `bindings/swift/Sources/UwUNewsCore/UwUNewsCore.swift` (in git, von `scripts/bindings.sh` erzeugt; geändert -> mitcommitten)
- Generiert und gitignored: `ios/UwUNews.xcodeproj/` (Änderungen in `ios/project.yml`), `bindings/swift/build/` (XCFramework), `build/`
- Nach Änderung an `uwunews-ffi` (UniFFI-Schnittstelle) immer `scripts/build-apple.sh` laufen lassen, sonst passen Swift-Bindings und Rust-Lib nicht zusammen
- `uwulock-core` ist per Git-`rev` in `Cargo.toml` gepinnt (gleicher Commit wie UwURDP); beide gemeinsam anheben
- iOS-Build und Xcode laufen nicht auf dem Linux-Host; dort nur Rust prüfen
- CI (`.github/workflows/ci.yml`) ist ein Rust-Job ohne Swift-Build und ohne `--locked`; Swift-Änderungen werden dort nicht geprüft

## Security
- Kritisch: `crates/uwunews-vault` (Krypto), `crates/uwunews-sync` (Sync-Protokoll, Konflikte), `crates/uwunews-feed` (Parser für fremde Feeds/OPML), Login-Pfad mit `uwulock-core`
- Vor jedem Feature-Release den globalen Subagent `security-reviewer` über den Bereich seit dem letzten Tag laufen lassen; Ergebnis nach `docs/security-review-<version>.md`
