// Games tab: small HTML games under <content>/games/<id>/ (index.html + game.json), served
// from a tiny local HTTP server and played in a WebView. Works without the library or a hub.
//
// Games come from the hub's catalog (game:<id> components, installed by content.dart) or are
// bundled in the APK (assets/games/, copied out on start): all of ours are, so a fresh install
// has the whole set. Third-party games (scripts/third_party_games.sh) come only from a hub. The server listens on a fixed port because the WebView keeps localStorage (scores,
// saved games) per origin, and the origin includes the port.

import 'dart:convert';
import 'dart:io';

import 'package:flutter/material.dart';
import 'package:flutter/services.dart';
import 'package:webview_flutter/webview_flutter.dart';

import 'content.dart';

const bundledGames = ['balloons', 'chess', 'dots', 'draughts', 'emberdeep', 'estates', 'four', 'freecell', 'greenhollow', 'leap', 'mancala', 'morris', 'party', 'reversi', 'seabattle', 'snake', 'solitaire', 'sudoku', 'tinkerworks', 'yacht'];
const _port = 47801;

class GameInfo {
  GameInfo(this.id, Map<String, dynamic> j)
      : title = j['title'] as String? ?? id,
        description = j['description'] as String? ?? '',
        players = j['players'] as String? ?? '1',
        license = j['license'] as String? ?? '',
        cover = j['cover'] as String?,
        version = (j['version'] as num?)?.toInt() ?? 0;
  final String id, title, description, players, license;
  final String? cover;
  final int version;
}

class Games {
  Games._(this.dir);
  final Directory dir;
  HttpServer? _server;

  static Future<Games> open(ContentStore store) async {
    final g = Games._(Directory(store.gamesDir));
    await g.dir.create(recursive: true);
    await g._copyBundled();
    return g;
  }

  /// Copies bundled games out of the APK when missing or older than the bundled version.
  Future<void> _copyBundled() async {
    final manifest = await AssetManifest.loadFromAssetBundle(rootBundle);
    for (final id in bundledGames) {
      final meta = jsonDecode(await rootBundle.loadString('assets/games/$id/game.json')) as Map<String, dynamic>;
      final have = await _read(id);
      if (have != null && have.version >= ((meta['version'] as num?)?.toInt() ?? 0)) continue;
      for (final a in manifest.listAssets().where((a) => a.startsWith('assets/games/$id/'))) {
        final f = File('${dir.path}/${a.substring('assets/games/'.length)}');
        await f.parent.create(recursive: true);
        final data = await rootBundle.load(a);
        await f.writeAsBytes(data.buffer.asUint8List(data.offsetInBytes, data.lengthInBytes));
      }
    }
  }

  Future<GameInfo?> _read(String id) async {
    final f = File('${dir.path}/$id/game.json');
    if (!await f.exists() || !await File('${dir.path}/$id/index.html').exists()) return null;
    try {
      return GameInfo(id, jsonDecode(await f.readAsString()) as Map<String, dynamic>);
    } catch (_) {
      return null;
    }
  }

  Future<List<GameInfo>> list() async {
    final out = <GameInfo>[];
    await for (final e in dir.list()) {
      if (e is! Directory) continue;
      final g = await _read(e.uri.pathSegments.where((s) => s.isNotEmpty).last);
      if (g != null) out.add(g);
    }
    out.sort((a, b) => a.title.compareTo(b.title));
    return out;
  }

  /// Starts the file server if needed; returns its base URL.
  Future<Uri> serve() async {
    var s = _server;
    if (s == null) {
      try {
        s = await HttpServer.bind(InternetAddress.loopbackIPv4, _port);
      } on SocketException {
        s = await HttpServer.bind(InternetAddress.loopbackIPv4, 0); // port taken: saves won't carry over
      }
      _server = s;
      s.listen(_handle);
    }
    return Uri.parse('http://127.0.0.1:${s.port}/');
  }

  static const _types = {
    'html': 'text/html; charset=utf-8', 'js': 'text/javascript', 'mjs': 'text/javascript', 'css': 'text/css',
    'json': 'application/json', 'png': 'image/png', 'svg': 'image/svg+xml', 'webp': 'image/webp',
    'wasm': 'application/wasm', 'ogg': 'audio/ogg', 'mp3': 'audio/mpeg', 'wav': 'audio/wav',
  };

