# Sync

UwUNews keeps sources, collections and read/starred marks on every device of
one person. Articles themselves are never synced: every device fetches its
feeds itself, and a mark finds its article by a key every device computes the
same way.

There are two backends, one at a time per device:

- **UwUSync**: the UwUSuite sync server UwUSSH and UwURDP use. UwUNews can
  create its own account there or join an account one of them made, and it
  shares that account's vault with them.
- **UwULock**: a UwULock Server account. The records live in the account's
  `news` space, sealed with the space key.

Everything is end-to-end encrypted. The server sees ids, kinds, clocks and
sealed bytes, and nothing else.

## Crates

| Crate | What it holds |
|---|---|
| `uwunews-proto` | Wire types: `Envelope`, `EntityKind`, payloads, manifests, `SCHEMA_VERSION` = 2 |
| `uwunews-vault` | Keys and seals: vault header, KDF, account key, `Flavor`, record AEAD |
| `uwunews-store` | SQLite plus a `SecretStore`: records, dirty tracking, apply, manifests, enrolment |
| `uwunews-sync` | Engine, HTTP with pinning, setup codes, SPAKE2 pairing, the UwULock backend, `client::SyncClient` |

Every key and token is read and written **only** through the
`uwunews_store::SecretStore` trait. That covers the vault key, device key,
account key, UwULock refresh token and remember tokens. On iOS this trait is
the Keychain, and in tests it is `MemorySecrets`. SQLite never holds a secret.

## Synced kinds

| Kind | Discriminant | Record id |
|---|---|---|
| `news_source` | 40 | random UUIDv7 |
| `news_collection` | 41 | random UUIDv7 |
| `news_mark` | 42 | `mark_id(article_key)` |

- UwUSSH and UwURDP use the discriminants from 0 upward, and 40 to 42 stay
  clear of them.
- Apply order is source, then collection, then mark.
- Unknown kinds are skipped, never rejected, in both directions.

Two derived keys let every device agree on a mark without talking to each
other:

- `article_key(source_id, guid)` is the hex SHA-256 of
  `"uwunews/article/v1\0" + source id (hyphenated, lowercase) + "\0" + guid`.
- `mark_id(article_key)` is a UUIDv8 built from the first 16 bytes of
  SHA-256(`"uwunews/mark/v1" + article key`).

Because the id is derived, every device that marks an article writes the same
record, and the newer HLC wins. A mark can arrive before its article does. It
waits in `news_marks` and applies when the article is fetched.

### Deletes and pruning

- A delete becomes a tombstone, and a tombstone beats every edit.
- When a source is deleted, its cached articles go with it.
- When old articles are pruned, their marks are pruned too, and the last
  version given up is noted in `superseded`. An older or equal version that
  arrives later stays dropped, and the manifest check does not count it as
  missing.
- Starred marks of a deleted source are not swept from other devices. See
  Risks.

## Flavors: joining UwUSSH and UwURDP accounts

An account's vault seals with labels chosen by the app that made it:

- `Flavor::Ssh` uses the `uwussh/…` labels.
- `Flavor::Rdp` uses the `uwurdp/…` labels.

The labels cover the record AAD, the login key, the manifest id and the
pairing transcript (`pairing/v2/a-to-b`, `pairing/v2/b-to-a`,
`pairing/confirm`, `pair/a`, `pair/b`). For `Ssh` they are byte-for-byte what
UwUSSH uses.

Which flavor applies:

- **Creating an account** in UwUNews makes an `Ssh` vault. UwUSSH devices can
  join it as if UwUSSH had made it.
- **Joining** takes the flavor as a parameter: `Ssh` for an account UwUNews or
  UwUSSH made, `Rdp` for a UwURDP one. The pairing code does not carry the
  flavor, so the UI has to ask which app made the account. A wrong flavor
  makes the SPAKE2 confirmation fail cleanly. Nothing is adopted and nothing
  leaks.
- **After joining**, the vault header carries the flavor, and every later seal
  and manifest uses it.
- **UwULock spaces** are UwUNews's own and use `Ssh` labels.

### Manifests and compatibility

- Each device publishes a manifest of the record versions it has confirmed.
- After a complete pull, each device checks the other manifests, which is how
  it notices a server that withholds or replays records.
- A UwUSSH or UwURDP device's manifest lists kinds UwUNews has no table for.
  Those entries are not checked, and those apps skip the news kinds in
  UwUNews manifests the same way.
- **There is no cross-app alarm**: each app checks only what it keeps.
- The manifest id uses the vault's flavor label, so the other app accepts a
  UwUNews manifest as well formed.

### Server relay

A UwUSync server relays kinds beyond its 0.2 set only to clients that ask for
them. UwUNews pulls with:

```
GET …/pull?since=<cursor>&limit=<n>&manifests=1&assist=1
```

`assist=1` tells the server that UwUNews skips any kind it does not know,
rather than failing the pull. This lets one vault carry SSH hosts, RDP
connections and news sources side by side.

## UwUSync flow

1. **Create.** The server admin hands out a setup code (`uwu1_…`), and
   `create_account(setup_code, password)` does the rest:
   - It creates the vault (KDF `RECOMMENDED`, `Flavor::Ssh`, vault id
     `Uuid::now_v7()`) and a device key.
   - It returns the account recovery code, which the app shows once.
   - Any records that existed on the device move into the new vault and are
     marked dirty.
2. **Offer pairing.** `offer_pairing()` opens a postbox and returns an offer
   in two forms:
   - spoken: `K7M4Q-tiger-radio-kiwi`
   - pasteable: `uwu2_…`, which also carries the server URL and the TLS pin

   `wait_for_device(offer, timeout)` then runs the SPAKE2 handshake and hands
   over the account key.
