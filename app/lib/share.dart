// Sharing this phone with people nearby: its server (library, maps, games, chat) is opened on the
// network, so anyone on the same Wi-Fi uses it in a browser, iPhones included, with no app and no
// internet. The phone can start its own Wi-Fi network (Android's local-only hotspot; Share.kt) or use
// a hotspot or Wi-Fi it is already on. Two QR codes do the rest: one joins the Wi-Fi, one opens the page.

import 'dart:async';
import 'dart:convert';
import 'dart:io';
import 'dart:math';

import 'package:flutter/material.dart';
import 'package:flutter/services.dart';
import 'package:http/http.dart' as http;
import 'package:qr_flutter/qr_flutter.dart';
import 'package:shared_preferences/shared_preferences.dart';
import 'package:webview_flutter/webview_flutter.dart';
import 'package:webview_flutter_android/webview_flutter_android.dart';

import 'content.dart';
import 'core.dart';

enum ShareMode { hotspot, existing }

const _channel = MethodChannel('survivalpedia/share');

class ShareController extends ChangeNotifier {
  ShareController({required this.ensureCore, required this.localPort}) {
    _channel.setMethodCallHandler((call) async {
      if (call.method == 'stopRequested') {
        await stop();
      } else if (call.method == 'hotspotStopped' && mode == ShareMode.hotspot && on) {
        await stop();
        error = 'The Wi-Fi network was turned off by Android.';
        notifyListeners();
      }
    });
  }

  /// Starts the embedded server if it isn't running (it may not be, with no content installed).
  final Future<void> Function() ensureCore;

  /// The server's port on 127.0.0.1 (for this phone's own chat).
  final int? Function() localPort;

  static const basePort = 8080;

  bool on = false, starting = false;
  ShareMode? mode;
  String? ssid, password, error, errorCode;
  int port = 0;
  List<String> addresses = [];
  Timer? _timer;

  String? get url => addresses.isEmpty || port == 0 ? null : 'http://${addresses.first}${port == 80 ? '' : ':$port'}/';

  Future<void> start(ShareMode m) async {
    starting = true;
    error = null;
    errorCode = null;
    notifyListeners();
    try {
      if (m == ShareMode.hotspot) {
        final r = await _channel.invokeMapMethod<String, dynamic>('startHotspot');
        ssid = r?['ssid'] as String?;
        password = r?['password'] as String?;
      } else {
        ssid = password = null;
      }
      await ensureCore();
      port = await Core.share(basePort);
      if (port == 0) throw PlatformException(code: 'core', message: 'The library server is not running');
      await _channel.invokeMethod<void>('startService');
      on = true;
      mode = m;
      await _refresh();
      // the hotspot's address can take a moment to appear, and Wi-Fi can change
      _timer = Timer.periodic(const Duration(seconds: 3), (_) => _refresh());
    } on PlatformException catch (e) {
      error = e.message ?? e.code;
      errorCode = e.code;
      if (m == ShareMode.hotspot) await _channel.invokeMethod<void>('stopHotspot');
    } catch (e) {
      error = '$e';
    } finally {
      starting = false;
      notifyListeners();
    }
  }

  Future<void> stop() async {
    _timer?.cancel();
    _timer = null;
    on = false;
    notifyListeners();
    await Core.share(0);
    await _channel.invokeMethod<void>('stopHotspot');
    await _channel.invokeMethod<void>('stopService');
    port = 0;
    addresses = [];
    notifyListeners();
  }

  Future<void> _refresh() async {
    final a = await lanAddresses();
    if (a.join() != addresses.join()) {
      addresses = a;
      notifyListeners();
    }
  }

  Future<void> openSettings(String which) => _channel.invokeMethod<void>(which);
}

