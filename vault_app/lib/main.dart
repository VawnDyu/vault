// Put this file at: vault_app/lib/main.dart  (replace everything in it)
//
// Before running (see the instructions in the chat):
//   - update vault_core (lib.rs, import.rs) and the bridge (vault_api.rs)
//   - flutter_rust_bridge_codegen generate

import 'dart:async';
import 'dart:io';
import 'dart:ui' as ui;

import 'package:file_picker/file_picker.dart' as fp;
import 'package:file_selector/file_selector.dart';
import 'package:flutter/foundation.dart';
import 'package:flutter/material.dart';
import 'package:flutter/rendering.dart';
import 'package:flutter/services.dart';
import 'package:path_provider/path_provider.dart';
import 'package:shared_preferences/shared_preferences.dart';
import 'package:url_launcher/url_launcher.dart';
import 'package:vault_app/src/rust/api/vault_api.dart';
import 'package:vault_app/src/rust/frb_generated.dart';

// ---------------------------------------------------------------------------
// Settings (saved on the device; these are preferences, not secrets).
// ---------------------------------------------------------------------------
const List<int> autoLockChoices = [1, 2, 5, 10, 15, 30]; // minutes
const List<int> clipboardChoices = [15, 30, 60]; // seconds

/// Shown on the About card and lock screen. Keep it in step with `version:`
/// in pubspec.yaml.
const String kAppVersion = '1.4.0';
const String kDeveloper = 'Vawndyu';
const String kGitHubUrl = 'https://github.com/VawnDyu';

class AppSettings extends ChangeNotifier {
  int autoLockMinutes = 5;
  int clipboardSeconds = 30;
  int genLength = 20;
  bool genSymbols = true;
  bool genExcludeAmbiguous = false;
  ThemeMode themeMode = ThemeMode.system;

  /// Fires only when the theme choice changes (not for every setting), so
  /// the whole app is not rebuilt when you drag a slider.
  final ValueNotifier<ThemeMode> themeModeListenable =
      ValueNotifier(ThemeMode.system);
  bool welcomeSeen = false;
  String lastSeenVersion = '';

  SharedPreferences? _prefs;

  Future<void> load() async {
    final p = _prefs = await SharedPreferences.getInstance();
    final lock = p.getInt('autoLockMinutes') ?? 5;
    final clip = p.getInt('clipboardSeconds') ?? 30;
    autoLockMinutes = autoLockChoices.contains(lock) ? lock : 5;
    clipboardSeconds = clipboardChoices.contains(clip) ? clip : 30;
    genLength = (p.getInt('genLength') ?? 20).clamp(8, 64);
    genSymbols = p.getBool('genSymbols') ?? true;
    genExcludeAmbiguous = p.getBool('genExcludeAmbiguous') ?? false;
    themeMode = ThemeMode.values[(p.getInt('themeMode') ?? 0).clamp(0, 2)];
    themeModeListenable.value = themeMode;
    welcomeSeen = p.getBool('welcomeSeen') ?? false;
    lastSeenVersion = p.getString('lastSeenVersion') ?? '';
  }

  Future<void> update({
    int? autoLockMinutes,
    int? clipboardSeconds,
    int? genLength,
    bool? genSymbols,
    bool? genExcludeAmbiguous,
    ThemeMode? themeMode,
    bool? welcomeSeen,
    String? lastSeenVersion,
  }) async {
    if (autoLockMinutes != null) this.autoLockMinutes = autoLockMinutes;
    if (clipboardSeconds != null) this.clipboardSeconds = clipboardSeconds;
    if (genLength != null) this.genLength = genLength;
    if (genSymbols != null) this.genSymbols = genSymbols;
    if (genExcludeAmbiguous != null) {
      this.genExcludeAmbiguous = genExcludeAmbiguous;
    }
    if (themeMode != null) {
      this.themeMode = themeMode;
      themeModeListenable.value = themeMode;
    }
    if (welcomeSeen != null) this.welcomeSeen = welcomeSeen;
    if (lastSeenVersion != null) this.lastSeenVersion = lastSeenVersion;
    notifyListeners();

    final p = _prefs;
    if (p == null) return;
    await p.setInt('autoLockMinutes', this.autoLockMinutes);
    await p.setInt('clipboardSeconds', this.clipboardSeconds);
    await p.setInt('genLength', this.genLength);
    await p.setBool('genSymbols', this.genSymbols);
    await p.setBool('genExcludeAmbiguous', this.genExcludeAmbiguous);
    await p.setInt('themeMode', this.themeMode.index);
    await p.setBool('welcomeSeen', this.welcomeSeen);
    await p.setString('lastSeenVersion', this.lastSeenVersion);
  }
}

final AppSettings settings = AppSettings();

// ---------------------------------------------------------------------------
// Categories: how entries are grouped (Social media, Banking, ...).
// The id is what is stored in the vault; label, icon and color are only for
// display, so you can change them freely without touching saved data.
// ---------------------------------------------------------------------------
class EntryCategory {
  final String id;
  final String label;
  final IconData icon;
  final Color color;

  const EntryCategory(this.id, this.label, this.icon, this.color);
}

const List<EntryCategory> kCategories = [
  EntryCategory('social', 'Social media', Icons.groups_rounded, Color(0xFF3B82F6)),
  EntryCategory('bank', 'Banking & finance', Icons.account_balance_rounded, Color(0xFF10B981)),
  EntryCategory('email', 'Email', Icons.mail_rounded, Color(0xFFF59E0B)),
  EntryCategory('shopping', 'Shopping', Icons.shopping_bag_rounded, Color(0xFFEC4899)),
  EntryCategory('entertainment', 'Entertainment', Icons.movie_rounded, Color(0xFFEF4444)),
  EntryCategory('gaming', 'Gaming', Icons.sports_esports_rounded, Color(0xFF14B8A6)),
  EntryCategory('work', 'Work', Icons.work_rounded, Color(0xFF8B5CF6)),
  EntryCategory('other', 'Other', Icons.folder_rounded, Color(0xFF64748B)),
];

/// Looks up a category by its stored id. Empty or unknown ids show as "Other".
EntryCategory categoryFor(String id) {
  return kCategories.firstWhere(
    (c) => c.id == id,
    orElse: () => kCategories.last,
  );
}


/// True while a system file dialog is open. On phones, opening the file
/// picker sends our app to the background, which would otherwise auto-lock
/// the vault in the middle of an export or restore.
bool pickerOpen = false;

/// Some items chosen in a file dialog (for example a file browsed to directly
/// on a phone connected by USB, or an online-only cloud file) have no real
/// path on this computer, so they cannot be read.
void _ensureReadable(XFile file) {
  if (file.path.isEmpty) {
    throw 'That file cannot be read directly. Copy it onto this computer '
        'first (for example into your Downloads folder), then choose the copy.';
  }
}

/// File type shown in the backup save/open dialogs.
const XTypeGroup csvTypeGroup = XTypeGroup(
  label: 'CSV file',
  extensions: <String>['csv'],
);

const XTypeGroup vaultTypeGroup = XTypeGroup(
  label: 'Vault backup',
  extensions: <String>['vault'],
);

/// Flutter's licenses page automatically lists Flutter and the Dart packages.
/// The Rust libraries are not known to Flutter, so their notices (generated
/// with `cargo about`, see the steps in the chat) are added here.
void registerRustLicenses() {
  LicenseRegistry.addLicense(() async* {
    String raw;
    try {
      raw = await rootBundle.loadString('assets/licenses/rust_licenses.txt');
    } catch (_) {
      return; // file missing: show only Flutter's own list
    }
    // The file is a series of blocks, each starting with "@@@ " followed by a
    // comma-separated list of libraries, a line break, then the license text.
    for (final block in raw.split('@@@ ')) {
      final cut = block.indexOf('\n');
      if (cut < 0) continue;
      final packages = block
          .substring(0, cut)
          .split(',')
          .map((p) => p.trim())
          .where((p) => p.isNotEmpty)
          .toList();
      final text = block.substring(cut + 1).trim();
      if (packages.isEmpty || text.isEmpty) continue;
      yield LicenseEntryWithLineBreaks(packages, text);
    }
  });
}

Future<void> main() async {
  WidgetsFlutterBinding.ensureInitialized();
  registerRustLicenses();
  await RustLib.init();
  await settings.load();
  runApp(const VaultApp());
}

// ---------------------------------------------------------------------------
// Clipboard helper: copy a secret and wipe it after the chosen delay
// (only if the clipboard still holds that same secret).
// Kept at top level so locking the vault doesn't cancel the wipe.
// ---------------------------------------------------------------------------
Timer? _clipboardTimer;

/// Talks to MainActivity.kt so Android treats the copied text as sensitive
/// (hidden from the clipboard preview popup and from clipboard history).
const MethodChannel _clipboardChannel = MethodChannel('vault/clipboard');

Future<void> copyWithAutoClear(String secret) async {
  if (Platform.isAndroid) {
    await _clipboardChannel.invokeMethod<void>('copySensitive', secret);
  } else {
    await Clipboard.setData(ClipboardData(text: secret));
  }
  _clipboardTimer?.cancel();
  _clipboardTimer = Timer(Duration(seconds: settings.clipboardSeconds), () async {
    final current = await Clipboard.getData(Clipboard.kTextPlain);
    if (current?.text == secret) {
      await Clipboard.setData(const ClipboardData(text: ''));
    }
  });
}

// ---------------------------------------------------------------------------
// Soft theme change. Fading every color on screen is slow on phones, so we
// do it the cheap way: take a photo of the current screen, switch the theme
// underneath instantly, then fade the photo away. Only one picture is faded,
// so it stays smooth even on modest phones.
// ---------------------------------------------------------------------------
final GlobalKey<_ThemeCrossFadeState> _themeFadeKey =
    GlobalKey<_ThemeCrossFadeState>();

/// Change the theme with a soft fade. Use this instead of calling
/// `settings.update(themeMode: ...)` directly.
Future<void> changeTheme(ThemeMode mode) async {
  if (mode == settings.themeMode) return;
  final fader = _themeFadeKey.currentState;
  if (fader == null) {
    await settings.update(themeMode: mode);
    return;
  }
  await fader.crossFadeTo(() => settings.update(themeMode: mode));
}

class ThemeCrossFade extends StatefulWidget {
  final Widget child;

  const ThemeCrossFade({super.key, required this.child});

  @override
  State<ThemeCrossFade> createState() => _ThemeCrossFadeState();
}

class _ThemeCrossFadeState extends State<ThemeCrossFade>
    with SingleTickerProviderStateMixin {
  final GlobalKey _boundaryKey = GlobalKey();
  late final AnimationController _fade = AnimationController(
    vsync: this,
    duration: const Duration(milliseconds: 300),
  );
  late final Animation<double> _opacity =
      CurvedAnimation(parent: _fade, curve: Curves.easeInOut);
  ui.Image? _photo;
  bool _busy = false;

  @override
  void dispose() {
    _fade.dispose();
    _photo?.dispose();
    super.dispose();
  }

  /// Photograph the screen, run [apply] (which switches the theme), then fade
  /// the photo away. If anything goes wrong, the theme still switches.
  Future<void> crossFadeTo(Future<void> Function() apply) async {
    if (_busy) {
      await apply(); // a fade is already running: just switch
      return;
    }
    _busy = true;
    try {
      ui.Image? image;
      try {
        final boundary = _boundaryKey.currentContext?.findRenderObject()
            as RenderRepaintBoundary?;
        final ratio = View.of(context).devicePixelRatio;
        image = await boundary?.toImage(pixelRatio: ratio);
      } catch (_) {
        image = null; // fall back to an instant switch
      }
      if (image == null || !mounted) {
        image?.dispose();
        await apply();
        return;
      }

      setState(() {
        _photo?.dispose();
        _photo = image;
      });
      _fade.value = 1.0;

      await apply(); // new theme is painted underneath the photo
      await WidgetsBinding.instance.endOfFrame;
      if (!mounted) return;
      await _fade.reverse(from: 1.0);
      if (!mounted) return;
      setState(() {
        _photo?.dispose();
        _photo = null;
      });
    } finally {
      _busy = false;
    }
  }

  @override
  Widget build(BuildContext context) {
    return Stack(
      textDirection: TextDirection.ltr,
      children: [
        RepaintBoundary(key: _boundaryKey, child: widget.child),
        if (_photo != null)
          Positioned.fill(
            child: IgnorePointer(
              child: FadeTransition(
                opacity: _opacity,
                child: RawImage(image: _photo, fit: BoxFit.fill),
              ),
            ),
          ),
      ],
    );
  }
}

class VaultApp extends StatelessWidget {
  const VaultApp({super.key});

  // Built once, so changing a setting never rebuilds the themes.
  static final ThemeData _light = _theme(Brightness.light);
  static final ThemeData _dark = _theme(Brightness.dark);

  static ThemeData _theme(Brightness brightness) {
    final scheme = ColorScheme.fromSeed(
      seedColor: const Color(0xFF4F46E5),
      brightness: brightness,
    );
    OutlineInputBorder border([Color? color, double width = 0]) =>
        OutlineInputBorder(
          borderRadius: BorderRadius.circular(16),
          borderSide: color == null
              ? BorderSide.none
              : BorderSide(color: color, width: width),
        );

    return ThemeData(
      useMaterial3: true,
      colorScheme: scheme,
      scaffoldBackgroundColor: scheme.surface,
      appBarTheme: AppBarTheme(
        centerTitle: false,
        backgroundColor: scheme.surface,
        scrolledUnderElevation: 0,
        titleTextStyle: TextStyle(
          fontSize: 24,
          fontWeight: FontWeight.w700,
          color: scheme.onSurface,
        ),
      ),
      inputDecorationTheme: InputDecorationTheme(
        filled: true,
        fillColor: scheme.surfaceContainerHighest,
        contentPadding:
            const EdgeInsets.symmetric(horizontal: 16, vertical: 16),
        border: border(),
        enabledBorder: border(),
        focusedBorder: border(scheme.primary, 2),
        errorBorder: border(scheme.error, 1),
        focusedErrorBorder: border(scheme.error, 2),
      ),
      filledButtonTheme: FilledButtonThemeData(
        style: FilledButton.styleFrom(
          minimumSize: const Size(64, 52),
          shape: RoundedRectangleBorder(
            borderRadius: BorderRadius.circular(16),
          ),
          textStyle: const TextStyle(fontSize: 16, fontWeight: FontWeight.w600),
        ),
      ),
      outlinedButtonTheme: OutlinedButtonThemeData(
        style: OutlinedButton.styleFrom(
          minimumSize: const Size(64, 48),
          shape: RoundedRectangleBorder(
            borderRadius: BorderRadius.circular(16),
          ),
        ),
      ),
      cardTheme: CardThemeData(
        elevation: 0,
        margin: EdgeInsets.zero,
        color: scheme.surfaceContainerLow,
        shape: RoundedRectangleBorder(
          borderRadius: BorderRadius.circular(20),
        ),
      ),
      dialogTheme: DialogThemeData(
        shape: RoundedRectangleBorder(
          borderRadius: BorderRadius.circular(28),
        ),
      ),
      chipTheme: ChipThemeData(
        shape: const StadiumBorder(),
        side: BorderSide(color: scheme.outlineVariant),
        showCheckmark: false,
      ),
      snackBarTheme: SnackBarThemeData(
        behavior: SnackBarBehavior.floating,
        shape: RoundedRectangleBorder(borderRadius: BorderRadius.circular(14)),
      ),
      floatingActionButtonTheme: FloatingActionButtonThemeData(
        shape: RoundedRectangleBorder(borderRadius: BorderRadius.circular(18)),
      ),
    );
  }

