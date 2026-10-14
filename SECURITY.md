# Security

## Reporting a vulnerability

Please **do not** open a public issue for a security problem. Use GitHub's
private reporting instead: open the **Security** tab of this repository and
choose **Report a vulnerability**. Include what you found, how to reproduce it,
and which version you tested. I will reply as soon as I can.

## Status

Vault is a hobby project. The design follows well-known practice and the
cryptography comes from established libraries, but **it has not had an
independent security audit**. Please do not rely on it for secrets you cannot
afford to lose until it has been reviewed. This file explains what the app tries
to protect, what it cannot protect, and what is still unfinished.

## 1. What is protected

| Asset | Where it lives |
|---|---|
| Entries (sites, usernames, passwords, notes, 2FA keys) | Encrypted inside the vault file (`my.vault`) |
| Master password | In the user's head. Typed in, used to derive a key, never stored |
| Data key | In Rust memory while unlocked; stored in the file only in wrapped (encrypted) form |
| Recovery key | On the user's paper. Never stored by the app |

## 2. Who the app defends against

| # | Attacker | What they have | Defense |
|---|---|---|---|
| A | Thief of the vault file (lost laptop, leaked backup, cloud copy) | The file, unlimited offline guessing | Argon2id key derivation plus XChaCha20-Poly1305. Only the master password's strength matters |
| B | Someone with a minute at an unlocked, unattended device | The running app | Auto-lock (idle, leaving the app); changing the password or creating a recovery key asks for the master password again; passwords hidden by default; clipboard auto-clear |
| C | Shoulder surfer or casual screen capture | Eyes, screenshots | Android screenshot blocking and a blank recent-apps thumbnail; passwords masked until revealed |
| D | Someone who edits the vault file or a backup | Write access to the file | The header and data are authenticated: any changed byte makes opening fail |
| E | A crafted file given to Restore, Merge or Import | A malicious file | Header values are bounds-checked before any work is done, parsing is length-checked, and the parsers are tested against truncated, corrupted and random input |
| F | Someone who finds an old copy after a password change | Old file plus the old password | Changing the master password creates a new data key, so an old copy only opens the old data |

## 3. What the app cannot defend against

- **Malware or a keylogger on the device.** If something records the master password or reads memory while the vault is unlocked, encryption cannot help.
- **A compromised or rooted operating system.**
- **A weak master password.** Argon2id slows guessing; it does not stop it. Use a long passphrase.
- **Someone who has both the vault file and the recovery key.** The recovery key is as powerful as the master password.
- **A forgotten master password with no recovery key and no backup.** By design, nobody can help, including the developer.

## 4. Protections in place

- Zero-knowledge and offline: the app contains no network code, no analytics and no accounts.
- Argon2id key derivation with a random per-vault salt. New vaults are tuned to the device (64 MiB of memory and 3 to 10 passes, aiming for about half a second). The settings are stored in the file.
- A random data key encrypts the vault. The master password and the optional recovery key each wrap it separately.
- XChaCha20-Poly1305 with a fresh random nonce on every save; the whole header is authenticated.
- Atomic saves (temporary file, then rename) so a crash cannot leave a half-written vault.
- A versioned format; older files are upgraded when opened.
- Keys and decrypted data are wiped from memory with `zeroize` when dropped. The key derived from the master password is not kept after unlocking.
- The vault and its entries stay inside Rust. The Flutter layer only receives what it needs to display, and passwords only when you open or copy an entry.
- After three wrong passwords the lock screen makes the user wait (5 seconds, doubling up to 60).
- Android: the main activity blocks screenshots and marks copied passwords as sensitive, and the manifest turns off automatic app backup.

## 5. Known weaknesses and limits

1. **Clipboard history on Windows.** Copied passwords can stay in Windows clipboard history or cloud clipboard. Turn clipboard history off in Windows settings. On Android, copied text is marked sensitive, but the timed clear is best effort because Android restricts what background apps can do with the clipboard.
2. **Strings cannot be wiped.** The master password typed in Flutter and entry passwords sent to the screen are Dart strings, which cannot be reliably zeroed. In Rust, entry fields are ordinary strings that are not zeroized when dropped.
3. **Master password policy.** The minimum is 8 characters and there is no strength meter yet.
4. **Argon2 settings are fixed per vault.** Existing vaults keep the settings they were created with. Unlock time on slow phones has not been measured widely.
5. **Debug builds are slow and unhardened.** Do not keep real passwords in a debug build; test with a release build.
6. **The unlock delay is only a speed bump.** It resets when the app restarts and does nothing against someone who copies the file, so the strength of the master password is what really matters.
7. **No biometric unlock.** This is a deliberate choice, not an oversight.
8. **Single vault file.** Beyond the safety copies made before a restore, import or sync, there are no automatic rotating backups.
9. **Entry numbers are converted from 64-bit to 32-bit** at the Flutter boundary. This is harmless for a personal vault.
10. **CSV import handles plaintext.** An exported CSV lists every password as readable text. The app reads it only in memory and offers a best-effort overwrite-and-delete on desktop, but cannot guarantee the data is gone from an SSD, the Recycle Bin, a synced folder or a phone's Downloads folder. Export, import, delete, then empty the trash.
11. **Two-factor keys live next to the passwords.** Anyone who opens the vault gets both. For important accounts (email, banking), consider a separate authenticator on another device. Codes also depend on the device clock being right.
12. **Sync is manual, and copies multiply.** The encrypted file is safe to carry, but every extra copy (cloud drive, USB stick, old backup) is another place an attacker could get it and guess the master password offline. The newest edit wins by clock time, so wrong clocks can pick the wrong version. A safety copy (`.before-sync`) is kept whenever a merge changes something. Deletion records are kept for one year.
13. **Android installs.** Release APKs are not yet signed with a dedicated release key and the app id is still Flutter's placeholder, so Android may warn about an unknown developer. Build from source if you want to be sure what you are running.

## 6. Release checklist (for maintainers)

Code hygiene
- [ ] `cargo clippy --all-targets` is clean.
- [ ] `cargo audit` and `flutter pub outdated` reviewed.
- [ ] Every `unwrap()` and `expect()` in non-test Rust code reviewed.
- [x] Dependency versions pinned (`Cargo.lock`, `pubspec.lock`).

Testing
- [x] Parser hardening tests (truncation, every single-byte change, absurd settings, random garbage) for the vault file and CSV import.
- [ ] A real fuzzer (`cargo fuzz`, needs Linux or WSL).
- [ ] Manual test on a release build per platform: create, add, lock, kill the app, reopen, restore, recover, change password, sync.
- [ ] Interrupted-save test (kill the app during a save, then confirm the vault opens).

Release
- [ ] Release builds only; no debug logging of any secret.
- [ ] Android release manifest has **no INTERNET permission** (an offline app does not need it).
- [ ] APKs signed with a dedicated key, with published checksums.
- [ ] A real Android application id instead of `com.example.vault_app`.

Trust
- [x] Source code is public.
- [ ] Independent security audit.