/// This phone's IPv4 addresses that others could reach, best first: hotspot interfaces, then Wi-Fi,
/// then wired. Mobile data, VPN and link-local addresses are left out.
Future<List<String>> lanAddresses() async {
  final ifs = await NetworkInterface.list(type: InternetAddressType.IPv4);
  int? rank(String n) {
    n = n.toLowerCase();
    if (RegExp(r'^(rmnet|ccmni|tun|ppp|dummy|v4-|clat|ipsec|lo)').hasMatch(n)) return null;
    if (RegExp(r'^(ap|swlan|softap|wlan1|wigig)').hasMatch(n)) return 0;
    if (n.startsWith('wlan')) return 1;
    if (n.startsWith('eth') || n.startsWith('en')) return 2;
    return 3;
  }

  final out = <(int, String)>[];
  for (final i in ifs) {
    final r = rank(i.name);
    if (r == null) continue;
    for (final a in i.addresses) {
      if (a.isLoopback || a.isLinkLocal) continue;
      out.add((r, a.address));
    }
  }
  out.sort((a, b) => a.$1.compareTo(b.$1));
  return [for (final o in out) o.$2];
}

/// The standard Wi-Fi QR code text, which Android and iPhone cameras both understand.
String wifiQr(String ssid, String? password) {
  String e(String s) => s.replaceAllMapped(RegExp(r'([\\;,:"])'), (m) => '\\${m[1]}');
  return password == null || password.isEmpty ? 'WIFI:T:nopass;S:${e(ssid)};;' : 'WIFI:T:WPA;S:${e(ssid)};P:${e(password)};;';
}

class SharePage extends StatefulWidget {
  const SharePage({super.key, required this.share});
  final ShareController share;
  @override
  State<SharePage> createState() => _SharePageState();
}

class _SharePageState extends State<SharePage> {
  ShareController get s => widget.share;

  @override
  void initState() {
    super.initState();
    s.addListener(_changed);
  }

  @override
  void dispose() {
    s.removeListener(_changed);
    super.dispose();
  }

  void _changed() {
    if (mounted) setState(() {});
  }

  @override
  Widget build(BuildContext context) {
    return Scaffold(
      appBar: AppBar(title: const Text('Share nearby'), backgroundColor: Theme.of(context).colorScheme.surface),
      body: SafeArea(child: s.on ? _sharing(context) : _off(context)),
    );
  }

  Widget _off(BuildContext context) {
    final t = Theme.of(context);
    final muted = t.textTheme.bodyMedium?.copyWith(color: t.colorScheme.onSurfaceVariant);
    return ListView(padding: const EdgeInsets.fromLTRB(20, 8, 20, 32), children: [
      Text('Let people near you use this phone', style: t.textTheme.titleLarge),
      const SizedBox(height: 8),
      Text(
        'Everyone around you can search the library, read, use the map, play games and chat, '
        "in their own phone's browser. iPhones too. They don't need the app or the internet.",
        style: muted,
      ),
      const SizedBox(height: 20),
      _Option(
        icon: Icons.wifi_tethering,
        title: 'Start a Wi-Fi network',
        text: 'This phone makes its own Wi-Fi. Others join by scanning a code.',
        recommended: true,
        onTap: s.starting ? null : () => s.start(ShareMode.hotspot),
      ),
      const SizedBox(height: 12),
      _Option(
        icon: Icons.wifi,
        title: 'Use my hotspot or this Wi-Fi',
        text: "If your phone's hotspot is already on, or everyone is on the same Wi-Fi.",
        onTap: s.starting ? null : () => s.start(ShareMode.existing),
      ),
      if (s.starting)
        const Padding(
          padding: EdgeInsets.only(top: 20),
          child: Row(children: [
            SizedBox(width: 18, height: 18, child: CircularProgressIndicator(strokeWidth: 2)),
            SizedBox(width: 12),
            Text('Starting…'),
          ]),
        ),
      if (s.error != null) ...[
        const SizedBox(height: 20),
        Card(
          color: t.colorScheme.errorContainer,
          child: Padding(
            padding: const EdgeInsets.all(14),
            child: Column(crossAxisAlignment: CrossAxisAlignment.start, children: [
              Text(s.error!, style: TextStyle(color: t.colorScheme.onErrorContainer)),
              if (s.errorCode == 'location_off')
                TextButton(onPressed: () => s.openSettings('locationSettings'), child: const Text('Open location settings')),
              if (s.errorCode == 'failed')
                TextButton(onPressed: () => s.openSettings('hotspotSettings'), child: const Text('Open hotspot settings')),
            ]),
          ),
        ),
      ],
      const SizedBox(height: 24),
      Row(crossAxisAlignment: CrossAxisAlignment.start, children: [
        Icon(Icons.battery_charging_full, size: 20, color: t.colorScheme.primary),
        const SizedBox(width: 10),
        Expanded(
          child: Text(
            'Sharing keeps Wi-Fi busy and uses more battery. Plug this phone into a power bank if you can; '
            'the screen can stay off. Wi-Fi reaches about 30 m indoors.',
            style: muted,
          ),
        ),
      ]),
    ]);
  }

