# UwUNews — Plan

> RSS- und Atom-Reader der UwUSuite. Quellen, Sammlungen, Filter-Feeds,
> Benachrichtigungen pro Quelle/Sammlung, Sync über UwUSync oder UwULock.

| | |
|---|---|
| **Plattform** | iOS 26 zuerst (SwiftUI, Liquid Glass). Windows später (Tauri, gleicher Rust-Kern). |
| **Bundle-ID** | `app.uwunews.ios` |
| **Kern** | Rust-Workspace `crates/`, für iOS per UniFFI als XCFramework (`bindings/swift`) |
| **Sprache** | Deutsch + Englisch (String Catalog) |

## Crates

| Crate | Inhalt |
|---|---|
| `uwunews-feed` | Abrufen (ETag/Last-Modified), RSS/Atom/JSON-Feed parsen, Filterregeln, OPML, Feed-Erkennung aus HTML |
| `uwunews-proto` | Wire-Format, HLC, Konfliktregel, Record-Arten (aus UwURDP übernommen) |
| `uwunews-vault` | Master-Passwort, Account-Key, Versiegeln (aus UwURDP) |
| `uwunews-store` | SQLite: synchronisierte Records (Quellen, Sammlungen, Markierungen) + lokaler Artikel-Cache |
| `uwunews-sync` | UwUSync-Server (Setup-Code, Pairing per SPAKE2) und UwULock-Server |
| `uwunews-ffi` | UniFFI-Oberfläche `Engine` für Swift |

## Datenmodell

Synchronisiert (versiegelt, HLC, Tombstones):

- `news_source` — URL, eigener Titel, Benachrichtigung an/aus
- `news_collection` — Name, Symbol, Farbe, Quellen-IDs oder „alle Quellen“, Filterregeln
  (Stichwörter beliebig/alle, Ausschlusswörter, Suchbereich), Benachrichtigung an/aus, Reihenfolge
- `news_mark` — gelesen/gemerkt pro Artikel (Artikel-Schlüssel = Hash aus Quelle + GUID)

Nur lokal: Artikel, Abrufstatus (ETag, letzter Fehler), bereits gemeldete Artikel.

## Benachrichtigungen (iOS)

`BGAppRefreshTask` (iOS entscheidet über den Zeitpunkt) plus Abruf beim Öffnen und Pull-to-refresh.
Eine Meldung, wenn ein neuer Artikel aus einer Quelle mit Benachrichtigung kommt oder in eine
Sammlung mit Benachrichtigung fällt. Beim ersten Abruf einer Quelle wird nichts gemeldet.

## Meilensteine

- [x] M1 Kern: Feed, Store, FFI, Tests
- [x] M2 iOS-App: Übersicht, Sammlungen, Filter, Leser, Einstellungen, OPML, Benachrichtigungen
- [x] M3 Sync: UwUSync + UwULock im Kern und in den Einstellungen
  - offen: UwULock-Server muss den Client `uwunews` auf den Bereich `news` abbilden
- [ ] M4 Windows (Tauri)