  @override
  Widget build(BuildContext context) {
    return ThemeCrossFade(
      key: _themeFadeKey,
      child: ValueListenableBuilder<ThemeMode>(
        valueListenable: settings.themeModeListenable,
        builder: (context, mode, _) => MaterialApp(
          title: 'Vault',
          debugShowCheckedModeBanner: false,
          theme: _light,
          darkTheme: _dark,
          themeMode: mode,
          // The soft fade is done by ThemeCrossFade above (one cheap picture
          // fading out). Flutter's own theme animation blends every color on
          // screen and stutters on many phones, so it is switched off.
          themeAnimationDuration: Duration.zero,
          home: const Gate(),
        ),
      ),
    );
  }
}

// ---------------------------------------------------------------------------
// Gate: decides between the lock screen and the vault screen.
// ---------------------------------------------------------------------------
class Gate extends StatefulWidget {
  const Gate({super.key});

  @override
  State<Gate> createState() => _GateState();
}

class _GateState extends State<Gate> {
  String? _path;
  bool _unlocked = false;

  @override
  void initState() {
    super.initState();
    _init();
  }

  Future<void> _init() async {
    final dir = await getApplicationSupportDirectory();
    await dir.create(recursive: true);
    setState(() => _path = '${dir.path}${Platform.pathSeparator}my.vault');
  }

  /// Lock now: switch the screen first, close any open dialogs/screens,
  /// then wipe the key inside Rust.
  Future<void> _lock() async {
    if (!_unlocked) return; // already locked (can be called twice)
    setState(() => _unlocked = false);
    Navigator.of(context).popUntil((route) => route.isFirst);
    await lockVault();
  }

  @override
  Widget build(BuildContext context) {
    final path = _path;
    if (path == null) {
      return const Scaffold(body: Center(child: CircularProgressIndicator()));
    }
    if (_unlocked) {
      return AutoLock(
        onLock: _lock,
        child: VaultScreen(onLock: _lock, vaultPath: path),
      );
    }
    return LockScreen(
      path: path,
      onUnlocked: () => setState(() => _unlocked = true),
    );
  }
}

// ---------------------------------------------------------------------------
// AutoLock: locks after inactivity (time comes from Settings), and when the
// app goes to the background or is minimized. Clicking another window does
// NOT lock, so you can still copy a password and paste it elsewhere.
// ---------------------------------------------------------------------------
class AutoLock extends StatefulWidget {
  final VoidCallback onLock;
  final Widget child;

  const AutoLock({super.key, required this.onLock, required this.child});

  @override
  State<AutoLock> createState() => _AutoLockState();
}

class _AutoLockState extends State<AutoLock> with WidgetsBindingObserver {
  Timer? _timer;

  @override
  void initState() {
    super.initState();
    WidgetsBinding.instance.addObserver(this);
    HardwareKeyboard.instance.addHandler(_onKey);
    _reset();
  }

  bool _onKey(KeyEvent event) {
    _reset();
    return false; // we only watch; never swallow the key
  }

  void _reset() {
    _timer?.cancel();
    _timer = Timer(Duration(minutes: settings.autoLockMinutes), widget.onLock);
  }

  @override
  void didChangeAppLifecycleState(AppLifecycleState state) {
    if (pickerOpen) return;
    if (state == AppLifecycleState.paused ||
        state == AppLifecycleState.hidden) {
      widget.onLock();
    }
  }

  @override
  void dispose() {
    _timer?.cancel();
    HardwareKeyboard.instance.removeHandler(_onKey);
    WidgetsBinding.instance.removeObserver(this);
    super.dispose();
  }

  @override
  Widget build(BuildContext context) {
    return Listener(
      behavior: HitTestBehavior.translucent,
      onPointerDown: (_) => _reset(),
      onPointerMove: (_) => _reset(),
      onPointerSignal: (_) => _reset(),
      child: widget.child,
    );
  }
}

// ---------------------------------------------------------------------------
// Lock screen: create a new vault, or unlock the existing one.
// ---------------------------------------------------------------------------
class LockScreen extends StatefulWidget {
  final String path;
  final VoidCallback onUnlocked;

  const LockScreen({super.key, required this.path, required this.onUnlocked});

  @override
  State<LockScreen> createState() => _LockScreenState();
}

class _LockScreenState extends State<LockScreen> {
  final _pw = TextEditingController();
  final _pw2 = TextEditingController();
  bool _busy = false;
  bool _show = false;
  String? _error;
  late final bool _creating = !File(widget.path).existsSync();

  // After 3 wrong passwords in a row, make the user wait a little. This slows
  // down guessing by tapping on the screen. It does NOT protect against
  // someone who copies the vault file (see SECURITY.md), and it resets when
  // the app restarts.
  int _failures = 0;
  int _waitSeconds = 0;
  Timer? _waitTimer;

  @override
  void dispose() {
    _pw.dispose();
    _pw2.dispose();
    _waitTimer?.cancel();
    super.dispose();
  }

  void _startWait() {
    if (_failures < 3) return;
    final step = (_failures - 3).clamp(0, 4);
    final seconds = (5 * (1 << step)).clamp(5, 60);
    setState(() => _waitSeconds = seconds);
    _waitTimer?.cancel();
    _waitTimer = Timer.periodic(const Duration(seconds: 1), (t) {
      if (!mounted) {
        t.cancel();
        return;
      }
      setState(() => _waitSeconds -= 1);
      if (_waitSeconds <= 0) t.cancel();
    });
  }

  /// "Forgot password": recover with the recovery key and set a new password.
  Future<void> _forgot() async {
    final result = await Navigator.of(context).push<NewKeyResult>(
      MaterialPageRoute(builder: (_) => RecoverScreen(path: widget.path)),
    );
    if (result == null || !mounted) return;
    final newKey = result.newKey;
    if (newKey != null) {
      await showDialog<void>(
        context: context,
        barrierDismissible: false,
        builder: (_) => RecoveryKeyDialog(recoveryKey: newKey, replaced: true),
      );
    }
    if (mounted) widget.onUnlocked();
  }

  Future<void> _submit() async {
    if (_busy || _waitSeconds > 0) return;
    if (_creating) {
      if (_pw.text.length < 8) {
        setState(() => _error = 'Use at least 8 characters (longer is better).');
        return;
      }
      if (_pw.text != _pw2.text) {
        setState(() => _error = 'The two passwords do not match.');
        return;
      }
    }
    setState(() {
      _busy = true;
      _error = null;
    });
    try {
      if (_creating) {
        await createVault(path: widget.path, password: _pw.text);
      } else {
        await unlockVault(path: widget.path, password: _pw.text);
      }
      _pw.clear();
      _pw2.clear();
      widget.onUnlocked();
    } catch (e) {
      if (!mounted) return;
      setState(() {
        _error = e.toString();
        _busy = false;
      });
      if (!_creating) {
        _failures++;
        _startWait();
      }
    }
  }

  @override
  Widget build(BuildContext context) {
    final theme = Theme.of(context);
    final scheme = theme.colorScheme;
    final waiting = _waitSeconds > 0;

    return Scaffold(
      body: SafeArea(
        child: Center(
          child: SingleChildScrollView(
            padding: const EdgeInsets.fromLTRB(24, 24, 24, 24),
            keyboardDismissBehavior: ScrollViewKeyboardDismissBehavior.onDrag,
            child: ConstrainedBox(
              constraints: const BoxConstraints(maxWidth: 420),
              child: Column(
                mainAxisSize: MainAxisSize.min,
                crossAxisAlignment: CrossAxisAlignment.stretch,
                children: [
                  Center(
                    child: Container(
                      width: 88,
                      height: 88,
                      decoration: BoxDecoration(
                        color: scheme.primaryContainer,
                        borderRadius: BorderRadius.circular(28),
                      ),
                      child: Icon(
                        Icons.lock_rounded,
                        size: 42,
                        color: scheme.onPrimaryContainer,
                      ),
                    ),
                  ),
                  const SizedBox(height: 24),
                  Text(
                    _creating ? 'Create your vault' : 'Welcome back',
                    textAlign: TextAlign.center,
                    style: theme.textTheme.headlineMedium
                        ?.copyWith(fontWeight: FontWeight.w700),
                  ),
                  const SizedBox(height: 8),
                  Text(
                    _creating
                        ? 'Choose a strong master password. If you forget it, '
                            'nobody can recover your vault, unless you set up '
                            'a recovery key later.'
                        : 'Enter your master password to unlock.',
                    textAlign: TextAlign.center,
                    style: theme.textTheme.bodyMedium
                        ?.copyWith(color: scheme.onSurfaceVariant),
                  ),
                  const SizedBox(height: 28),
                  TextField(
                    controller: _pw,
                    obscureText: !_show,
                    autofocus: true,
                    enabled: !_busy,
                    autocorrect: false,
                    enableSuggestions: false,
                    enableIMEPersonalizedLearning: false,
                    textInputAction:
                        _creating ? TextInputAction.next : TextInputAction.done,
                    decoration: InputDecoration(
                      labelText: 'Master password',
                      prefixIcon: const Icon(Icons.key_rounded),
                      suffixIcon: IconButton(
                        tooltip: _show ? 'Hide' : 'Show',
                        icon: Icon(_show
                            ? Icons.visibility_off_rounded
                            : Icons.visibility_rounded),
                        onPressed: () => setState(() => _show = !_show),
                      ),
                    ),
                    onSubmitted: (_) => _creating ? null : _submit(),
                  ),
                  if (_creating) ...[
                    const SizedBox(height: 12),
                    TextField(
                      controller: _pw2,
                      obscureText: !_show,
                      enabled: !_busy,
                      autocorrect: false,
                      enableSuggestions: false,
                      enableIMEPersonalizedLearning: false,
                      decoration: const InputDecoration(
                        labelText: 'Repeat master password',
                        prefixIcon: Icon(Icons.key_rounded),
                      ),
                      onSubmitted: (_) => _submit(),
                    ),
                  ],
                  if (_error != null) ...[
                    const SizedBox(height: 14),
                    Container(
                      padding: const EdgeInsets.all(12),
                      decoration: BoxDecoration(
                        color: scheme.errorContainer,
                        borderRadius: BorderRadius.circular(14),
                      ),
                      child: Row(
                        crossAxisAlignment: CrossAxisAlignment.start,
                        children: [
                          Icon(Icons.error_outline_rounded,
                              color: scheme.onErrorContainer, size: 20),
                          const SizedBox(width: 10),
                          Expanded(
                            child: Text(
                              _error!,
                              style: TextStyle(color: scheme.onErrorContainer),
                            ),
                          ),
                        ],
                      ),
                    ),
                  ],
                  const SizedBox(height: 20),
                  FilledButton(
                    onPressed: (_busy || waiting) ? null : _submit,
                    child: _busy
                        ? const SizedBox(
                            height: 22,
                            width: 22,
                            child: CircularProgressIndicator(strokeWidth: 2.5),
                          )
                        : Text(
                            waiting
                                ? 'Too many tries. Wait ${_waitSeconds}s'
                                : (_creating ? 'Create vault' : 'Unlock'),
                          ),
                  ),
                  if (!_creating) ...[
                    const SizedBox(height: 4),
                    TextButton(
                      onPressed: _busy ? null : _forgot,
                      child: const Text('Forgot password? Use recovery key'),
                    ),
                  ],
                  // const SizedBox(height: 20),
                  // Text(
                  //   'Vault file: ${widget.path}',
                  //   textAlign: TextAlign.center,
                  //   style: theme.textTheme.bodySmall
                  //       ?.copyWith(color: scheme.outline),
                  // ),
                  const SizedBox(height: 6),
                  Text(
                    'Vault v$kAppVersion · by $kDeveloper',
                    textAlign: TextAlign.center,
                    style: theme.textTheme.bodySmall
                        ?.copyWith(color: scheme.outline),
                  ),
                ],
              ),
            ),
          ),
        ),
      ),
    );
  }
}

// ---------------------------------------------------------------------------
// Vault screen: search, category filter, list, add, edit, delete, settings.
// ---------------------------------------------------------------------------
class VaultScreen extends StatefulWidget {
  final VoidCallback onLock;
  final String vaultPath;

  const VaultScreen({super.key, required this.onLock, required this.vaultPath});

  @override
  State<VaultScreen> createState() => _VaultScreenState();
}

class _VaultScreenState extends State<VaultScreen> {
  List<EntrySummary> _entries = [];
  bool _loading = true;
  String _query = '';
  String? _filter; // null = show everything, otherwise a category id

  @override
  void initState() {
    super.initState();
    _refresh();
    WidgetsBinding.instance.addPostFrameCallback((_) => _maybeShowIntro());
  }

  void _toast(String message) {
    if (!mounted) return;
    ScaffoldMessenger.of(context)
      ..hideCurrentSnackBar()
      ..showSnackBar(SnackBar(content: Text(message)));
  }

  /// First launch: the welcome tour. After an update: what's new.
  Future<void> _maybeShowIntro() async {
    if (!mounted) return;
    if (!settings.welcomeSeen) {
      await showWelcome(context);
    } else if (settings.lastSeenVersion != kAppVersion) {
      await showWhatsNew(context);
    }
  }

  String _catId(EntrySummary e) => categoryFor(e.category).id;

  /// How many entries each category has (only categories in use are listed).
  Map<String, int> get _counts {
    final counts = <String, int>{};
    for (final e in _entries) {
      counts.update(_catId(e), (n) => n + 1, ifAbsent: () => 1);
    }
    return counts;
  }