  Widget _sharing(BuildContext context) {
    final t = Theme.of(context);
    final muted = t.textTheme.bodyMedium?.copyWith(color: t.colorScheme.onSurfaceVariant);
    final url = s.url;
    final hotspot = s.mode == ShareMode.hotspot && s.ssid != null;
    return ListView(padding: const EdgeInsets.fromLTRB(20, 8, 20, 32), children: [
      Row(children: [
        Container(width: 10, height: 10, decoration: const BoxDecoration(color: Color(0xFF3FA66B), shape: BoxShape.circle)),
        const SizedBox(width: 10),
        Expanded(child: Text('Sharing now', style: t.textTheme.titleLarge)),
      ]),
      const SizedBox(height: 4),
      Text('Show this screen to people nearby. They point their camera at each code in turn.', style: muted),
      const SizedBox(height: 16),
      _Step(
        n: 1,
        title: 'Join the Wi-Fi',
        child: hotspot
            ? Column(children: [
                _Qr(data: wifiQr(s.ssid!, s.password)),
                const SizedBox(height: 10),
                _Field(label: 'Network', value: s.ssid!),
                if (s.password != null) _Field(label: 'Password', value: s.password!),
              ])
            : Column(crossAxisAlignment: CrossAxisAlignment.start, children: [
                Text(
                  "They join this phone's hotspot, or the same Wi-Fi as this phone. Android's hotspot settings "
                  'can show a code for joining it.',
                  style: muted,
                ),
                TextButton.icon(
                  onPressed: () => s.openSettings('hotspotSettings'),
                  icon: const Icon(Icons.settings_outlined),
                  label: const Text('Hotspot settings'),
                ),
              ]),
      ),
      const SizedBox(height: 12),
      _Step(
        n: 2,
        title: 'Open Bothy',
        child: url == null
            ? Text(
                hotspot ? 'Waiting for the Wi-Fi network…' : 'This phone is not on a Wi-Fi network or hotspot yet.',
                style: muted,
              )
            : Column(children: [
                _Qr(data: url),
                const SizedBox(height: 10),
                SelectableText(url, style: t.textTheme.titleMedium?.copyWith(color: t.colorScheme.primary)),
                if (s.addresses.length > 1)
                  Padding(
                    padding: const EdgeInsets.only(top: 4),
                    child: Text(
                      'Not working? Try ${s.addresses.skip(1).map((a) => 'http://$a:${s.port}/').join(' or ')}',
                      style: t.textTheme.bodySmall?.copyWith(color: t.colorScheme.onSurfaceVariant),
                      textAlign: TextAlign.center,
                    ),
                  ),
              ]),
      ),
      const SizedBox(height: 20),
      FilledButton.icon(
        onPressed: s.localPort() == null ? null : () => openChat(context, s.localPort()!),
        icon: const Icon(Icons.forum_outlined),
        label: const Text('Open chat'),
      ),
      const SizedBox(height: 8),
      OutlinedButton.icon(onPressed: s.stop, icon: const Icon(Icons.stop_circle_outlined), label: const Text('Stop sharing')),
      const SizedBox(height: 16),
      Text(
        'Sharing carries on with the screen off or the app in the background, until you stop it here or in the '
        'notification.',
        style: muted,
      ),
      const SizedBox(height: 8),
      Text(
        'Android phones can get this app from the home page here ("Get the app"). With the app, they can copy what '
        'this phone has (articles, maps, books, games) in Content → the Wi-Fi search button. It is fastest with this '
        'phone on a charger.',
        style: muted,
      ),
    ]);
  }
}