  Future<void> _handle(HttpRequest r) async {
    final segs = r.uri.pathSegments.where((s) => s.isNotEmpty).toList();
    if (segs.any((s) => s == '..' || s.startsWith('.'))) {
      r.response.statusCode = 400;
      return r.response.close();
    }
    var f = File([dir.path, ...segs].join('/'));
    if (await FileSystemEntity.isDirectory(f.path)) f = File('${f.path}/index.html');
    if (!await f.exists()) {
      r.response.statusCode = 404;
      return r.response.close();
    }
    final ext = f.path.split('.').last.toLowerCase();
    r.response.headers
      ..set('content-type', _types[ext] ?? 'application/octet-stream')
      ..set('cache-control', 'no-cache');
    await r.response.addStream(f.openRead());
    await r.response.close();
  }
}

class GamesPage extends StatefulWidget {
  const GamesPage({super.key, required this.games, required this.onGetMore, this.preloaded});
  final Games games;
  /// The list, when already read (screenshots)
  final List<GameInfo>? preloaded;
  final VoidCallback onGetMore;
  @override
  State<GamesPage> createState() => _GamesPageState();
}

class _GamesPageState extends State<GamesPage> {
  late Future<List<GameInfo>> list = widget.preloaded != null ? Future.value(widget.preloaded) : widget.games.list();

  Future<void> play(GameInfo g) async {
    final base = await widget.games.serve();
    if (!mounted) return;
    await Navigator.of(context).push(MaterialPageRoute(
      builder: (_) => GameScreen(title: g.title, url: base.resolve('${g.id}/')),
    ));
  }

  void about(GameInfo g) {
    final t = Theme.of(context);
    showModalBottomSheet<void>(
      context: context,
      showDragHandle: true,
      builder: (ctx) => Padding(
        padding: const EdgeInsets.fromLTRB(20, 0, 20, 24),
        child: Column(mainAxisSize: MainAxisSize.min, crossAxisAlignment: CrossAxisAlignment.start, children: [
          Text(g.title, style: t.textTheme.titleLarge),
          const SizedBox(height: 4),
          Text('${_players(g)} · ${g.license}', style: t.textTheme.bodySmall?.copyWith(color: t.colorScheme.onSurfaceVariant)),
          const SizedBox(height: 12),
          Text(g.description),
          const SizedBox(height: 20),
          SizedBox(
            width: double.infinity,
            child: FilledButton.icon(
              onPressed: () {
                Navigator.pop(ctx);
                play(g);
              },
              icon: const Icon(Icons.play_arrow),
              label: const Text('Play'),
            ),
          ),
        ]),
      ),
    );
  }

  static String _players(GameInfo g) => g.players == '1' ? '1 player' : '${g.players} players';

  @override
  Widget build(BuildContext context) {
    final t = Theme.of(context);
    return FutureBuilder<List<GameInfo>>(
      future: list,
      builder: (context, snap) {
        final games = snap.data;
        if (games == null) return const Center(child: CircularProgressIndicator());
        return CustomScrollView(slivers: [
          SliverToBoxAdapter(
            child: Padding(
              padding: const EdgeInsets.fromLTRB(20, 16, 20, 4),
              child: Text('Games', style: t.textTheme.headlineMedium),
            ),
          ),
          SliverToBoxAdapter(
            child: Padding(
              padding: const EdgeInsets.fromLTRB(20, 0, 20, 12),
              child: Text('All offline. Tap to play, long-press for details.',
                  style: t.textTheme.bodyMedium?.copyWith(color: t.colorScheme.onSurfaceVariant)),
            ),
          ),
          SliverPadding(
            padding: const EdgeInsets.symmetric(horizontal: 12),
            sliver: SliverGrid.count(
              crossAxisCount: 3,
              mainAxisSpacing: 10,
              crossAxisSpacing: 10,
              childAspectRatio: 0.8,
              children: [for (final g in games) _GameTile(game: g, dir: widget.games.dir, onTap: () => play(g), onLongPress: () => about(g))],
            ),
          ),
          SliverToBoxAdapter(
            child: Padding(
              padding: const EdgeInsets.all(16),
              child: OutlinedButton.icon(onPressed: widget.onGetMore, icon: const Icon(Icons.add), label: const Text('More games from a hub')),
            ),
          ),
        ]);
      },
    );
  }
}

class _GameTile extends StatelessWidget {
  const _GameTile({required this.game, required this.dir, required this.onTap, required this.onLongPress});
  final GameInfo game;
  final Directory dir;
  final VoidCallback onTap, onLongPress;