  /// Entries matching the search box and category chip, sorted by site.
  List<EntrySummary> get _filtered {
    final q = _query.trim().toLowerCase();
    final list = _entries.where((e) {
      if (_filter != null && _catId(e) != _filter) return false;
      if (q.isEmpty) return true;
      return e.site.toLowerCase().contains(q) ||
          e.username.toLowerCase().contains(q);
    }).toList();
    list.sort((a, b) => a.site.toLowerCase().compareTo(b.site.toLowerCase()));
    return list;
  }

  Future<void> _refresh() async {
    try {
      final list = await listEntries();
      if (!mounted) return;
      setState(() {
        _entries = list;
        _loading = false;
        // If the chosen category became empty, go back to "All".
        if (_filter != null && !_counts.containsKey(_filter)) _filter = null;
      });
    } catch (e) {
      _toast(e.toString());
    }
  }

  Future<void> _copyPassword(EntrySummary e) async {
    try {
      final pw = await getEntryPassword(id: e.id);
      await copyWithAutoClear(pw);
      _toast(
        'Password copied. The clipboard clears in '
        '${settings.clipboardSeconds} seconds.',
      );
    } catch (err) {
      _toast(err.toString());
    }
  }

  Future<void> _copyUsername(EntrySummary e) async {
    await Clipboard.setData(ClipboardData(text: e.username));
    _toast('Username copied.');
  }

  Future<void> _delete(EntrySummary e) async {
    final ok = await showDialog<bool>(
      context: context,
      builder: (ctx) => AlertDialog(
        title: const Text('Delete this entry?'),
        content: Text('"${e.site}" will be deleted permanently.'),
        actions: [
          TextButton(
            onPressed: () => Navigator.pop(ctx, false),
            child: const Text('Cancel'),
          ),
          FilledButton(
            onPressed: () => Navigator.pop(ctx, true),
            child: const Text('Delete'),
          ),
        ],
      ),
    );
    if (ok != true) return;
    try {
      await deleteEntry(id: e.id);
      await _refresh();
    } catch (err) {
      _toast(err.toString());
    }
  }

  Future<void> _add() async {
    final saved = await Navigator.of(context).push<bool>(
      MaterialPageRoute(builder: (_) => EntryScreen(presetCategory: _filter)),
    );
    if (saved == true) await _refresh();
  }

  Future<void> _edit(EntrySummary e) async {
    try {
      final details = await getEntry(id: e.id);
      if (!mounted) return;
      final saved = await Navigator.of(context).push<bool>(
        MaterialPageRoute(builder: (_) => EntryScreen(existing: details)),
      );
      if (saved == true) await _refresh();
    } catch (err) {
      _toast(err.toString());
    }
  }

  Future<void> _openCodes() async {
    await Navigator.of(context).push(
      MaterialPageRoute<void>(builder: (_) => const CodesScreen()),
    );
  }

  Future<void> _openHealth() async {
    await Navigator.of(context).push(
      MaterialPageRoute<void>(builder: (_) => const HealthScreen()),
    );
    // Passwords may have been changed from the health page.
    if (mounted) await _refresh();
  }

  Future<void> _openSettings() async {
    await Navigator.of(context).push(
      MaterialPageRoute<void>(
        builder: (_) => SettingsScreen(vaultPath: widget.vaultPath),
      ),
    );
    // The vault may have been replaced by a restored backup or an import.
    if (mounted) await _refresh();
  }

  Widget _filterChips() {
    final counts = _counts;
    final used = kCategories.where((c) => counts.containsKey(c.id)).toList();
    return SizedBox(
      height: 44,
      child: ListView(
        scrollDirection: Axis.horizontal,
        padding: const EdgeInsets.symmetric(horizontal: 16),
        children: [
          Padding(
            padding: const EdgeInsets.only(right: 8),
            child: ChoiceChip(
              label: Text('All  ${_entries.length}'),
              selected: _filter == null,
              onSelected: (_) => setState(() => _filter = null),
            ),
          ),
          for (final c in used)
            Padding(
              padding: const EdgeInsets.only(right: 8),
              child: ChoiceChip(
                avatar: Icon(c.icon, size: 18, color: c.color),
                label: Text('${c.label}  ${counts[c.id]}'),
                selected: _filter == c.id,
                onSelected: (_) => setState(() => _filter = c.id),
              ),
            ),
        ],
      ),
    );
  }

  Widget _entryCard(EntrySummary e) {
    final theme = Theme.of(context);
    final cat = categoryFor(e.category);
    return Padding(
      padding: const EdgeInsets.symmetric(horizontal: 16, vertical: 4),
      child: Card(
        clipBehavior: Clip.antiAlias,
        child: InkWell(
          onTap: () => _edit(e),
          child: Padding(
            padding: const EdgeInsets.fromLTRB(14, 10, 2, 10),
            child: Row(
              children: [
                CircleAvatar(
                  radius: 22,
                  backgroundColor: cat.color.withValues(alpha: 0.15),
                  child: Icon(cat.icon, color: cat.color, size: 22),
                ),
                const SizedBox(width: 14),
                Expanded(
                  child: Column(
                    crossAxisAlignment: CrossAxisAlignment.start,
                    children: [
                      Row(
                        children: [
                          Flexible(
                            child: Text(
                              e.site,
                              maxLines: 1,
                              overflow: TextOverflow.ellipsis,
                              style: theme.textTheme.titleMedium
                                  ?.copyWith(fontWeight: FontWeight.w600),
                            ),
                          ),
                          if (e.hasTotp) ...[
                            const SizedBox(width: 6),
                            Icon(Icons.pin_rounded,
                                size: 16, color: theme.colorScheme.primary),
                          ],
                        ],
                      ),
                      if (e.username.isNotEmpty)
                        Text(
                          e.username,
                          maxLines: 1,
                          overflow: TextOverflow.ellipsis,
                          style: theme.textTheme.bodyMedium?.copyWith(
                            color: theme.colorScheme.onSurfaceVariant,
                          ),
                        ),
                    ],
                  ),
                ),
                IconButton(
                  icon: const Icon(Icons.copy_rounded),
                  tooltip: 'Copy password',
                  onPressed: () => _copyPassword(e),
                ),
                PopupMenuButton<String>(
                  tooltip: 'More',
                  onSelected: (v) {
                    if (v == 'user') _copyUsername(e);
                    if (v == 'edit') _edit(e);
                    if (v == 'delete') _delete(e);
                  },
                  itemBuilder: (_) => [
                    if (e.username.isNotEmpty)
                      const PopupMenuItem(
                        value: 'user',
                        child: Text('Copy username'),
                      ),
                    const PopupMenuItem(value: 'edit', child: Text('Edit')),
                    const PopupMenuItem(value: 'delete', child: Text('Delete')),
                  ],
                ),
              ],
            ),
          ),
        ),
      ),
    );
  }

  Widget _emptyState({required bool noEntriesAtAll}) {
    final theme = Theme.of(context);
    return Center(
      child: Padding(
        padding: const EdgeInsets.all(32),
        child: Column(
          mainAxisSize: MainAxisSize.min,
          children: [
            Icon(
              noEntriesAtAll ? Icons.key_rounded : Icons.search_off_rounded,
              size: 56,
              color: theme.colorScheme.outline,
            ),
            const SizedBox(height: 16),
            Text(
              noEntriesAtAll ? 'No passwords yet' : 'No matches',
              style: theme.textTheme.titleLarge,
            ),
            const SizedBox(height: 8),
            Text(
              noEntriesAtAll
                  ? 'Tap Add to save your first one, or import them from '
                      'Settings.'
                  : 'Try a different search or category.',
              textAlign: TextAlign.center,
              style: theme.textTheme.bodyMedium
                  ?.copyWith(color: theme.colorScheme.onSurfaceVariant),
            ),
          ],
        ),
      ),
    );
  }

  @override
  Widget build(BuildContext context) {
    final shown = _filtered;
    return Scaffold(
      appBar: AppBar(
        title: const Text('My Vault'),
        actions: [
          IconButton(
            icon: const Icon(Icons.pin_rounded),
            tooltip: 'Two-factor codes',
            onPressed: _openCodes,
          ),
          IconButton(
            icon: const Icon(Icons.health_and_safety_rounded),
            tooltip: 'Password health',
            onPressed: _openHealth,
          ),
          IconButton(
            icon: const Icon(Icons.settings_rounded),
            tooltip: 'Settings',
            onPressed: _openSettings,
          ),
          IconButton(
            icon: const Icon(Icons.lock_rounded),
            tooltip: 'Lock',
            onPressed: widget.onLock,
          ),
          const SizedBox(width: 4),
        ],
      ),
      floatingActionButton: FloatingActionButton.extended(
        onPressed: _add,
        icon: const Icon(Icons.add_rounded),
        label: const Text('Add'),
      ),
      body: _loading
          ? const Center(child: CircularProgressIndicator())
          : Column(
              children: [
                Padding(
                  padding: const EdgeInsets.fromLTRB(16, 4, 16, 12),
                  child: TextField(
                    decoration: const InputDecoration(
                      prefixIcon: Icon(Icons.search_rounded),
                      hintText: 'Search passwords',
                    ),
                    onChanged: (v) => setState(() => _query = v),
                  ),
                ),
                if (_entries.isNotEmpty) _filterChips(),
                if (_entries.isNotEmpty) const SizedBox(height: 8),
                Expanded(
                  child: shown.isEmpty
                      ? _emptyState(noEntriesAtAll: _entries.isEmpty)
                      : ListView.builder(
                          padding: const EdgeInsets.only(bottom: 96),
                          itemCount: shown.length,
                          itemBuilder: (_, i) => _entryCard(shown[i]),
                        ),
                ),
              ],
            ),
    );
  }
}

// ---------------------------------------------------------------------------
// Settings screen.
// ---------------------------------------------------------------------------

/// Result of dialogs that may hand back a NEW recovery key
/// (changing the master password or recovering replaces the old key).
class NewKeyResult {
  final String? newKey;

  const NewKeyResult(this.newKey);
}

class SettingsScreen extends StatefulWidget {
  final String vaultPath;

  const SettingsScreen({super.key, required this.vaultPath});

  @override
  State<SettingsScreen> createState() => _SettingsScreenState();
}

class _SettingsScreenState extends State<SettingsScreen> {
  bool? _hasRecovery;

  @override
  void initState() {
    super.initState();
    _loadRecoveryStatus();
  }

  void _toast(String message) {
    if (!mounted) return;
    ScaffoldMessenger.of(context)
      ..hideCurrentSnackBar()
      ..showSnackBar(SnackBar(content: Text(message)));
  }

  Future<void> _loadRecoveryStatus() async {
    try {
      final has = await hasRecoveryKey();
      if (!mounted) return;
      setState(() => _hasRecovery = has);
    } catch (e) {
      _toast(e.toString());
    }
  }

  Future<void> _changePassword() async {
    final result = await Navigator.of(context).push<NewKeyResult>(
      MaterialPageRoute(builder: (_) => const ChangePasswordScreen()),
    );
    if (result == null || !mounted) return;
    _toast('Master password changed.');
    final newKey = result.newKey;
    if (newKey != null) {
      await showDialog<void>(
        context: context,
        barrierDismissible: false,
        builder: (_) => RecoveryKeyDialog(recoveryKey: newKey, replaced: true),
      );
    }
  }

  Future<void> _recoveryKey() async {
    if (_hasRecovery == true) {
      final ok = await showDialog<bool>(
        context: context,
        builder: (ctx) => AlertDialog(
          title: const Text('Replace recovery key?'),
          content: const Text(
            'A new key will be created. Your old recovery key will stop '
            'working, so destroy any copies of it.',
          ),
          actions: [
            TextButton(
              onPressed: () => Navigator.pop(ctx, false),
              child: const Text('Cancel'),
            ),
            FilledButton(
              onPressed: () => Navigator.pop(ctx, true),
              child: const Text('Create new key'),
            ),
          ],
        ),
      );
      if (ok != true || !mounted) return;
    }
    final pw = await showDialog<String>(
      context: context,
      builder: (_) => const PasswordPromptDialog(
        title: 'Confirm your master password',
        label: 'Master password',
      ),
    );
    if (pw == null || !mounted) return;
    _toast('Working, please wait...');
    try {
      final key = await createRecoveryKey(password: pw);
      if (!mounted) return;
      ScaffoldMessenger.of(context).hideCurrentSnackBar();
      await showDialog<void>(
        context: context,
        barrierDismissible: false,
        builder: (_) => RecoveryKeyDialog(recoveryKey: key),
      );
      await _loadRecoveryStatus();
    } catch (e) {
      _toast(e.toString());
    }
  }

  Future<void> _export() async {
    pickerOpen = true;
    try {
      final bytes = await exportBackupBytes();
      final stamp = DateTime.now().toIso8601String().substring(0, 10);
      final name = 'vault-backup-$stamp.vault';
      if (Platform.isAndroid || Platform.isIOS) {
        final saved = await fp.FilePicker.saveFile(
          dialogTitle: 'Save encrypted backup',
          fileName: name,
          bytes: bytes,
        );
        if (saved == null) return; // cancelled
      } else {
        final location = await getSaveLocation(
          suggestedName: name,
          acceptedTypeGroups: [vaultTypeGroup],
        );
        if (location == null) return; // cancelled
        await File(location.path).writeAsBytes(bytes);
      }
      _toast('Backup saved. It is encrypted with your master password.');
    } catch (e) {
      _toast(e.toString());
    } finally {
      pickerOpen = false;
    }
  }