class _Option extends StatelessWidget {
  const _Option({required this.icon, required this.title, required this.text, this.recommended = false, this.onTap});
  final IconData icon;
  final String title, text;
  final bool recommended;
  final VoidCallback? onTap;
  @override
  Widget build(BuildContext context) {
    final t = Theme.of(context);
    return Card(
      child: InkWell(
        onTap: onTap,
        child: Padding(
          padding: const EdgeInsets.all(16),
          child: Row(children: [
            Container(
              width: 48,
              height: 48,
              decoration: BoxDecoration(color: t.colorScheme.primaryContainer, borderRadius: BorderRadius.circular(14)),
              child: Icon(icon, color: t.colorScheme.primary),
            ),
            const SizedBox(width: 14),
            Expanded(
              child: Column(crossAxisAlignment: CrossAxisAlignment.start, children: [
                Row(children: [
                  Flexible(child: Text(title, style: t.textTheme.titleMedium)),
                  if (recommended) ...[
                    const SizedBox(width: 8),
                    Container(
                      padding: const EdgeInsets.symmetric(horizontal: 8, vertical: 1),
                      decoration: BoxDecoration(color: t.colorScheme.primaryContainer, borderRadius: BorderRadius.circular(99)),
                      child: Text('Best', style: t.textTheme.labelSmall?.copyWith(color: t.colorScheme.primary)),
                    ),
                  ],
                ]),
                const SizedBox(height: 2),
                Text(text, style: t.textTheme.bodyMedium?.copyWith(color: t.colorScheme.onSurfaceVariant)),
              ]),
            ),
          ]),
        ),
      ),
    );
  }
}

class _Step extends StatelessWidget {
  const _Step({required this.n, required this.title, required this.child});
  final int n;
  final String title;
  final Widget child;
  @override
  Widget build(BuildContext context) {
    final t = Theme.of(context);
    return Card(
      child: Padding(
        padding: const EdgeInsets.all(16),
        child: Column(crossAxisAlignment: CrossAxisAlignment.stretch, children: [
          Row(children: [
            CircleAvatar(
              radius: 13,
              backgroundColor: t.colorScheme.primary,
              child: Text('$n', style: TextStyle(color: t.colorScheme.onPrimary, fontWeight: FontWeight.w700, fontSize: 14)),
            ),
            const SizedBox(width: 10),
            Text(title, style: t.textTheme.titleMedium),
          ]),
          const SizedBox(height: 12),
          child,
        ]),
      ),
    );
  }
}

class _Qr extends StatelessWidget {
  const _Qr({required this.data});
  final String data;
  @override
  Widget build(BuildContext context) {
    // dark on white in both themes: cameras read it best
    return Center(
      child: Container(
        padding: const EdgeInsets.all(12),
        decoration: BoxDecoration(color: Colors.white, borderRadius: BorderRadius.circular(12)),
        child: QrImageView(
          data: data,
          size: 200,
          padding: EdgeInsets.zero,
          backgroundColor: Colors.white,
          eyeStyle: const QrEyeStyle(eyeShape: QrEyeShape.square, color: Color(0xFF1E2421)),
          dataModuleStyle: const QrDataModuleStyle(dataModuleShape: QrDataModuleShape.square, color: Color(0xFF1E2421)),
        ),
      ),
    );
  }
}

class _Field extends StatelessWidget {
  const _Field({required this.label, required this.value});
  final String label, value;
  @override
  Widget build(BuildContext context) {
    final t = Theme.of(context);
    return Padding(
      padding: const EdgeInsets.symmetric(vertical: 2),
      child: Row(children: [
        SizedBox(width: 90, child: Text(label, style: t.textTheme.bodyMedium?.copyWith(color: t.colorScheme.onSurfaceVariant))),
        Expanded(child: SelectableText(value, style: t.textTheme.titleMedium)),
      ]),
    );
  }
}

/// This phone's own identity in chat: made once, kept in preferences.
Future<(String, String)> chatIdentity() async {
  final p = await SharedPreferences.getInstance();
  String make(int n) {
    const abc = 'abcdefghijklmnopqrstuvwxyzABCDEFGHIJKLMNOPQRSTUVWXYZ0123456789';
    final r = Random.secure();
    return String.fromCharCodes(Iterable.generate(n, (_) => abc.codeUnitAt(r.nextInt(abc.length))));
  }

  var id = p.getString('chat-id'), token = p.getString('chat-token');
  if (id == null || token == null) {
    id = make(16);
    token = make(32);
    await p.setString('chat-id', id);
    await p.setString('chat-token', token);
  }
  return (id, token);
}