  @override
  Widget build(BuildContext context) {
    final t = Theme.of(context);
    final cover = game.cover == null ? null : File('${dir.path}/${game.id}/${game.cover}');
    final placeholder = Container(
      color: t.colorScheme.primaryContainer,
      alignment: Alignment.center,
      child: Text(game.title.characters.first, style: t.textTheme.headlineMedium?.copyWith(color: t.colorScheme.primary)),
    );
    return Card(
      child: InkWell(
        onTap: onTap,
        onLongPress: onLongPress,
        child: Column(crossAxisAlignment: CrossAxisAlignment.stretch, children: [
          AspectRatio(
            aspectRatio: 4 / 3,
            child: cover == null
                ? placeholder
                : Image.file(cover, fit: BoxFit.cover, cacheWidth: 400, errorBuilder: (_, _, _) => placeholder),
          ),
          Expanded(
            child: Padding(
              padding: const EdgeInsets.fromLTRB(8, 6, 8, 4),
              child: Column(crossAxisAlignment: CrossAxisAlignment.start, children: [
                Text(game.title, maxLines: 1, overflow: TextOverflow.ellipsis, style: t.textTheme.titleSmall),
                Text(game.players == '1' ? '1 player' : '${game.players} players',
                    maxLines: 1, style: t.textTheme.bodySmall?.copyWith(color: t.colorScheme.onSurfaceVariant)),
              ]),
            ),
          ),
        ]),
      ),
    );
  }
}

/// A game, full screen. Back leaves the game (it saves its own state).
class GameScreen extends StatefulWidget {
  const GameScreen({super.key, required this.title, required this.url});
  final String title;
  final Uri url;
  @override
  State<GameScreen> createState() => _GameScreenState();
}

const _paperIfTransparent = '''(() => {
  const clear = e => { const s = getComputedStyle(e), c = s.backgroundColor; return (c === 'transparent' || c === 'rgba(0, 0, 0, 0)') && s.backgroundImage === 'none'; };
  if (document.body && clear(document.documentElement) && clear(document.body)) {
    document.documentElement.style.background = '#fff'; document.documentElement.style.color = '#111';
    document.documentElement.style.colorScheme = 'light';
  }
})();''';

class _GameScreenState extends State<GameScreen> with WidgetsBindingObserver {
  late final WebViewController c = WebViewController()
    ..setJavaScriptMode(JavaScriptMode.unrestricted)
    ..setBackgroundColor(const Color(0xff11161c))
    ..setNavigationDelegate(NavigationDelegate(
      // Pages that leave their background transparent (Tatham's puzzles and their help pages) assume
      // white paper; on our dark WebView background their black text would be unreadable.
      onPageFinished: (_) => c.runJavaScript(_paperIfTransparent),
      onNavigationRequest: (r) =>
          Uri.tryParse(r.url)?.host == '127.0.0.1' ? NavigationDecision.navigate : NavigationDecision.prevent,
    ))
    ..loadRequest(widget.url);

  @override
  void initState() {
    super.initState();
    WidgetsBinding.instance.addObserver(this);
    // Without these, Android's WebView drops alert() and answers confirm()/prompt() with no/empty.
    c.setOnJavaScriptAlertDialog((r) => _dialog<void>(r.message));
    c.setOnJavaScriptConfirmDialog((r) async => await _dialog<bool>(r.message, ok: true, cancel: false) ?? false);
    c.setOnJavaScriptTextInputDialog((r) async {
      final t = TextEditingController(text: r.defaultText ?? '');
      return await _dialog<String>(r.message, field: t, ok: null, cancel: '') ?? '';
    });
  }

  /// A Flutter dialog for a JS alert/confirm/prompt. [ok] is returned by OK (the field's text when
  /// [field] is set); [cancel] by Cancel, which is hidden when null (alerts).
  Future<T?> _dialog<T>(String message, {T? ok, T? cancel, TextEditingController? field}) {
    return showDialog<T>(
      context: context,
      builder: (ctx) => AlertDialog(
        content: field == null
            ? Text(message)
            : Column(mainAxisSize: MainAxisSize.min, crossAxisAlignment: CrossAxisAlignment.start, children: [
                Text(message),
                TextField(controller: field, autofocus: true),
              ]),
        actions: [
          if (cancel != null) TextButton(onPressed: () => Navigator.pop(ctx, cancel), child: const Text('Cancel')),
          TextButton(onPressed: () => Navigator.pop(ctx, field != null ? field.text as T : ok), child: const Text('OK')),
        ],
      ),
    );
  }

  @override
  void dispose() {
    WidgetsBinding.instance.removeObserver(this);
    super.dispose();
  }

  // The WebView doesn't always mark the page hidden when the app goes to the background, so games
  // also listen for these events to pause their clocks.
  @override
  void didChangeAppLifecycleState(AppLifecycleState s) {
    final ev = s == AppLifecycleState.resumed ? 'sp-resume' : 'sp-pause';
    c.runJavaScript("window.dispatchEvent(new Event('$ev'))").catchError((_) {});
  }

  @override
  Widget build(BuildContext context) {
    return Scaffold(
      backgroundColor: const Color(0xff11161c),
      body: SafeArea(child: WebViewWidget(controller: c)),
    );
  }
}