  /// Import logins from a CSV export (Chrome, Edge, Firefox, Bitwarden...).
  Future<void> _importCsv() async {
    final ok = await showDialog<bool>(
      context: context,
      builder: (ctx) => AlertDialog(
        title: const Text('Import from a CSV file'),
        scrollable: true,
        content: const Text(
          'Works with exports from Chrome, Edge, Firefox, Bitwarden and '
          'similar apps.\n\n'
          'Important: a CSV export holds your passwords as plain, readable '
          'text. Only use a file you just exported, and delete it as soon as '
          'the import is done.\n\n'
          'Entries that are already in your vault are not duplicated.',
        ),
        actions: [
          TextButton(
            onPressed: () => Navigator.pop(ctx, false),
            child: const Text('Cancel'),
          ),
          FilledButton(
            onPressed: () => Navigator.pop(ctx, true),
            child: const Text('Choose CSV file'),
          ),
        ],
      ),
    );
    if (ok != true || !mounted) return;
    try {
      XFile? file;
      pickerOpen = true;
      try {
        file = await openFile(
          acceptedTypeGroups:
              Platform.isAndroid ? const <XTypeGroup>[] : [csvTypeGroup],
        );
      } finally {
        pickerOpen = false;
      }
      if (file == null || !mounted) return;
      _ensureReadable(file);
      final bytes = await file.readAsBytes();
      final summary = await importCsv(bytes: bytes);
      if (!mounted) return;
      final canWipe =
          Platform.isWindows || Platform.isLinux || Platform.isMacOS;
      final wipe = await showDialog<bool>(
        context: context,
        builder: (ctx) => AlertDialog(
          title: const Text('Import finished'),
          scrollable: true,
          content: Text(
            '${summary.added} added\n'
            '${summary.duplicates} skipped (already in your vault)\n'
            '${summary.skipped} skipped (no password, no site, or not a login)\n\n'
            'Your CSV file still contains every password in plain text. '
            'Delete it now, and empty the Recycle Bin or Trash too.',
          ),
          actions: [
            TextButton(
              onPressed: () => Navigator.pop(ctx, false),
              child: const Text('Keep file'),
            ),
            FilledButton(
              onPressed: () => Navigator.pop(ctx, canWipe),
              child: Text(canWipe ? 'Delete the file' : 'OK'),
            ),
          ],
        ),
      );
      if (wipe == true) {
        // Best effort: overwrite with zeros, then delete. On SSDs and phones
        // this cannot be guaranteed, so deleting it yourself is still wise.
        final f = File(file.path);
        await f.writeAsBytes(List<int>.filled(bytes.length, 0), flush: true);
        await f.delete();
        _toast('CSV file overwritten and deleted.');
      }
    } catch (e) {
      _toast(e.toString());
    }
  }

  Future<void> _import() async {
    final ok = await showDialog<bool>(
      context: context,
      builder: (ctx) => AlertDialog(
        title: const Text('Restore from backup?'),
        content: const Text(
          'This replaces everything in your current vault with the backup. '
          'Your current vault file is kept next to it with a ".bak" ending, '
          'just in case. You will need the master password the backup was '
          'protected with.',
        ),
        actions: [
          TextButton(
            onPressed: () => Navigator.pop(ctx, false),
            child: const Text('Cancel'),
          ),
          FilledButton(
            onPressed: () => Navigator.pop(ctx, true),
            child: const Text('Choose backup file'),
          ),
        ],
      ),
    );
    if (ok != true || !mounted) return;
    try {
      XFile? file;
      pickerOpen = true;
      try {
        file = await openFile(
          acceptedTypeGroups:
              Platform.isAndroid ? const <XTypeGroup>[] : [vaultTypeGroup],
        );
      } finally {
        pickerOpen = false;
      }
      if (file == null || !mounted) return;
      _ensureReadable(file);
      final bytes = await file.readAsBytes();
      if (!mounted) return;
      final pw = await showDialog<String>(
        context: context,
        builder: (_) => const PasswordPromptDialog(
          title: 'Password for this backup',
          label: 'Backup master password',
        ),
      );
      if (pw == null || !mounted) return;
      _toast('Restoring, please wait...');
      await importBackup(path: widget.vaultPath, bytes: bytes, password: pw);
      _toast('Backup restored. Your previous vault was kept as a .bak file.');
      await _loadRecoveryStatus();
    } catch (e) {
      _toast(e.toString());
    }
  }

  Future<void> _openSync() async {
    await Navigator.of(context).push(
      MaterialPageRoute<void>(builder: (_) => const SyncScreen()),
    );
  }

  Future<void> _openGitHub() async {
    try {
      final ok = await launchUrl(
        Uri.parse(kGitHubUrl),
        mode: LaunchMode.externalApplication,
      );
      if (!ok) throw StateError('could not open the link');
    } catch (_) {
      await Clipboard.setData(const ClipboardData(text: kGitHubUrl));
      _toast('Could not open the browser. The link was copied instead.');
    }
  }

  Future<void> _copyAppInfo() async {
    final info =
        'Vault $kAppVersion - ${Platform.operatingSystem} - Rust core, Flutter UI';
    await Clipboard.setData(ClipboardData(text: info));
    _toast('App info copied.');
  }

  void _showLicenses() {
    showLicensePage(
      context: context,
      applicationName: 'Vault',
      applicationVersion: kAppVersion,
      applicationLegalese: '\u00a9 2026 $kDeveloper. Released under the MIT '
          'License. Built with the open-source libraries listed here.',
    );
  }

  void _showProtection() {
    final theme = Theme.of(context);
    showModalBottomSheet<void>(
      context: context,
      isScrollControlled: true,
      showDragHandle: true,
      builder: (ctx) => SafeArea(
        child: SingleChildScrollView(
          padding: const EdgeInsets.fromLTRB(24, 0, 24, 24),
          child: Column(
            mainAxisSize: MainAxisSize.min,
            crossAxisAlignment: CrossAxisAlignment.start,
            children: [
              Text(
                'How your data is protected',
                style: theme.textTheme.titleLarge
                    ?.copyWith(fontWeight: FontWeight.w700),
              ),
              const SizedBox(height: 16),
              const _ProtectionPoint(
                icon: Icons.enhanced_encryption_rounded,
                title: 'Encrypted on your device',
                text: 'Your vault is encrypted with XChaCha20-Poly1305 before '
                    'it is saved. Anyone who copies the file sees only noise.',
              ),
              const _ProtectionPoint(
                icon: Icons.speed_rounded,
                title: 'Hard to guess',
                text: 'Your master password is turned into a key with '
                    'Argon2id, which is deliberately slow and memory-hungry, '
                    'so guessing millions of passwords is expensive.',
              ),
              const _ProtectionPoint(
                icon: Icons.wifi_off_rounded,
                title: 'Works fully offline',
                text: 'There are no accounts and no servers. The app contains '
                    'no network code, so nothing is sent anywhere.',
              ),
              const _ProtectionPoint(
                icon: Icons.timer_rounded,
                title: 'Locks itself',
                text: 'The vault locks after inactivity and when you leave '
                    'the app, and copied passwords are cleared from the '
                    'clipboard.',
              ),
              const _ProtectionPoint(
                icon: Icons.key_off_rounded,
                title: 'Nobody can reset it for you',
                text: 'Not even the developer. If you forget your master '
                    'password, only your recovery key or a backup can help, '
                    'so keep them safe.',
              ),
            ],
          ),
        ),
      ),
    );
  }

  @override
  Widget build(BuildContext context) {
    final theme = Theme.of(context);
    final scheme = theme.colorScheme;
    final recoverySubtitle = switch (_hasRecovery) {
      null => 'Checking...',
      true => 'Active. Tap to replace it with a new one.',
      false => 'Not set up. Tap to create one.',
    };
    return Scaffold(
      appBar: AppBar(title: const Text('Settings')),
      body: ListenableBuilder(
        listenable: settings,
        builder: (context, _) => ListView(
          padding: const EdgeInsets.fromLTRB(16, 0, 16, 32),
          children: [
            _SettingsSection(
              title: 'Security',
              children: [
                _SettingsTile(
                  icon: Icons.timer_rounded,
                  color: const Color(0xFF6366F1),
                  title: 'Auto-lock',
                  subtitle: 'Lock after this much inactivity',
                  trailing: DropdownButton<int>(
                    value: settings.autoLockMinutes,
                    underline: const SizedBox.shrink(),
                    borderRadius: BorderRadius.circular(14),
                    items: [
                      for (final m in autoLockChoices)
                        DropdownMenuItem(
                          value: m,
                          child: Text(m == 1 ? '1 minute' : '$m minutes'),
                        ),
                    ],
                    onChanged: (v) {
                      if (v != null) settings.update(autoLockMinutes: v);
                    },
                  ),
                ),
                _SettingsTile(
                  icon: Icons.content_paste_off_rounded,
                  color: const Color(0xFF0EA5E9),
                  title: 'Clear copied passwords',
                  subtitle: 'Wipe the clipboard after this long',
                  trailing: DropdownButton<int>(
                    value: settings.clipboardSeconds,
                    underline: const SizedBox.shrink(),
                    borderRadius: BorderRadius.circular(14),
                    items: [
                      for (final s in clipboardChoices)
                        DropdownMenuItem(value: s, child: Text('$s seconds')),
                    ],
                    onChanged: (v) {
                      if (v != null) settings.update(clipboardSeconds: v);
                    },
                  ),
                ),
                _SettingsTile(
                  icon: Icons.password_rounded,
                  color: const Color(0xFFF59E0B),
                  title: 'Change master password',
                  subtitle: 'Choose a new password for your vault',
                  onTap: _changePassword,
                ),
              ],
            ),
            _SettingsSection(
              title: 'Backup and recovery',
              children: [
                _SettingsTile(
                  icon: Icons.vpn_key_rounded,
                  color: const Color(0xFF10B981),
                  title: 'Recovery key',
                  subtitle: recoverySubtitle,
                  onTap: _hasRecovery == null ? null : _recoveryKey,
                ),
                _SettingsTile(
                  icon: Icons.sync_rounded,
                  color: const Color(0xFF0EA5E9),
                  title: 'Sync with another device',
                  subtitle: 'Merge changes between your phone and PC',
                  onTap: _openSync,
                ),
                _SettingsTile(
                  icon: Icons.file_upload_rounded,
                  color: const Color(0xFF3B82F6),
                  title: 'Export encrypted backup',
                  subtitle: 'A copy of your vault, still protected by your '
                      'master password',
                  onTap: _export,
                ),
                _SettingsTile(
                  icon: Icons.settings_backup_restore_rounded,
                  color: const Color(0xFF8B5CF6),
                  title: 'Restore from backup',
                  subtitle: 'Replace this vault with a backup file',
                  onTap: _import,
                ),
                _SettingsTile(
                  icon: Icons.table_view_rounded,
                  color: const Color(0xFFEC4899),
                  title: 'Import from CSV',
                  subtitle: 'From Chrome, Edge, Firefox, Bitwarden and similar',
                  onTap: _importCsv,
                ),
              ],
            ),
            _SettingsSection(
              title: 'Password generator',
              children: [
                Padding(
                  padding: const EdgeInsets.fromLTRB(16, 12, 16, 4),
                  child: Column(
                    crossAxisAlignment: CrossAxisAlignment.start,
                    children: [
                      Row(
                        children: [
                          const _SettingsAvatar(
                            icon: Icons.straighten_rounded,
                            color: Color(0xFF14B8A6),
                          ),
                          const SizedBox(width: 16),
                          Expanded(
                            child: Text(
                              'Password length',
                              style: theme.textTheme.titleMedium
                                  ?.copyWith(fontWeight: FontWeight.w600),
                            ),
                          ),
                          Container(
                            padding: const EdgeInsets.symmetric(
                                horizontal: 14, vertical: 6),
                            decoration: BoxDecoration(
                              color: scheme.primaryContainer,
                              borderRadius: BorderRadius.circular(20),
                            ),
                            child: Text(
                              '${settings.genLength}',
                              style: TextStyle(
                                fontWeight: FontWeight.w700,
                                color: scheme.onPrimaryContainer,
                              ),
                            ),
                          ),
                        ],
                      ),
                      Slider(
                        value: settings.genLength.toDouble(),
                        min: 8,
                        max: 64,
                        divisions: 56,
                        label: '${settings.genLength}',
                        onChanged: (v) => settings.update(genLength: v.round()),
                      ),
                    ],
                  ),
                ),
                SwitchListTile(
                  secondary: const _SettingsAvatar(
                    icon: Icons.tag_rounded,
                    color: Color(0xFFF97316),
                  ),
                  contentPadding: const EdgeInsets.symmetric(horizontal: 16),
                  title: const Text('Include symbols'),
                  subtitle: const Text('For example ! @ # \$ %'),
                  value: settings.genSymbols,
                  onChanged: (v) => settings.update(genSymbols: v),
                ),
                SwitchListTile(
                  secondary: const _SettingsAvatar(
                    icon: Icons.visibility_rounded,
                    color: Color(0xFF64748B),
                  ),
                  contentPadding: const EdgeInsets.symmetric(horizontal: 16),
                  title: const Text('Avoid look-alike characters'),
                  subtitle: const Text('Skips I, l, 1, O and 0'),
                  value: settings.genExcludeAmbiguous,
                  onChanged: (v) => settings.update(genExcludeAmbiguous: v),
                ),
              ],
            ),
            _SettingsSection(
              title: 'Appearance',
              children: [
                Padding(
                  padding: const EdgeInsets.fromLTRB(16, 14, 16, 16),
                  child: Column(
                    crossAxisAlignment: CrossAxisAlignment.start,
                    children: [
                      Row(
                        children: [
                          const _SettingsAvatar(
                            icon: Icons.palette_rounded,
                            color: Color(0xFFEC4899),
                          ),
                          const SizedBox(width: 16),
                          Expanded(
                            child: Text(
                              'Theme',
                              style: theme.textTheme.titleMedium
                                  ?.copyWith(fontWeight: FontWeight.w600),
                            ),
                          ),
                        ],
                      ),
                      const SizedBox(height: 12),
                      SizedBox(
                        width: double.infinity,
                        child: SegmentedButton<ThemeMode>(
                          showSelectedIcon: false,
                          segments: const [
                            ButtonSegment(
                              value: ThemeMode.system,
                              icon: Icon(Icons.brightness_auto_rounded),
                              label: Text('Auto'),
                            ),
                            ButtonSegment(
                              value: ThemeMode.light,
                              icon: Icon(Icons.light_mode_rounded),
                              label: Text('Light'),
                            ),
                            ButtonSegment(
                              value: ThemeMode.dark,
                              icon: Icon(Icons.dark_mode_rounded),
                              label: Text('Dark'),
                            ),
                          ],
                          selected: {settings.themeMode},
                          onSelectionChanged: (s) => changeTheme(s.first),
                        ),
                      ),
                    ],
                  ),
                ),
              ],
            ),
            _SettingsSection(
              title: 'About',
              children: [
                Padding(
                  padding: const EdgeInsets.all(20),
                  child: Row(
                    children: [
                      Container(
                        width: 60,
                        height: 60,
                        decoration: BoxDecoration(
                          gradient: const LinearGradient(
                            colors: [Color(0xFF4F46E5), Color(0xFF7C3AED)],
                            begin: Alignment.topLeft,
                            end: Alignment.bottomRight,
                          ),
                          borderRadius: BorderRadius.circular(18),
                        ),
                        child: const Icon(
                          Icons.shield_rounded,
                          color: Colors.white,
                          size: 32,
                        ),
                      ),
                      const SizedBox(width: 16),
                      Expanded(
                        child: Column(
                          crossAxisAlignment: CrossAxisAlignment.start,
                          children: [
                            Text(
                              'Vault',
                              style: theme.textTheme.titleLarge
                                  ?.copyWith(fontWeight: FontWeight.w700),
                            ),
                            Text(
                              'Version $kAppVersion',
                              style: theme.textTheme.bodyMedium?.copyWith(
                                color: scheme.onSurfaceVariant,
                              ),
                            ),
                            Text(
                              'Offline password manager',
                              style: theme.textTheme.bodyMedium?.copyWith(
                                color: scheme.onSurfaceVariant,
                              ),
                            ),
                          ],
                        ),
                      ),
                    ],
                  ),
                ),
                const _SettingsTile(
                  icon: Icons.code_rounded,
                  color: Color(0xFF8B5CF6),
                  title: 'Developed by',
                  subtitle: kDeveloper,
                ),
                _SettingsTile(
                  icon: Icons.public_rounded,
                  color: const Color(0xFF334155),
                  title: '$kDeveloper on GitHub',
                  subtitle: 'github.com/VawnDyu',
                  trailing: const Icon(Icons.open_in_new_rounded),
                  onTap: _openGitHub,
                ),
                _SettingsTile(
                  icon: Icons.new_releases_rounded,
                  color: const Color(0xFFEF4444),
                  title: "What's new",
                  subtitle: 'Changes in version $kAppVersion',
                  onTap: () => showWhatsNew(context),
                ),
                _SettingsTile(
                  icon: Icons.waving_hand_rounded,
                  color: const Color(0xFFF59E0B),
                  title: 'Welcome tour',
                  subtitle: 'See the introduction again',
                  onTap: () => showWelcome(context),
                ),
                _SettingsTile(
                  icon: Icons.verified_user_rounded,
                  color: const Color(0xFF10B981),
                  title: 'How your data is protected',
                  subtitle: 'Encryption, offline design and locking',
                  onTap: _showProtection,
                ),
                _SettingsTile(
                  icon: Icons.description_rounded,
                  color: const Color(0xFF3B82F6),
                  title: 'Open-source licenses',
                  subtitle: 'Credits for the libraries this app is built with',
                  onTap: _showLicenses,
                ),
                _SettingsTile(
                  icon: Icons.copy_all_rounded,
                  color: const Color(0xFFF59E0B),
                  title: 'Copy app info',
                  subtitle: 'Handy when reporting a problem',
                  onTap: _copyAppInfo,
                ),
              ],
            ),
            const SizedBox(height: 6),
            Center(
              child: Text(
                '\u00a9 2026 $kDeveloper',
                style: theme.textTheme.bodySmall
                    ?.copyWith(color: scheme.outline),
              ),
            ),
          ],
        ),
      ),
    );
  }
}