/// Chat on this phone's own server, as this phone's person.
Future<void> openChat(BuildContext context, int port) async {
  final (id, token) = await chatIdentity();
  if (!context.mounted) return;
  await Navigator.of(context).push(MaterialPageRoute<void>(
    builder: (_) => _ChatView(url: 'http://127.0.0.1:$port/chat#id=$id&token=$token'),
  ));
}

class _ChatView extends StatefulWidget {
  const _ChatView({required this.url});
  final String url;
  @override
  State<_ChatView> createState() => _ChatViewState();
}

class _ChatViewState extends State<_ChatView> {
  late final WebViewController c;

  @override
  void initState() {
    super.initState();
    c = WebViewController(
      // voice messages: the page asks for the microphone; Android asks the person
      onPermissionRequest: (req) async {
        if (req.types.contains(WebViewPermissionResourceType.microphone) && await _ask('mic')) {
          req.grant();
        } else {
          req.deny();
        }
      },
    )
      ..setJavaScriptMode(JavaScriptMode.unrestricted)
      ..setNavigationDelegate(NavigationDelegate(
        onNavigationRequest: (r) {
          final u = Uri.tryParse(r.url);
          if (u == null) return NavigationDecision.prevent;
          if (u.scheme == 'tel') {
            _files.invokeMethod<void>('dial', {'number': u.path});
            return NavigationDecision.prevent;
          }
          if (u.scheme == 'geo') {
            _files.invokeMethod<void>('openUri', {'uri': r.url}).catchError((Object e) {
              if (mounted) {
                ScaffoldMessenger.of(context)
                    .showSnackBar(const SnackBar(content: Text('No maps app on this phone. Use "Show on map".')));
              }
            });
            return NavigationDecision.prevent;
          }
          // the chat page, and the map for showing or picking a place; the brand link goes back to the app
          if (u.host == '127.0.0.1' && (u.path == '/chat' || u.path == '/map')) return NavigationDecision.navigate;
          Navigator.of(context).maybePop();
          return NavigationDecision.prevent;
        },
      ))
      ..loadRequest(Uri.parse(widget.url));
    final a = c.platform;
    if (a is AndroidWebViewController) {
      // the page's photo / sound file input: the gallery, the camera, or a recorder app
      a.setOnShowFileSelector((p) async {
        final audio = p.acceptTypes.any((t) => t.startsWith('audio'));
        try {
          return await _files.invokeListMethod<String>('pickMedia', {'accept': audio ? 'audio' : 'image'}) ?? [];
        } catch (_) {
          return [];
        }
      });
      // "My location": GPS works offline; Android asks the person first
      a.setGeolocationPermissionsPromptCallbacks(
        onShowPrompt: (_) async => GeolocationPermissionsResponse(allow: await _ask('location'), retain: false),
      );
    }
  }

  static const _files = MethodChannel('survivalpedia/files');

  static Future<bool> _ask(String kind) async {
    try {
      return await _files.invokeMethod<bool>('askPermission', {'kind': kind}) ?? false;
    } catch (_) {
      return false;
    }
  }

  @override
  Widget build(BuildContext context) {
    return Scaffold(
      appBar: AppBar(title: const Text('Chat'), backgroundColor: Theme.of(context).colorScheme.surface),
      body: SafeArea(child: WebViewWidget(controller: c)),
    );
  }
}

/// A phone sharing nearby, or a hub, found on this Wi-Fi by [findNearby].
class Nearby {
  Nearby(this.url, this.phone, this.size, this.parts, this.books);
  final Uri url;
  final bool phone;
  final int size, parts;
  final bool books;
}