3. **Join.** `join(code, server, password, flavor)` accepts either form. A
   spoken code needs `server = Some((url, fingerprint))`. The steps are:
   1. Take over the postbox and receive the account key.
   2. Derive the login key for the flavor.
   3. Fetch and unlock the vault header.
   4. Enrol this device and adopt the vault, setting the manifest floor.
   5. Report back that this device has joined.
4. **Reconnect** uses the device key and needs no password. **Unlock**
   (`unlock(password)`) brings the vault key back when the secret store has
   lost it.

### TLS

- Every request pins the server certificate's SHA-256 when a pin is known.
  Without a pin, the certificate must chain to a trusted root.
- Desktop trusts webpki-roots plus the OS trust store
  (`rustls-native-certs`).
- iOS and Android trust **only webpki-roots plus the pin**, so a server behind
  a private CA has to be paired with the pasteable code that carries the pin.

## UwULock flow

`connect_lock(LockLogin { server_url, email, password, two_factor, accept_space })`
mirrors the UwURDP desktop:

- It sends the remember token only to the server and email that issued it.
- The outcome is one of:
  - `SignedIn { made_space }`: the device calls `store.join_space(space, enrolment)`,
    which adopts the space as the vault and moves local records into it. This
    also forgets any UwUSync pairing.
  - `TwoFactor { methods, message }`: ask for a code and sign in again. For
    email codes, `send_lock_email_code` asks the server to send one.
  - `SpaceChanged { was, now }`: nothing changed on the device. Ask the user
    and retry with `accept_space`. Any new remember token is kept regardless.
- The refresh token rotates on every refresh. The keeper stores the new token
  through `keep_lock_refresh_token` before it is used.
- When a sync fails with `TransportError::SignIn`, the client calls
  `end_lock_session()`, drops the cached connection and returns
  `ClientError::SignInAgain`. UwULock stays the backend.
- `lock_connection()` hands the `Lock` to `lock::live::run` for realtime
  pushes.

Identity on the server:

- client id `uwunews`, space `news`, device name `UwUNews`
- records live at `/uwu/v1/suite/spaces/news/records`

### UwULock server patch required

The UwULock server has to know the new client before any of this works:

- `suite.rs`: add `"uwunews" => "news"` to `space_of_client`, add `news` to
  `SPACES`, and allow kinds `news_source`, `news_collection` and `news_mark`
  for that space.
- `sso.rs`: add `uwunews` to `SUITE_CLIENTS`.
- `uwulock-mail`: add an app name for client `uwunews` and space `news`.

## API: `uwunews_sync::client::SyncClient`

Every call blocks, so the app should run them on a thread of its own.

```rust
let client = SyncClient::new(Arc<Store>, "iPhone");

client.status()?                // Status { backend, vault, pending, last_sync_ms }
//   backend: None | UwuSync { server_url, account_id }
//                 | UwuLock { server_url, email, signed_in }
client.create_account(setup_code, pw)? -> String         // recovery code
client.join(code, Option<(url, Option<pin>)>, pw, Flavor)?
client.offer_pairing()? -> PairingOffer
client.wait_for_device(&offer, timeout)? -> Joined
client.unlock(pw)?
client.reconnect()?
client.connect_lock(&LockLogin { .. })? -> LockOutcome
client.send_lock_email_code(url, email, pw)?
client.lock_connection()? -> Option<Arc<Lock>>
client.sync_once()? -> Synced { pushed, pulled, conflicts, report }
client.disconnect()?            // keys and tokens go, data stays
```

`ClientError` variants:

- `NotConnected`, `BadCode`, `NeedsServer`, `VaultLocked`, `SignInAgain`
- wrapped `Flow`, `Lock`, `Sync`, `Transport`, `Store` and `Vault` errors

The lower layers are public too, for tests and tools: `engine::sync_once`
against any `Transport`, `MemoryServer`, `flow::*` and `pairing::*`.

## Switching backends

- A device uses one backend at a time.
- Joining a UwULock space forgets the UwUSync enrolment. Creating or joining a
  UwUSync account ends UwULock mode.
- Either way the local records are re-sealed into the new vault and pushed.
  **Nothing checks that the new account already holds them**, so a switch is a
  copy, not a verified move.

## Tests

- Unit tests sit next to the code:
  - pairing runs both flavors over a Condvar relay;
  - the HTTP query and the pin.
- The integration binary is `crates/uwunews-sync/tests/integration`. It holds
  devices against `MemoryServer`, covering:
  - sync of sources, collections and marks;
  - conflicts, deletes, and a device that joins later;
  - Rdp interop, and a wrong flavor that rejects everything;
  - client status, disconnect and errors in both modes, with the move from
    UwUSync to UwULock.
- No test touches the network or sleeps.

## Risks and open points

- **One vault key per account.** UwUNews, UwUSSH and UwURDP devices on one
  account all hold the same vault key. A compromised device of any of the
  three apps can read every record of all of them.
- **`SecretStore` is infallible.** `set` and `delete` return nothing. A
  Keychain write that fails silently (a locked device, an access group
  mistake) loses a rotated refresh token, and the next sync then asks for a
  sign-in.
- **Starred marks of deleted sources.** A deleted source does not tombstone
  its marks, so they stay on the server and in the manifests until they are
  pruned.
- **`disconnect()` also drops the UwULock remember token**, so the next
  sign-in asks for two-step login again.
- **The flow functions are untested end to end.** `create_account`, `join`,
  `offer_pairing` and `connect_lock` need a real server, and the tests may not
  use the network. Their parts (pairing, setup and pairing code parsing, wire
  conversion, the store primitives and the engine) are tested on their own.
  An end-to-end test against a local UwUSync and UwULock server is still to
  do.
- **The UwULock server patch** above is required. Without it the server
  refuses client `uwunews`.
