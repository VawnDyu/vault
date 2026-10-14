# Vault

An **offline password manager** for Android and Windows. Your passwords are
encrypted on your own device, there are no accounts and no servers, and the app
never needs the internet.

Built by [Vawndyu](https://github.com/VawnDyu) with a **Rust** core and a
**Flutter** interface.

> **Status: hobby project, not independently audited.** The design follows
> well-known practice and the code has tests, but nobody outside this project
> has reviewed it. Please do not rely on it for secrets you cannot afford to
> lose until it has had a proper security review. See [SECURITY.md](SECURITY.md).

## Features

- Encrypted vault stored in one file on your device
- Categories (Social media, Banking, Email, Shopping, ...), search and filters
- Password generator with length and character options
- Two-factor (TOTP) codes next to each password
- Password health check for reused and weak passwords
- Recovery key and encrypted backups
- Import from Chrome, Edge, Firefox and Bitwarden CSV exports
- Manual sync between phone and PC by merging encrypted vault files
- Auto-lock, clipboard clearing, light/dark themes

## How your data is protected

- The master password is turned into a key with **Argon2id** (slow and
  memory-hungry, so guessing is expensive).
- The vault is encrypted with **XChaCha20-Poly1305**, which also detects any
  tampering with the file.
- A random data key is stored wrapped by the master password, and optionally
  by a recovery key, so changing the password does not re-expose old copies.
- The app contains no network code. Nothing is sent anywhere.
- Nobody, including the developer, can reset a forgotten master password.

The details, the threat model, and the known weaknesses are in
[SECURITY.md](SECURITY.md).

## Project layout

```
vault_core/   Rust library: encryption, vault format, TOTP, import, sync merge
vault_app/    Flutter app
  rust/       Small Rust crate that exposes vault_core to Flutter
  lib/        The user interface (Dart)
```

`vault_app/rust` depends on `../../vault_core`, so keep the two folders side by
side.

## Building

Get the code:

```
git clone https://github.com/VawnDyu/vault.git
cd vault
```

You need:

- [Rust](https://rustup.rs)
- [Flutter](https://docs.flutter.dev/get-started/install)
- For Windows builds: Visual Studio Build Tools with "Desktop development with C++"
- For Android builds: Android Studio with the SDK, NDK and command-line tools
- `flutter_rust_bridge_codegen` (`cargo install flutter_rust_bridge_codegen`)

Then:

```
cd vault_app
flutter pub get
flutter run -d windows        # or: flutter run -d <your phone's id>
```

The Android app id is still Flutter's placeholder (`com.example.vault_app`). Change it, and sign with your own key, before you publish builds of your own.

The Flutter-to-Rust glue is generated and included in the repository. If you
change `vault_app/rust/src/api/vault_api.rs`, regenerate it:

```
cd vault_app
flutter_rust_bridge_codegen generate
```

### Tests

```
cd vault_core
cargo test
```

### Third-party licenses

The credits page lists Flutter and Dart packages automatically. The Rust
libraries are added from `assets/licenses/rust_licenses.txt`, generated with
[`cargo about`](https://github.com/EmbarkStudios/cargo-about):

```
cd vault_app/rust
cargo about generate about.hbs -o ../assets/licenses/rust_licenses.txt
```

## Using sync

Sync is manual and offline. On one device choose **Settings > Sync with another
device > Send my vault**, move the encrypted file to the other device, and
choose **Merge a vault file** there. The newest edit to each entry wins, and
deletions are carried across. The first time, set up the second device with
**Restore from backup** so both start from the same vault.

## Roadmap

- Scan 2FA QR codes with the camera
- Optional automatic sync (without needing a server)
- Protection against Windows clipboard history
- A dedicated Android release key and application id
- An independent security review

## Reporting problems

Bugs and ideas are welcome as [issues](../../issues). For anything that could be
a security problem, please follow [SECURITY.md](SECURITY.md) and report it
privately instead.

## License

Released under the [MIT License](LICENSE). Thanks to the authors of the
open-source libraries this app is built on; they are credited inside the app
under **Settings > Open-source licenses**.