/// Looks for Bothy servers on this phone's Wi-Fi networks: every address of each /24 on the
/// usual ports, keeping those that answer with a catalog. With a phone's hotspot that finds the phone
/// sharing it; on a shared router, any sharing phone or hub. Takes a few seconds.
Future<List<Nearby>> findNearby({void Function(double done)? progress}) async {
  final mine = (await lanAddresses()).toSet();
  final targets = <(String, int)>[];
  final seen = <String>{};
  for (final a in mine) {
    final p = a.split('.');
    if (p.length != 4) continue;
    for (var i = 1; i < 255; i++) {
      final ip = '${p[0]}.${p[1]}.${p[2]}.$i';
      if (mine.contains(ip) || !seen.add(ip)) continue;
      for (final port in const [8080, 8081]) {
        targets.add((ip, port));
      }
    }
  }
  final found = <Nearby>[];
  var next = 0, done = 0;
  Future<void> worker() async {
    while (next < targets.length) {
      final (ip, port) = targets[next++];
      try {
        final s = await Socket.connect(ip, port, timeout: const Duration(milliseconds: 700));
        s.destroy();
        final r = await http.get(Uri.parse('http://$ip:$port/api/catalog')).timeout(const Duration(seconds: 5));
        final j = jsonDecode(utf8.decode(r.bodyBytes)) as Map<String, dynamic>;
        if (r.statusCode == 200 && j['format'] == 'survivalpedia-catalog') {
          final comps = j['components'] as List? ?? [];
          found.add(Nearby(
            Uri.parse('http://$ip:$port'),
            j['source'] == 'phone',
            comps.fold<int>(0, (a, c) => a + ((c as Map)['size'] as num).toInt()),
            comps.length,
            j['books'] != null,
          ));
        }
      } catch (_) {
        // nothing there, or not ours
      }
      done++;
      progress?.call(done / targets.length);
    }
  }

  await Future.wait([for (var i = 0; i < 48; i++) worker()]);
  found.sort((a, b) => a.url.toString().compareTo(b.url.toString()));
  return found;
}

/// Finds phones sharing nearby and hubs, and lets the person pick one to download from.
Future<Uri?> pickNearby(BuildContext context) =>
    showModalBottomSheet<Uri>(context: context, showDragHandle: true, builder: (_) => const _NearbySheet());

class _NearbySheet extends StatefulWidget {
  const _NearbySheet();
  @override
  State<_NearbySheet> createState() => _NearbySheetState();
}

class _NearbySheetState extends State<_NearbySheet> {
  double done = 0;
  List<Nearby>? found;

  @override
  void initState() {
    super.initState();
    _scan();
  }

  Future<void> _scan() async {
    setState(() {
      found = null;
      done = 0;
    });
    final f = await findNearby(progress: (d) {
      if (mounted && (d - done >= 0.02 || d == 1)) setState(() => done = d);
    });
    if (mounted) setState(() => found = f);
  }

  @override
  Widget build(BuildContext context) {
    final t = Theme.of(context);
    final f = found;
    return SafeArea(
      child: Padding(
        padding: const EdgeInsets.fromLTRB(20, 0, 20, 16),
        child: Column(mainAxisSize: MainAxisSize.min, crossAxisAlignment: CrossAxisAlignment.stretch, children: [
          Text('Download from nearby', style: t.textTheme.titleLarge),
          const SizedBox(height: 4),
          Text('Phones sharing with "Share nearby", and hubs, on the Wi-Fi this phone is on.',
              style: t.textTheme.bodyMedium?.copyWith(color: t.colorScheme.onSurfaceVariant)),
          const SizedBox(height: 16),
          if (f == null) ...[
            LinearProgressIndicator(value: done),
            const SizedBox(height: 8),
            Text('Looking…', style: t.textTheme.bodySmall),
          ] else if (f.isEmpty) ...[
            const Text('Nothing found. Join the Wi-Fi of the phone that is sharing (scan its Wi-Fi code), '
                'then look again. You can also type the address it shows.'),
            const SizedBox(height: 12),
            OutlinedButton.icon(onPressed: _scan, icon: const Icon(Icons.refresh), label: const Text('Look again')),
          ] else
            for (final n in f)
              Card(
                child: ListTile(
                  leading: Icon(n.phone ? Icons.smartphone : Icons.router_outlined, color: t.colorScheme.primary),
                  title: Text(n.phone ? 'A phone sharing nearby' : 'Bothy hub'),
                  subtitle: Text([
                    '${n.url.host}:${n.url.port}',
                    if (n.parts > 0) '${n.parts} items · ${formatBytes(n.size)}',
                    if (n.books) 'books',
                  ].join(' · ')),
                  trailing: const Icon(Icons.chevron_right),
                  onTap: () => Navigator.of(context).pop(n.url),
                ),
              ),
        ]),
      ),
    );
  }
}