/// A titled card that groups related settings.
class _SettingsSection extends StatelessWidget {
  final String title;
  final List<Widget> children;

  const _SettingsSection({required this.title, required this.children});

  @override
  Widget build(BuildContext context) {
    final theme = Theme.of(context);
    return Column(
      crossAxisAlignment: CrossAxisAlignment.start,
      children: [
        Padding(
          padding: const EdgeInsets.fromLTRB(8, 20, 8, 8),
          child: Text(
            title,
            style: theme.textTheme.titleSmall?.copyWith(
              color: theme.colorScheme.primary,
              fontWeight: FontWeight.w700,
            ),
          ),
        ),
        Card(
          clipBehavior: Clip.antiAlias,
          child: Column(
            children: [
              for (var i = 0; i < children.length; i++) ...[
                if (i > 0) const Divider(height: 1, indent: 72),
                children[i],
              ],
            ],
          ),
        ),
      ],
    );
  }
}

/// A round, softly tinted icon used at the start of a settings row.
class _SettingsAvatar extends StatelessWidget {
  final IconData icon;
  final Color color;

  const _SettingsAvatar({required this.icon, required this.color});

  @override
  Widget build(BuildContext context) {
    return CircleAvatar(
      radius: 20,
      backgroundColor: color.withValues(alpha: 0.15),
      child: Icon(icon, color: color, size: 20),
    );
  }
}

class _SettingsTile extends StatelessWidget {
  final IconData icon;
  final Color color;
  final String title;
  final String? subtitle;
  final Widget? trailing;
  final VoidCallback? onTap;

  const _SettingsTile({
    required this.icon,
    required this.color,
    required this.title,
    this.subtitle,
    this.trailing,
    this.onTap,
  });

  @override
  Widget build(BuildContext context) {
    return ListTile(
      contentPadding: const EdgeInsets.symmetric(horizontal: 16, vertical: 4),
      leading: _SettingsAvatar(icon: icon, color: color),
      title: Text(title, style: const TextStyle(fontWeight: FontWeight.w600)),
      subtitle: subtitle == null ? null : Text(subtitle!),
      trailing: trailing ??
          (onTap != null ? const Icon(Icons.chevron_right_rounded) : null),
      onTap: onTap,
    );
  }
}

class _ProtectionPoint extends StatelessWidget {
  final IconData icon;
  final String title;
  final String text;

  const _ProtectionPoint({
    required this.icon,
    required this.title,
    required this.text,
  });

  @override
  Widget build(BuildContext context) {
    final theme = Theme.of(context);
    return Padding(
      padding: const EdgeInsets.only(bottom: 16),
      child: Row(
        crossAxisAlignment: CrossAxisAlignment.start,
        children: [
          Icon(icon, color: theme.colorScheme.primary),
          const SizedBox(width: 14),
          Expanded(
            child: Column(
              crossAxisAlignment: CrossAxisAlignment.start,
              children: [
                Text(
                  title,
                  style: theme.textTheme.titleMedium
                      ?.copyWith(fontWeight: FontWeight.w600),
                ),
                const SizedBox(height: 2),
                Text(
                  text,
                  style: theme.textTheme.bodyMedium
                      ?.copyWith(color: theme.colorScheme.onSurfaceVariant),
                ),
              ],
            ),
          ),
        ],
      ),
    );
  }
}

/// A password box with a show/hide eye, used on the password screens.
class _PasswordField extends StatefulWidget {
  final TextEditingController controller;
  final String label;
  final bool enabled;
  final bool autofocus;
  final TextInputAction action;
  final ValueChanged<String>? onSubmitted;

  const _PasswordField({
    required this.controller,
    required this.label,
    this.enabled = true,
    this.autofocus = false,
    this.action = TextInputAction.next,
    this.onSubmitted,
  });

  @override
  State<_PasswordField> createState() => _PasswordFieldState();
}

class _PasswordFieldState extends State<_PasswordField> {
  bool _show = false;

  @override
  Widget build(BuildContext context) {
    return TextField(
      controller: widget.controller,
      obscureText: !_show,
      enabled: widget.enabled,
      autofocus: widget.autofocus,
      autocorrect: false,
      enableSuggestions: false,
      enableIMEPersonalizedLearning: false,
      textInputAction: widget.action,
      onSubmitted: widget.onSubmitted,
      decoration: InputDecoration(
        labelText: widget.label,
        prefixIcon: const Icon(Icons.key_rounded),
        suffixIcon: IconButton(
          tooltip: _show ? 'Hide' : 'Show',
          icon: Icon(
            _show ? Icons.visibility_off_rounded : Icons.visibility_rounded,
          ),
          onPressed: () => setState(() => _show = !_show),
        ),
      ),
    );
  }
}

/// A red box for error messages.
class _ErrorBox extends StatelessWidget {
  final String message;

  const _ErrorBox(this.message);

  @override
  Widget build(BuildContext context) {
    final scheme = Theme.of(context).colorScheme;
    return Container(
      padding: const EdgeInsets.all(12),
      decoration: BoxDecoration(
        color: scheme.errorContainer,
        borderRadius: BorderRadius.circular(14),
      ),
      child: Row(
        crossAxisAlignment: CrossAxisAlignment.start,
        children: [
          Icon(Icons.error_outline_rounded,
              color: scheme.onErrorContainer, size: 20),
          const SizedBox(width: 10),
          Expanded(
            child: Text(message, style: TextStyle(color: scheme.onErrorContainer)),
          ),
        ],
      ),
    );
  }
}

/// A softly tinted box for tips and warnings.
class _InfoBox extends StatelessWidget {
  final IconData icon;
  final String text;

  const _InfoBox({required this.icon, required this.text});

  @override
  Widget build(BuildContext context) {
    final scheme = Theme.of(context).colorScheme;
    return Container(
      padding: const EdgeInsets.all(14),
      decoration: BoxDecoration(
        color: scheme.secondaryContainer.withValues(alpha: 0.6),
        borderRadius: BorderRadius.circular(14),
      ),
      child: Row(
        crossAxisAlignment: CrossAxisAlignment.start,
        children: [
          Icon(icon, color: scheme.onSecondaryContainer, size: 20),
          const SizedBox(width: 10),
          Expanded(
            child: Text(
              text,
              style: TextStyle(color: scheme.onSecondaryContainer, height: 1.4),
            ),
          ),
        ],
      ),
    );
  }
}

// ---------------------------------------------------------------------------
// Shows a recovery key once, and makes the user confirm they saved it.
// ---------------------------------------------------------------------------
class RecoveryKeyDialog extends StatefulWidget {
  final String recoveryKey;
  final bool replaced;

  const RecoveryKeyDialog({
    super.key,
    required this.recoveryKey,
    this.replaced = false,
  });

  @override
  State<RecoveryKeyDialog> createState() => _RecoveryKeyDialogState();
}

class _RecoveryKeyDialogState extends State<RecoveryKeyDialog> {
  bool _saved = false;
  bool _copied = false;

  @override
  Widget build(BuildContext context) {
    final theme = Theme.of(context);
    final scheme = theme.colorScheme;
    return AlertDialog(
      icon: Icon(Icons.vpn_key_rounded, size: 36, color: scheme.primary),
      title: const Text('Your recovery key'),
      scrollable: true,
      content: SizedBox(
        width: 440,
        child: Column(
          mainAxisSize: MainAxisSize.min,
          crossAxisAlignment: CrossAxisAlignment.start,
          children: [
            Text(
              widget.replaced
                  ? 'Your recovery key was replaced. The old one no longer works.'
                  : 'If you forget your master password, this key is the only '
                      'way to get your data back.',
            ),
            const SizedBox(height: 14),
            Container(
              width: double.infinity,
              padding: const EdgeInsets.all(16),
              decoration: BoxDecoration(
                color: scheme.surfaceContainerHighest,
                borderRadius: BorderRadius.circular(16),
              ),
              child: SelectableText(
                widget.recoveryKey,
                textAlign: TextAlign.center,
                style: const TextStyle(
                  fontFamily: 'monospace',
                  fontSize: 17,
                  fontWeight: FontWeight.w600,
                  letterSpacing: 1.2,
                ),
              ),
            ),
            const SizedBox(height: 4),
            Center(
              child: TextButton.icon(
                icon: Icon(_copied ? Icons.check_rounded : Icons.copy_rounded),
                label: Text(
                  _copied ? 'Copied (clipboard clears soon)' : 'Copy to clipboard',
                ),
                onPressed: () async {
                  await copyWithAutoClear(widget.recoveryKey);
                  if (mounted) setState(() => _copied = true);
                },
              ),
            ),
            const SizedBox(height: 4),
            const _InfoBox(
              icon: Icons.warning_amber_rounded,
              text: 'Write it down and keep it somewhere safe and offline. '
                  'Never store it inside this vault or share it. It is shown '
                  'only once.',
            ),
            const SizedBox(height: 8),
            CheckboxListTile(
              value: _saved,
              onChanged: (v) => setState(() => _saved = v ?? false),
              title: const Text('I have saved this key'),
              controlAffinity: ListTileControlAffinity.leading,
              contentPadding: EdgeInsets.zero,
            ),
          ],
        ),
      ),
      actions: [
        FilledButton(
          onPressed: _saved ? () => Navigator.pop(context) : null,
          child: const Text('Done'),
        ),
      ],
    );
  }
}

// ---------------------------------------------------------------------------
// Asks for one password and returns it (null if cancelled).
// ---------------------------------------------------------------------------
class PasswordPromptDialog extends StatefulWidget {
  final String title;
  final String label;

  const PasswordPromptDialog({
    super.key,
    required this.title,
    required this.label,
  });

  @override
  State<PasswordPromptDialog> createState() => _PasswordPromptDialogState();
}

class _PasswordPromptDialogState extends State<PasswordPromptDialog> {
  final _controller = TextEditingController();
  bool _show = false;

  @override
  void dispose() {
    _controller.dispose();
    super.dispose();
  }

  @override
  Widget build(BuildContext context) {
    return AlertDialog(
      icon: Icon(Icons.lock_rounded,
          size: 32, color: Theme.of(context).colorScheme.primary),
      title: Text(widget.title),
      content: SizedBox(
        width: 400,
        child: TextField(
          controller: _controller,
          obscureText: !_show,
          autofocus: true,
          autocorrect: false,
          enableSuggestions: false,
          enableIMEPersonalizedLearning: false,
          decoration: InputDecoration(
            labelText: widget.label,
            prefixIcon: const Icon(Icons.key_rounded),
            suffixIcon: IconButton(
              tooltip: _show ? 'Hide' : 'Show',
              icon: Icon(
                _show ? Icons.visibility_off_rounded : Icons.visibility_rounded,
              ),
              onPressed: () => setState(() => _show = !_show),
            ),
          ),
          onSubmitted: (v) => Navigator.pop(context, v),
        ),
      ),
      actions: [
        TextButton(
          onPressed: () => Navigator.pop(context),
          child: const Text('Cancel'),
        ),
        FilledButton(
          onPressed: () => Navigator.pop(context, _controller.text),
          child: const Text('Continue'),
        ),
      ],
    );
  }
}

// ---------------------------------------------------------------------------
// "Forgot password": a full page for the recovery key + a new master password.
// ---------------------------------------------------------------------------
class RecoverScreen extends StatefulWidget {
  final String path;

  const RecoverScreen({super.key, required this.path});

  @override
  State<RecoverScreen> createState() => _RecoverScreenState();
}

class _RecoverScreenState extends State<RecoverScreen> {
  final _key = TextEditingController();
  final _new = TextEditingController();
  final _again = TextEditingController();
  bool _busy = false;
  String? _error;

  @override
  void dispose() {
    _key.dispose();
    _new.dispose();
    _again.dispose();
    super.dispose();
  }

  Future<void> _save() async {
    if (_busy) return;
    if (_key.text.trim().isEmpty) {
      setState(() => _error = 'Enter your recovery key.');
      return;
    }
    if (_new.text.length < 8) {
      setState(() => _error = 'Use at least 8 characters for the new password.');
      return;
    }
    if (_new.text != _again.text) {
      setState(() => _error = 'The new passwords do not match.');
      return;
    }
    setState(() {
      _busy = true;
      _error = null;
    });
    try {
      final newKey = await recoverWithKey(
        path: widget.path,
        recoveryKey: _key.text,
        newPassword: _new.text,
      );
      if (mounted) Navigator.pop(context, NewKeyResult(newKey));
    } catch (e) {
      if (!mounted) return;
      setState(() {
        _error = e.toString();
        _busy = false;
      });
    }
  }

  @override
  Widget build(BuildContext context) {
    return Scaffold(
      appBar: AppBar(title: const Text('Recover your vault')),
      body: SafeArea(
        child: ListView(
          keyboardDismissBehavior: ScrollViewKeyboardDismissBehavior.onDrag,
          padding: const EdgeInsets.fromLTRB(20, 4, 20, 32),
          children: [
            const _InfoBox(
              icon: Icons.info_outline_rounded,
              text: 'Enter the recovery key you saved when you set it up, '
                  'then choose a new master password. Your entries stay as '
                  'they are.',
            ),
            const SizedBox(height: 20),
            TextField(
              controller: _key,
              autofocus: true,
              enabled: !_busy,
              autocorrect: false,
              enableSuggestions: false,
              enableIMEPersonalizedLearning: false,
              textCapitalization: TextCapitalization.characters,
              textInputAction: TextInputAction.next,
              style: const TextStyle(fontFamily: 'monospace', letterSpacing: 1),
              decoration: const InputDecoration(
                labelText: 'Recovery key',
                prefixIcon: Icon(Icons.vpn_key_rounded),
              ),
            ),
            const SizedBox(height: 14),
            _PasswordField(
              controller: _new,
              label: 'New master password',
              enabled: !_busy,
            ),
            const SizedBox(height: 14),
            _PasswordField(
              controller: _again,
              label: 'Repeat new password',
              enabled: !_busy,
              action: TextInputAction.done,
              onSubmitted: (_) => _save(),
            ),
            if (_busy) ...[
              const SizedBox(height: 20),
              const LinearProgressIndicator(),
            ],
            if (_error != null) ...[
              const SizedBox(height: 16),
              _ErrorBox(_error!),
            ],
            const SizedBox(height: 24),
            FilledButton(
              onPressed: _busy ? null : _save,
              child: const Text('Recover vault'),
            ),
          ],
        ),
      ),
    );
  }
}

// ---------------------------------------------------------------------------
// Change master password: a full page, in the same style as the entry form.
// ---------------------------------------------------------------------------
class ChangePasswordScreen extends StatefulWidget {
  const ChangePasswordScreen({super.key});

  @override
  State<ChangePasswordScreen> createState() => _ChangePasswordScreenState();
}

class _ChangePasswordScreenState extends State<ChangePasswordScreen> {
  final _old = TextEditingController();
  final _new = TextEditingController();
  final _again = TextEditingController();
  bool _busy = false;
  String? _error;

  @override
  void dispose() {
    _old.dispose();
    _new.dispose();
    _again.dispose();
    super.dispose();
  }

  Future<void> _save() async {
    if (_busy) return;
    if (_old.text.isEmpty) {
      setState(() => _error = 'Enter your current master password.');
      return;
    }
    if (_new.text.length < 8) {
      setState(() => _error = 'Use at least 8 characters (longer is better).');
      return;
    }
    if (_new.text != _again.text) {
      setState(() => _error = 'The new passwords do not match.');
      return;
    }
    if (_new.text == _old.text) {
      setState(() => _error = 'The new password must be different.');
      return;
    }
    setState(() {
      _busy = true;
      _error = null;
    });
    try {
      final newKey = await changeMasterPassword(
        oldPassword: _old.text,
        newPassword: _new.text,
      );
      if (mounted) Navigator.pop(context, NewKeyResult(newKey));
    } catch (e) {
      if (!mounted) return;
      setState(() {
        _error = e.toString();
        _busy = false;
      });
    }
  }

  @override
  Widget build(BuildContext context) {
    return Scaffold(
      appBar: AppBar(title: const Text('Change master password')),
      body: SafeArea(
        child: ListView(
          keyboardDismissBehavior: ScrollViewKeyboardDismissBehavior.onDrag,
          padding: const EdgeInsets.fromLTRB(20, 4, 20, 32),
          children: [
            const _InfoBox(
              icon: Icons.info_outline_rounded,
              text: 'Backups you made earlier still open with your old '
                  'password. If you have a recovery key, you will get a new '
                  'one to save afterwards, and the old one stops working.',
            ),
            const SizedBox(height: 20),
            _PasswordField(
              controller: _old,
              label: 'Current password',
              enabled: !_busy,
              autofocus: true,
            ),
            const SizedBox(height: 14),
            _PasswordField(
              controller: _new,
              label: 'New password',
              enabled: !_busy,
            ),
            const SizedBox(height: 14),
            _PasswordField(
              controller: _again,
              label: 'Repeat new password',
              enabled: !_busy,
              action: TextInputAction.done,
              onSubmitted: (_) => _save(),
            ),
            const SizedBox(height: 8),
            Padding(
              padding: const EdgeInsets.symmetric(horizontal: 4),
              child: Text(
                'Tip: a long phrase of several random words is easier to '
                'remember and harder to guess than a short, complicated one.',
                style: Theme.of(context).textTheme.bodySmall?.copyWith(
                      color: Theme.of(context).colorScheme.onSurfaceVariant,
                    ),
              ),
            ),
            if (_busy) ...[
              const SizedBox(height: 20),
              const LinearProgressIndicator(),
            ],
            if (_error != null) ...[
              const SizedBox(height: 16),
              _ErrorBox(_error!),
            ],
            const SizedBox(height: 24),
            FilledButton(
              onPressed: _busy ? null : _save,
              child: const Text('Change master password'),
            ),
          ],
        ),
      ),
    );
  }
}

// ---------------------------------------------------------------------------
// Entry screen: a full page used for both adding and editing. A page (not a
// popup) gives small phones room to breathe and scroll above the keyboard.
// ---------------------------------------------------------------------------
class EntryScreen extends StatefulWidget {
  final EntryDetails? existing;
  final String? presetCategory;

  const EntryScreen({super.key, this.existing, this.presetCategory});

  @override
  State<EntryScreen> createState() => _EntryScreenState();
}

class _EntryScreenState extends State<EntryScreen> {
  late final TextEditingController _site;
  late final TextEditingController _user;
  late final TextEditingController _pw;
  late final TextEditingController _notes;
  late final TextEditingController _totp;
  late String _category;
  bool _hide = true;
  bool _hideTotp = true;
  bool _busy = false;
  String? _error;

  bool get _editing => widget.existing != null;

  @override
  void initState() {
    super.initState();
    final x = widget.existing;
    _site = TextEditingController(text: x?.site ?? '');
    _user = TextEditingController(text: x?.username ?? '');
    _pw = TextEditingController(text: x?.password ?? '');
    _notes = TextEditingController(text: x?.notes ?? '');
    _totp = TextEditingController(text: x?.totp ?? '');
    _category = x != null
        ? categoryFor(x.category).id
        : (widget.presetCategory ?? 'other');
  }

  @override
  void dispose() {
    _site.dispose();
    _user.dispose();
    _pw.dispose();
    _notes.dispose();
    _totp.dispose();
    super.dispose();
  }

  Future<void> _pasteTotp() async {
    final data = await Clipboard.getData(Clipboard.kTextPlain);
    final text = data?.text?.trim() ?? '';
    if (text.isEmpty || !mounted) return;
    setState(() {
      _totp.text = text;
      _hideTotp = false;
    });
  }

  Future<void> _generate() async {
    try {
      final p = await generatePassword(
        length: settings.genLength,
        symbols: settings.genSymbols,
        excludeAmbiguous: settings.genExcludeAmbiguous,
      );
      setState(() {
        _pw.text = p;
        _hide = false;
        _error = null;
      });
    } catch (e) {
      setState(() => _error = e.toString());
    }
  }

  Future<void> _save() async {
    if (_busy) return;
    if (_site.text.trim().isEmpty || _pw.text.isEmpty) {
      setState(() => _error = 'Please fill in the website or app, and the password.');
      return;
    }
    setState(() {
      _busy = true;
      _error = null;
    });
    try {
      final x = widget.existing;
      if (x == null) {
        await addEntry(
          site: _site.text.trim(),
          username: _user.text.trim(),
          password: _pw.text,
          notes: _notes.text,
          category: _category,
          totp: _totp.text.trim(),
        );
      } else {
        await updateEntry(
          id: x.id,
          site: _site.text.trim(),
          username: _user.text.trim(),
          password: _pw.text,
          notes: _notes.text,
          category: _category,
          totp: _totp.text.trim(),
        );
      }
      if (mounted) Navigator.pop(context, true);
    } catch (e) {
      if (!mounted) return;
      setState(() {
        _error = e.toString();
        _busy = false;
      });
    }
  }

  Future<void> _delete() async {
    final x = widget.existing;
    if (x == null) return;
    final ok = await showDialog<bool>(
      context: context,
      builder: (ctx) => AlertDialog(
        title: const Text('Delete this entry?'),
        content: Text('"${x.site}" will be deleted permanently.'),
        actions: [
          TextButton(
            onPressed: () => Navigator.pop(ctx, false),
            child: const Text('Cancel'),
          ),
          FilledButton(
            onPressed: () => Navigator.pop(ctx, true),
            child: const Text('Delete'),
          ),
        ],
      ),
    );
    if (ok != true) return;
    try {
      await deleteEntry(id: x.id);
      if (mounted) Navigator.pop(context, true);
    } catch (e) {
      if (mounted) setState(() => _error = e.toString());
    }
  }

  Widget _sectionLabel(String text) {
    final theme = Theme.of(context);
    return Padding(
      padding: const EdgeInsets.only(bottom: 10),
      child: Text(
        text,
        style: theme.textTheme.labelLarge
            ?.copyWith(color: theme.colorScheme.onSurfaceVariant),
      ),
    );
  }

  @override
  Widget build(BuildContext context) {
    final scheme = Theme.of(context).colorScheme;
    return Scaffold(
      appBar: AppBar(
        title: Text(_editing ? 'Edit password' : 'New password'),
        actions: [
          TextButton(
            onPressed: _busy ? null : _save,
            child: const Text('Save'),
          ),
          const SizedBox(width: 8),
        ],
      ),
      body: SafeArea(
        child: ListView(
          keyboardDismissBehavior: ScrollViewKeyboardDismissBehavior.onDrag,
          padding: const EdgeInsets.fromLTRB(20, 4, 20, 32),
          children: [
            _sectionLabel('Category'),
            Wrap(
              spacing: 8,
              runSpacing: 8,
              children: [
                for (final c in kCategories)
                  ChoiceChip(
                    avatar: Icon(c.icon, size: 18, color: c.color),
                    label: Text(c.label),
                    selected: _category == c.id,
                    onSelected: (_) => setState(() => _category = c.id),
                  ),
              ],
            ),
            const SizedBox(height: 24),
            TextField(
              controller: _site,
              autofocus: !_editing,
              textInputAction: TextInputAction.next,
              keyboardType: TextInputType.url,
              decoration: const InputDecoration(
                labelText: 'Website or app',
                prefixIcon: Icon(Icons.public_rounded),
              ),
            ),
            const SizedBox(height: 14),
            TextField(
              controller: _user,
              textInputAction: TextInputAction.next,
              keyboardType: TextInputType.emailAddress,
              autocorrect: false,
              enableSuggestions: false,
              enableIMEPersonalizedLearning: false,
              decoration: const InputDecoration(
                labelText: 'Username or email',
                prefixIcon: Icon(Icons.person_rounded),
              ),
            ),
            const SizedBox(height: 14),
            TextField(
              controller: _pw,
              obscureText: _hide,
              autocorrect: false,
              enableSuggestions: false,
              enableIMEPersonalizedLearning: false,
              textInputAction: TextInputAction.next,
              decoration: InputDecoration(
                labelText: 'Password',
                prefixIcon: const Icon(Icons.key_rounded),
                suffixIcon: IconButton(
                  tooltip: _hide ? 'Show' : 'Hide',
                  icon: Icon(_hide
                      ? Icons.visibility_rounded
                      : Icons.visibility_off_rounded),
                  onPressed: () => setState(() => _hide = !_hide),
                ),
              ),
            ),
            const SizedBox(height: 10),
            Align(
              alignment: Alignment.centerLeft,
              child: OutlinedButton.icon(
                onPressed: _generate,
                icon: const Icon(Icons.auto_awesome_rounded),
                label: const Text('Generate a strong password'),
              ),
            ),
            const SizedBox(height: 18),
            TextField(
              controller: _totp,
              obscureText: _hideTotp,
              autocorrect: false,
              enableSuggestions: false,
              enableIMEPersonalizedLearning: false,
              keyboardType: TextInputType.visiblePassword,
              decoration: InputDecoration(
                labelText: 'Two-factor setup key (optional)',
                prefixIcon: const Icon(Icons.pin_rounded),
                helperText: 'On the website\'s 2FA setup page, choose "Can\'t '
                    'scan the QR code?" and paste the key or link here.',
                helperMaxLines: 3,
                suffixIcon: Row(
                  mainAxisSize: MainAxisSize.min,
                  children: [
                    IconButton(
                      tooltip: 'Paste',
                      icon: const Icon(Icons.content_paste_rounded),
                      onPressed: _pasteTotp,
                    ),
                    IconButton(
                      tooltip: _hideTotp ? 'Show' : 'Hide',
                      icon: Icon(_hideTotp
                          ? Icons.visibility_rounded
                          : Icons.visibility_off_rounded),
                      onPressed: () => setState(() => _hideTotp = !_hideTotp),
                    ),
                  ],
                ),
              ),
            ),
            const SizedBox(height: 18),
            TextField(
              controller: _notes,
              minLines: 3,
              maxLines: 6,
              keyboardType: TextInputType.multiline,
              enableIMEPersonalizedLearning: false,
              decoration: const InputDecoration(
                labelText: 'Notes (optional)',
                alignLabelWithHint: true,
              ),
            ),
            if (_error != null) ...[
              const SizedBox(height: 16),
              Container(
                padding: const EdgeInsets.all(12),
                decoration: BoxDecoration(
                  color: scheme.errorContainer,
                  borderRadius: BorderRadius.circular(14),
                ),
                child: Text(
                  _error!,
                  style: TextStyle(color: scheme.onErrorContainer),
                ),
              ),
            ],
            const SizedBox(height: 24),
            FilledButton(
              onPressed: _busy ? null : _save,
              child: _busy
                  ? const SizedBox(
                      height: 22,
                      width: 22,
                      child: CircularProgressIndicator(strokeWidth: 2.5),
                    )
                  : const Text('Save'),
            ),
            if (_editing) ...[
              const SizedBox(height: 8),
              TextButton.icon(
                onPressed: _busy ? null : _delete,
                style: TextButton.styleFrom(foregroundColor: scheme.error),
                icon: const Icon(Icons.delete_outline_rounded),
                label: const Text('Delete this entry'),
              ),
            ],
          ],
        ),
      ),
    );
  }
}

// ---------------------------------------------------------------------------
// First-run welcome, and the "What's new" sheet shown once after an update.
// ---------------------------------------------------------------------------

/// Shown in the "What's new" sheet. Update this list (and kAppVersion) when
/// you release a new version.
const List<String> kWhatsNew = [
  'Sync between your phone and PC: merge changes from another device',
  'Each entry now remembers when it last changed',
  'Everything still works offline, and only the encrypted file moves',
];

/// Full-screen welcome tour. Marks it as seen when it closes.
Future<void> showWelcome(BuildContext context) async {
  await Navigator.of(context).push(
    MaterialPageRoute<void>(
      fullscreenDialog: true,
      builder: (_) => const WelcomeScreen(),
    ),
  );
  await settings.update(welcomeSeen: true, lastSeenVersion: kAppVersion);
}

/// Bottom sheet listing what changed. Marks this version as seen.
Future<void> showWhatsNew(BuildContext context) async {
  await showModalBottomSheet<void>(
    context: context,
    isScrollControlled: true,
    showDragHandle: true,
    builder: (_) => const _WhatsNewSheet(),
  );
  await settings.update(lastSeenVersion: kAppVersion);
}

class _WhatsNewSheet extends StatelessWidget {
  const _WhatsNewSheet();

  @override
  Widget build(BuildContext context) {
    final theme = Theme.of(context);
    final scheme = theme.colorScheme;
    return SafeArea(
      child: SingleChildScrollView(
        padding: const EdgeInsets.fromLTRB(24, 0, 24, 24),
        child: Column(
          mainAxisSize: MainAxisSize.min,
          crossAxisAlignment: CrossAxisAlignment.start,
          children: [
            Row(
              children: [
                Icon(Icons.new_releases_rounded, color: scheme.primary),
                const SizedBox(width: 10),
                Text(
                  "What's new",
                  style: theme.textTheme.titleLarge
                      ?.copyWith(fontWeight: FontWeight.w700),
                ),
              ],
            ),
            const SizedBox(height: 4),
            Text(
              'Version $kAppVersion',
              style: theme.textTheme.bodyMedium
                  ?.copyWith(color: scheme.onSurfaceVariant),
            ),
            const SizedBox(height: 16),
            for (final item in kWhatsNew)
              Padding(
                padding: const EdgeInsets.only(bottom: 12),
                child: Row(
                  crossAxisAlignment: CrossAxisAlignment.start,
                  children: [
                    Icon(Icons.check_circle_rounded,
                        size: 20, color: scheme.primary),
                    const SizedBox(width: 12),
                    Expanded(
                      child: Text(item, style: theme.textTheme.bodyLarge),
                    ),
                  ],
                ),
              ),
            const SizedBox(height: 8),
            FilledButton(
              onPressed: () => Navigator.pop(context),
              child: const Text('Got it'),
            ),
          ],
        ),
      ),
    );
  }
}

class WelcomeScreen extends StatefulWidget {
  const WelcomeScreen({super.key});

  @override
  State<WelcomeScreen> createState() => _WelcomeScreenState();
}

class _WelcomeScreenState extends State<WelcomeScreen> {
  final _controller = PageController();
  int _page = 0;

  static const List<Widget> _pages = [
    _WelcomePage(
      icon: Icons.shield_rounded,
      color: Color(0xFF4F46E5),
      title: 'Your passwords stay with you',
      text: 'Everything is encrypted on this device. There are no accounts '
          'and no servers, so your vault never leaves your phone or computer '
          'unless you export it yourself.',
    ),
    _WelcomePage(
      icon: Icons.vpn_key_rounded,
      color: Color(0xFF10B981),
      title: 'Keep a way back in',
      text: 'Nobody can reset your master password, not even the developer. '
          'In Settings, create a recovery key and an encrypted backup, and '
          'store them somewhere safe.',
    ),
    _WelcomePage(
      icon: Icons.auto_awesome_rounded,
      color: Color(0xFFEC4899),
      title: 'Make it yours',
      text: 'Sort passwords into categories, import them from your browser, '
          'and let the generator create strong ones for you.',
    ),
  ];

  @override
  void dispose() {
    _controller.dispose();
    super.dispose();
  }

  bool get _isLast => _page == _pages.length - 1;

  void _next() {
    if (_isLast) {
      Navigator.pop(context);
    } else {
      _controller.nextPage(
        duration: const Duration(milliseconds: 300),
        curve: Curves.easeOut,
      );
    }
  }

  @override
  Widget build(BuildContext context) {
    final scheme = Theme.of(context).colorScheme;
    return Scaffold(
      body: SafeArea(
        child: Column(
          children: [
            SizedBox(
              height: 52,
              child: Align(
                alignment: Alignment.centerRight,
                child: _isLast
                    ? null
                    : Padding(
                        padding: const EdgeInsets.only(right: 8),
                        child: TextButton(
                          onPressed: () => Navigator.pop(context),
                          child: const Text('Skip'),
                        ),
                      ),
              ),
            ),
            Expanded(
              child: PageView(
                controller: _controller,
                onPageChanged: (i) => setState(() => _page = i),
                children: _pages,
              ),
            ),
            Row(
              mainAxisAlignment: MainAxisAlignment.center,
              children: [
                for (var i = 0; i < _pages.length; i++)
                  AnimatedContainer(
                    duration: const Duration(milliseconds: 200),
                    margin: const EdgeInsets.symmetric(horizontal: 4),
                    width: i == _page ? 24 : 8,
                    height: 8,
                    decoration: BoxDecoration(
                      color: i == _page ? scheme.primary : scheme.outlineVariant,
                      borderRadius: BorderRadius.circular(4),
                    ),
                  ),
              ],
            ),
            Padding(
              padding: const EdgeInsets.fromLTRB(24, 24, 24, 24),
              child: SizedBox(
                width: double.infinity,
                child: FilledButton(
                  onPressed: _next,
                  child: Text(_isLast ? 'Get started' : 'Next'),
                ),
              ),
            ),
          ],
        ),
      ),
    );
  }
}

class _WelcomePage extends StatelessWidget {
  final IconData icon;
  final Color color;
  final String title;
  final String text;

  const _WelcomePage({
    required this.icon,
    required this.color,
    required this.title,
    required this.text,
  });

  @override
  Widget build(BuildContext context) {
    final theme = Theme.of(context);
    return Center(
      child: SingleChildScrollView(
      padding: const EdgeInsets.symmetric(horizontal: 32),
      child: ConstrainedBox(
        constraints: const BoxConstraints(maxWidth: 420),
        child: Column(
          mainAxisAlignment: MainAxisAlignment.center,
          children: [
            const SizedBox(height: 24),
            Container(
              width: 120,
              height: 120,
              decoration: BoxDecoration(
                color: color.withValues(alpha: 0.15),
                borderRadius: BorderRadius.circular(36),
              ),
              child: Icon(icon, size: 60, color: color),
            ),
            const SizedBox(height: 32),
            Text(
              title,
              textAlign: TextAlign.center,
              style: theme.textTheme.headlineSmall
                  ?.copyWith(fontWeight: FontWeight.w700),
            ),
            const SizedBox(height: 14),
            Text(
              text,
              textAlign: TextAlign.center,
              style: theme.textTheme.bodyLarge?.copyWith(
                color: theme.colorScheme.onSurfaceVariant,
                height: 1.5,
              ),
            ),
          ],
        ),
      ),
      ),
    );
  }
}

// ---------------------------------------------------------------------------
// Password health: reused and weak passwords, found on the device.
// ---------------------------------------------------------------------------
class HealthScreen extends StatefulWidget {
  const HealthScreen({super.key});

  @override
  State<HealthScreen> createState() => _HealthScreenState();
}

class _HealthScreenState extends State<HealthScreen> {
  HealthSummary? _summary;
  String? _error;

  @override
  void initState() {
    super.initState();
    _load();
  }

  Future<void> _load() async {
    try {
      final s = await passwordHealth();
      if (!mounted) return;
      setState(() {
        _summary = s;
        _error = null;
      });
    } catch (e) {
      if (!mounted) return;
      setState(() => _error = e.toString());
    }
  }

  Future<void> _fix(HealthIssue issue) async {
    try {
      final details = await getEntry(id: issue.id);
      if (!mounted) return;
      final saved = await Navigator.of(context).push<bool>(
        MaterialPageRoute(builder: (_) => EntryScreen(existing: details)),
      );
      if (saved == true) await _load();
    } catch (e) {
      if (mounted) setState(() => _error = e.toString());
    }
  }

  Widget _issueTile(HealthIssue issue, String subtitle) {
    final cat = categoryFor(issue.category);
    return ListTile(
      onTap: () => _fix(issue),
      contentPadding: const EdgeInsets.symmetric(horizontal: 16, vertical: 2),
      leading: CircleAvatar(
        radius: 20,
        backgroundColor: cat.color.withValues(alpha: 0.15),
        child: Icon(cat.icon, color: cat.color, size: 20),
      ),
      title: Text(
        issue.site,
        maxLines: 1,
        overflow: TextOverflow.ellipsis,
        style: const TextStyle(fontWeight: FontWeight.w600),
      ),
      subtitle: Text(subtitle, maxLines: 2, overflow: TextOverflow.ellipsis),
      trailing: const Icon(Icons.chevron_right_rounded),
    );
  }

  Widget _scoreCard(HealthSummary s) {
    final theme = Theme.of(context);
    final scheme = theme.colorScheme;
    final percent = ((s.healthy * 100) / s.total).round();
    final color = percent >= 80
        ? const Color(0xFF10B981)
        : (percent >= 50 ? const Color(0xFFF59E0B) : const Color(0xFFEF4444));
    final problems = s.total - s.healthy;
    return Card(
      child: Padding(
        padding: const EdgeInsets.all(20),
        child: Row(
          children: [
            SizedBox(
              width: 76,
              height: 76,
              child: Stack(
                fit: StackFit.expand,
                children: [
                  CircularProgressIndicator(
                    value: percent / 100,
                    strokeWidth: 8,
                    strokeCap: StrokeCap.round,
                    color: color,
                    backgroundColor: scheme.surfaceContainerHighest,
                  ),
                  Center(
                    child: Text(
                      '$percent%',
                      style: theme.textTheme.titleMedium
                          ?.copyWith(fontWeight: FontWeight.w700),
                    ),
                  ),
                ],
              ),
            ),
            const SizedBox(width: 20),
            Expanded(
              child: Column(
                crossAxisAlignment: CrossAxisAlignment.start,
                children: [
                  Text(
                    problems == 0
                        ? 'All passwords look healthy'
                        : '${s.healthy} of ${s.total} passwords are healthy',
                    style: theme.textTheme.titleMedium
                        ?.copyWith(fontWeight: FontWeight.w700),
                  ),
                  const SizedBox(height: 4),
                  Text(
                    problems == 0
                        ? 'No reused or weak passwords found.'
                        : '$problems ${problems == 1 ? 'entry needs' : 'entries need'} attention.',
                    style: theme.textTheme.bodyMedium
                        ?.copyWith(color: scheme.onSurfaceVariant),
                  ),
                ],
              ),
            ),
          ],
        ),
      ),
    );
  }

  Widget _body(HealthSummary s) {
    final theme = Theme.of(context);
    if (s.total == 0) {
      return Center(
        child: Padding(
          padding: const EdgeInsets.all(32),
          child: Column(
            mainAxisSize: MainAxisSize.min,
            children: [
              Icon(Icons.health_and_safety_rounded,
                  size: 56, color: theme.colorScheme.outline),
              const SizedBox(height: 16),
              Text('Nothing to check yet', style: theme.textTheme.titleLarge),
              const SizedBox(height: 8),
              Text(
                'Add some passwords and come back.',
                style: theme.textTheme.bodyMedium
                    ?.copyWith(color: theme.colorScheme.onSurfaceVariant),
              ),
            ],
          ),
        ),
      );
    }

    final reused = <int, List<HealthIssue>>{};
    final weak = <HealthIssue>[];
    for (final issue in s.issues) {
      if (issue.kind == 'reused') {
        reused.putIfAbsent(issue.group, () => []).add(issue);
      } else {
        weak.add(issue);
      }
    }
    final groupNumbers = reused.keys.toList()..sort();

    return ListView(
      padding: const EdgeInsets.fromLTRB(16, 4, 16, 32),
      children: [
        _scoreCard(s),
        const SizedBox(height: 12),
        _InfoBox(
          icon: s.issues.isEmpty
              ? Icons.check_circle_rounded
              : Icons.touch_app_rounded,
          text: s.issues.isEmpty
              ? 'Nice work. Every password is unique and not obviously weak.'
              : 'Tap an entry to change its password. The generator button '
                  'on that page makes a strong one.',
        ),
        if (groupNumbers.isNotEmpty)
          _SettingsSection(
            title: 'Reused passwords',
            children: [
              for (final g in groupNumbers) ...[
                Padding(
                  padding: const EdgeInsets.fromLTRB(16, 12, 16, 4),
                  child: Text(
                    'The same password is used on ${reused[g]!.length} sites',
                    style: theme.textTheme.labelLarge?.copyWith(
                      color: theme.colorScheme.onSurfaceVariant,
                    ),
                  ),
                ),
                for (final issue in reused[g]!)
                  _issueTile(
                    issue,
                    issue.username.isEmpty ? 'No username' : issue.username,
                  ),
              ],
            ],
          ),
        if (weak.isNotEmpty)
          _SettingsSection(
            title: 'Weak passwords',
            children: [
              for (final issue in weak) _issueTile(issue, issue.detail),
            ],
          ),
      ],
    );
  }

  @override
  Widget build(BuildContext context) {
    final s = _summary;
    return Scaffold(
      appBar: AppBar(
        title: const Text('Password health'),
        actions: [
          IconButton(
            icon: const Icon(Icons.refresh_rounded),
            tooltip: 'Check again',
            onPressed: _load,
          ),
          const SizedBox(width: 4),
        ],
      ),
      body: s != null
          ? _body(s)
          : Center(
              child: _error != null
                  ? Padding(
                      padding: const EdgeInsets.all(24),
                      child: _ErrorBox(_error!),
                    )
                  : const CircularProgressIndicator(),
            ),
    );
  }
}

// ---------------------------------------------------------------------------
// Two-factor codes: the 6-digit codes for every entry that has a 2FA key.
// ---------------------------------------------------------------------------
class CodesScreen extends StatefulWidget {
  const CodesScreen({super.key});

  @override
  State<CodesScreen> createState() => _CodesScreenState();
}

class _CodesScreenState extends State<CodesScreen> {
  List<TotpView> _codes = [];
  bool _loaded = false;
  bool _busy = false;
  String? _error;
  Timer? _timer;

  @override
  void initState() {
    super.initState();
    _load();
    // Codes are cheap to compute, so just ask again every second. This keeps
    // the countdown and the codes themselves exactly in step with the clock.
    _timer = Timer.periodic(const Duration(seconds: 1), (_) => _load());
  }

  @override
  void dispose() {
    _timer?.cancel();
    super.dispose();
  }

  Future<void> _load() async {
    if (_busy) return;
    _busy = true;
    try {
      final list = await totpCodes();
      if (!mounted) return;
      setState(() {
        _codes = list;
        _loaded = true;
        _error = null;
      });
    } catch (e) {
      if (mounted) setState(() => _error = e.toString());
    } finally {
      _busy = false;
    }
  }

  /// "123456" -> "123 456" so it is easier to read and type.
  String _group(String code) {
    final half = code.length ~/ 2;
    return '${code.substring(0, half)} ${code.substring(half)}';
  }

  Future<void> _copy(TotpView c) async {
    await copyWithAutoClear(c.code);
    if (!mounted) return;
    ScaffoldMessenger.of(context)
      ..hideCurrentSnackBar()
      ..showSnackBar(
        SnackBar(
          content: Text(
            'Code copied. The clipboard clears in '
            '${settings.clipboardSeconds} seconds.',
          ),
        ),
      );
  }

  Widget _codeCard(TotpView c) {
    final theme = Theme.of(context);
    final scheme = theme.colorScheme;
    final cat = categoryFor(c.category);
    final urgent = c.secondsRemaining <= 5;
    final color = urgent ? scheme.error : scheme.primary;
    return Padding(
      padding: const EdgeInsets.symmetric(horizontal: 16, vertical: 4),
      child: Card(
        clipBehavior: Clip.antiAlias,
        child: InkWell(
          onTap: () => _copy(c),
          child: Padding(
            padding: const EdgeInsets.all(16),
            child: Row(
              children: [
                CircleAvatar(
                  radius: 22,
                  backgroundColor: cat.color.withValues(alpha: 0.15),
                  child: Icon(cat.icon, color: cat.color, size: 22),
                ),
                const SizedBox(width: 14),
                Expanded(
                  child: Column(
                    crossAxisAlignment: CrossAxisAlignment.start,
                    children: [
                      Text(
                        c.site,
                        maxLines: 1,
                        overflow: TextOverflow.ellipsis,
                        style: theme.textTheme.titleMedium
                            ?.copyWith(fontWeight: FontWeight.w600),
                      ),
                      if (c.username.isNotEmpty)
                        Text(
                          c.username,
                          maxLines: 1,
                          overflow: TextOverflow.ellipsis,
                          style: theme.textTheme.bodyMedium
                              ?.copyWith(color: scheme.onSurfaceVariant),
                        ),
                      const SizedBox(height: 6),
                      Text(
                        _group(c.code),
                        style: TextStyle(
                          fontFamily: 'monospace',
                          fontSize: 30,
                          fontWeight: FontWeight.w700,
                          letterSpacing: 2,
                          color: color,
                        ),
                      ),
                    ],
                  ),
                ),
                SizedBox(
                  width: 46,
                  height: 46,
                  child: Stack(
                    fit: StackFit.expand,
                    children: [
                      CircularProgressIndicator(
                        value: c.secondsRemaining / c.period,
                        strokeWidth: 4,
                        color: color,
                        backgroundColor: scheme.surfaceContainerHighest,
                      ),
                      Center(
                        child: Text(
                          '${c.secondsRemaining}',
                          style: theme.textTheme.labelLarge
                              ?.copyWith(fontWeight: FontWeight.w700),
                        ),
                      ),
                    ],
                  ),
                ),
              ],
            ),
          ),
        ),
      ),
    );
  }

  @override
  Widget build(BuildContext context) {
    final theme = Theme.of(context);
    Widget body;
    if (!_loaded) {
      body = Center(
        child: _error != null
            ? Padding(padding: const EdgeInsets.all(24), child: _ErrorBox(_error!))
            : const CircularProgressIndicator(),
      );
    } else if (_codes.isEmpty) {
      body = Center(
        child: Padding(
          padding: const EdgeInsets.all(32),
          child: Column(
            mainAxisSize: MainAxisSize.min,
            children: [
              Icon(Icons.pin_rounded, size: 56, color: theme.colorScheme.outline),
              const SizedBox(height: 16),
              Text('No two-factor codes yet', style: theme.textTheme.titleLarge),
              const SizedBox(height: 8),
              Text(
                'Open an entry and paste the setup key from the website\'s '
                'two-factor page into the "Two-factor setup key" box.',
                textAlign: TextAlign.center,
                style: theme.textTheme.bodyMedium
                    ?.copyWith(color: theme.colorScheme.onSurfaceVariant),
              ),
            ],
          ),
        ),
      );
    } else {
      body = ListView(
        padding: const EdgeInsets.fromLTRB(0, 4, 0, 32),
        children: [
          Padding(
            padding: const EdgeInsets.fromLTRB(16, 0, 16, 8),
            child: Text(
              'Tap a code to copy it. Codes depend on your device clock, so '
              'keep "set time automatically" turned on.',
              style: theme.textTheme.bodySmall
                  ?.copyWith(color: theme.colorScheme.onSurfaceVariant),
            ),
          ),
          for (final c in _codes) _codeCard(c),
        ],
      );
    }
    return Scaffold(
      appBar: AppBar(title: const Text('Two-factor codes')),
      body: body,
    );
  }
}

// ---------------------------------------------------------------------------
// Sync: bring a phone and a PC back in step by merging their vault files.
// Only the encrypted vault file ever moves between devices.
// ---------------------------------------------------------------------------

/// Saves a copy of the encrypted vault wherever the user chooses.
/// Returns true if a file was saved.
Future<bool> saveVaultCopy() async {
  pickerOpen = true;
  try {
    final bytes = await exportBackupBytes();
    final stamp = DateTime.now().toIso8601String().substring(0, 10);
    final name = 'vault-sync-$stamp.vault';
    if (Platform.isAndroid || Platform.isIOS) {
      final saved = await fp.FilePicker.saveFile(
        dialogTitle: 'Save a copy of your vault',
        fileName: name,
        bytes: bytes,
      );
      return saved != null;
    }
    final location = await getSaveLocation(
      suggestedName: name,
      acceptedTypeGroups: [vaultTypeGroup],
    );
    if (location == null) return false;
    await File(location.path).writeAsBytes(bytes);
    return true;
  } finally {
    pickerOpen = false;
  }
}

class SyncScreen extends StatefulWidget {
  const SyncScreen({super.key});

  @override
  State<SyncScreen> createState() => _SyncScreenState();
}

class _SyncScreenState extends State<SyncScreen> {
  bool _busy = false;
  String? _error;
  MergeSummary? _result;

  void _toast(String message) {
    if (!mounted) return;
    ScaffoldMessenger.of(context)
      ..hideCurrentSnackBar()
      ..showSnackBar(SnackBar(content: Text(message)));
  }

  Future<void> _send() async {
    try {
      final saved = await saveVaultCopy();
      if (saved) _toast('Saved. Move this file to your other device.');
    } catch (e) {
      if (mounted) setState(() => _error = e.toString());
    }
  }

  Future<void> _merge() async {
    setState(() {
      _error = null;
      _result = null;
    });
    try {
      XFile? file;
      pickerOpen = true;
      try {
        file = await openFile(
          acceptedTypeGroups:
              Platform.isAndroid ? const <XTypeGroup>[] : [vaultTypeGroup],
        );
      } finally {
        pickerOpen = false;
      }
      if (file == null || !mounted) return;
      _ensureReadable(file);
      final bytes = await file.readAsBytes();

      setState(() => _busy = true);
      var summary = await mergeVaultBytes(bytes: bytes, password: '');
      if (summary.needsPassword) {
        if (!mounted) return;
        setState(() => _busy = false);
        final pw = await showDialog<String>(
          context: context,
          builder: (_) => const PasswordPromptDialog(
            title: 'Password for that file',
            label: 'Its master password',
          ),
        );
        if (pw == null || !mounted) return;
        setState(() => _busy = true);
        summary = await mergeVaultBytes(bytes: bytes, password: pw);
      }
      if (!mounted) return;
      setState(() {
        _busy = false;
        _result = summary;
      });
    } catch (e) {
      if (mounted) {
        setState(() {
          _busy = false;
          _error = e.toString();
        });
      }
    }
  }

  Widget _step(int number, String text) {
    final theme = Theme.of(context);
    return Padding(
      padding: const EdgeInsets.only(bottom: 12),
      child: Row(
        crossAxisAlignment: CrossAxisAlignment.start,
        children: [
          CircleAvatar(
            radius: 12,
            backgroundColor: theme.colorScheme.primaryContainer,
            child: Text(
              '$number',
              style: TextStyle(
                fontSize: 13,
                fontWeight: FontWeight.w700,
                color: theme.colorScheme.onPrimaryContainer,
              ),
            ),
          ),
          const SizedBox(width: 12),
          Expanded(child: Text(text, style: theme.textTheme.bodyMedium)),
        ],
      ),
    );
  }

  Widget _resultCard(MergeSummary r) {
    final theme = Theme.of(context);
    final scheme = theme.colorScheme;
    final changed = r.added + r.updated + r.deleted;
    return Card(
      child: Padding(
        padding: const EdgeInsets.all(16),
        child: Column(
          crossAxisAlignment: CrossAxisAlignment.start,
          children: [
            Row(
              children: [
                Icon(Icons.check_circle_rounded, color: scheme.primary),
                const SizedBox(width: 10),
                Text(
                  changed == 0 ? 'Already up to date' : 'Merged',
                  style: theme.textTheme.titleMedium
                      ?.copyWith(fontWeight: FontWeight.w700),
                ),
              ],
            ),
            const SizedBox(height: 10),
            Text(
              '${r.added} added\n'
              '${r.updated} updated\n'
              '${r.deleted} removed (deleted on the other device)\n'
              '${r.unchanged} already the same',
              style: theme.textTheme.bodyMedium?.copyWith(height: 1.5),
            ),
            if (changed > 0) ...[
              const SizedBox(height: 12),
              Text(
                'Now save a copy for the other device, so it receives the '
                'changes made here too.',
                style: theme.textTheme.bodySmall
                    ?.copyWith(color: scheme.onSurfaceVariant),
              ),
              const SizedBox(height: 10),
              OutlinedButton.icon(
                onPressed: _send,
                icon: const Icon(Icons.upload_file_rounded),
                label: const Text('Save a copy for the other device'),
              ),
            ],
          ],
        ),
      ),
    );
  }

  @override
  Widget build(BuildContext context) {
    final theme = Theme.of(context);
    final r = _result;
    return Scaffold(
      appBar: AppBar(title: const Text('Sync with another device')),
      body: SafeArea(
        child: ListView(
          padding: const EdgeInsets.fromLTRB(16, 4, 16, 32),
          children: [
            const _InfoBox(
              icon: Icons.lock_rounded,
              text: 'Only the encrypted vault file moves between your devices, '
                  'and the app never uses the internet. Move the file yourself '
                  '(USB cable, Nearby Share, a cloud drive, anything). It is '
                  'safe to carry because it is encrypted.',
            ),
            const SizedBox(height: 20),
            Text(
              'How to sync',
              style: theme.textTheme.titleMedium
                  ?.copyWith(fontWeight: FontWeight.w700),
            ),
            const SizedBox(height: 12),
            _step(
              1,
              'First time only: on the first device tap "Send my vault", move '
              'the file to the second device, and use Settings > Restore from '
              'backup there. Now both start from the same vault.',
            ),
            _step(
              2,
              'Later, on the device where you made changes, tap "Send my '
              'vault" and move the file to the other device.',
            ),
            _step(
              3,
              'On the other device tap "Merge a vault file" and choose that '
              'file. The changes are combined, and nothing is lost.',
            ),
            _step(
              4,
              'To bring changes back the other way, repeat in the opposite '
              'direction (or save the merged copy offered after merging).',
            ),
            const SizedBox(height: 4),
            Text(
              'If the same entry was edited on both devices, the most recent '
              'edit wins, so keep both devices\' clocks set automatically. '
              'Deleted entries stay deleted unless they were edited later on '
              'the other device.',
              style: theme.textTheme.bodySmall
                  ?.copyWith(color: theme.colorScheme.onSurfaceVariant),
            ),
            const SizedBox(height: 24),
            FilledButton.icon(
              onPressed: _busy ? null : _send,
              icon: const Icon(Icons.upload_file_rounded),
              label: const Text('Send my vault'),
            ),
            const SizedBox(height: 12),
            OutlinedButton.icon(
              onPressed: _busy ? null : _merge,
              icon: _busy
                  ? const SizedBox(
                      width: 18,
                      height: 18,
                      child: CircularProgressIndicator(strokeWidth: 2),
                    )
                  : const Icon(Icons.merge_type_rounded),
              label: Text(_busy ? 'Merging...' : 'Merge a vault file'),
            ),
            if (_error != null) ...[
              const SizedBox(height: 16),
              _ErrorBox(_error!),
            ],
            if (r != null) ...[
              const SizedBox(height: 16),
              _resultCard(r),
            ],
          ],
        ),
      ),
    );
  }
}