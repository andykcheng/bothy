// Bothy mobile app (DESIGN §2, Milestone 3).
//
// The Rust core runs the same server as the hub on 127.0.0.1 (core.dart); the Library and
// Map tabs show it in a WebView, so search, articles and maps work offline exactly as on the
// hub. The Notes tab keeps notes and checklists on the phone (notes.dart). The Games tab plays
// small offline games (games.dart). The Content tab downloads collections, maps, games and the
// AI model from a hub over Wi-Fi (content.dart); all of it is optional — someone who only wants
// games needs nothing else. "Share nearby" (share.dart) opens this phone's server on Wi-Fi for
// people around it, in their browsers.

import 'dart:convert';
import 'dart:io';

import 'package:flutter/material.dart';
import 'package:flutter/services.dart';
import 'package:http/http.dart' as http;
import 'package:path_provider/path_provider.dart';
import 'package:shared_preferences/shared_preferences.dart';
import 'package:webview_flutter/webview_flutter.dart';
import 'package:webview_flutter_android/webview_flutter_android.dart';

import 'books.dart';
import 'content.dart';
import 'download_card.dart';
import 'core.dart';
import 'downloads.dart';
import 'emergency.dart';
import 'games.dart';
import 'map_picker.dart';
import 'notes.dart';
import 'share.dart';
import 'theme.dart';

void main() => runApp(const SurvivalpediaApp());

class SurvivalpediaApp extends StatelessWidget {
  const SurvivalpediaApp({super.key});

  @override
  Widget build(BuildContext context) {
    return MaterialApp(
      title: 'Bothy',
      debugShowCheckedModeBanner: false,
      theme: buildTheme(Brightness.light),
      darkTheme: buildTheme(Brightness.dark),
      home: const Home(),
    );
  }
}

class Home extends StatefulWidget {
  const Home({super.key});
  @override
  State<Home> createState() => _HomeState();
}

class _HomeState extends State<Home> {
  ContentStore? store;
  Downloads? downloads;
  Games? games;
  NotesStore? notes;
  late final ShareController share = ShareController(ensureCore: _ensureCore, localPort: () => port);
  String? chatDir;
  bool keepCore = false; // sharing: the server runs even with no content (chat, games)
  int? port;
  String? error;
  bool starting = false;
  int tab = 0;
  int generation = 0; // bumps when the server restarts, so the WebViews reload
  static const booksTab = 1, mapTab = 2, notesTab = 3, gamesTab = 4, contentTab = 5; // 0 is the library

  @override
  void initState() {
    super.initState();
    _init();
  }

  Future<void> _init() async {
    final n = await NotesStore.open();
    chatDir = '${(await getApplicationSupportDirectory()).path}/chat';
    share.addListener(_shareChanged);
    final s = await ContentStore.open();
    final g = await Games.open(s);
    final d = Downloads(s, onInstalled: restartCore);
    setState(() {
      notes = n;
      store = s;
      games = g;
      downloads = d;
    });
    await restartCore();
    await d.load(); // carries on with downloads left unfinished last time
  }

  void _shareChanged() {
    if (mounted) setState(() {});
  }

  Future<void> _ensureCore() async {
    keepCore = true;
    if (port == null) await restartCore();
    if (port == null) throw Exception(error ?? 'The library server did not start');
  }

  // This app's own APK, so a sharing phone can hand it to phones that don't have it yet
  String? _apk;
  Future<String?> _apkPath() async {
    if (_apk != null || !Platform.isAndroid) return _apk;
    try {
      _apk = await const MethodChannel('survivalpedia/files').invokeMethod<String>('apkPath');
    } catch (_) {}
    return _apk;
  }

  Future<void> restartCore() async {
    final s = store!;
    generation++; // also refreshes the games list after downloads
    if (!s.needsCore && !keepCore) {
      await Core.stop();
      setState(() => port = null);
      return;
    }
    setState(() {
      starting = true;
      error = null;
    });
    try {
      final p = await Core.start(
        packs: s.collectionPaths,
        model: s.modelPath,
        maps: s.hasMaps ? s.mapsDir : null,
        llm: s.hasLlm ? s.llmDir : null,
        games: s.gamesDir,
        chat: chatDir,
        installed: '${s.root.path}/installed.json',
        apk: await _apkPath(),
      );
      setState(() {
        port = p;
        generation++;
      });
    } catch (e) {
      setState(() => error = '$e');
    } finally {
      setState(() => starting = false);
    }
  }

  /// Vectors from the embedded server's model (Notes search by meaning). Throws when it can't answer.
  Future<List<List<double>>> _embed(List<String> texts, {required bool query}) async {
    final p = port;
    if (p == null) throw StateError('the library is not running');
    final r = await http
        .post(Uri.parse('http://127.0.0.1:$p/api/embed'),
            headers: {'content-type': 'application/json'}, body: jsonEncode({'texts': texts, 'query': query}))
        .timeout(const Duration(seconds: 60));
    if (r.statusCode != 200) throw StateError('embed: ${r.statusCode} ${r.body}');
    final j = jsonDecode(r.body) as Map<String, dynamic>;
    return [for (final v in j['vectors'] as List) [for (final x in v as List) (x as num).toDouble()]];
  }

  void goContent() => setState(() => tab = contentTab);

  @override
  Widget build(BuildContext context) {
    final s = store;
    Widget body;
    if (s == null) {
      body = const Center(child: CircularProgressIndicator());
    } else if (tab == contentTab) {
      body = ContentPage(store: s, downloads: downloads!, share: share);
    } else if (tab == notesTab) {
      body = NotesPage(store: notes!, embed: port != null && s.modelPath != null ? _embed : null);
    } else if (tab == booksTab) {
      body = BooksPage(store: s, downloads: downloads!);
    } else if (tab == gamesTab) {
      body = GamesPage(key: ValueKey('games$generation'), games: games!, onGetMore: goContent);
    } else if (tab == 0 && !s.hasLibrary) {
      body = _Message(
        icon: Icons.menu_book_outlined,
        title: 'No library yet',
        text: 'The library is optional. Pick the collections you want — survival guides, wikiHow, Wikipedia Essentials — '
            'and download them from a Bothy hub over Wi-Fi. Books are in the Books tab.',
        action: ('Get collections', goContent),
        secondary: ('Play games', () => setState(() => tab = gamesTab)),
        firstAid: true,
      );
    } else if (tab == mapTab && !s.hasMaps) {
      body = _Message(
        icon: Icons.map_outlined,
        title: 'No maps yet',
        text: 'Add a map region from a hub in Content. Maps work fully offline.',
        action: ('Get maps', goContent),
      );
    } else if (starting) {
      body = const Center(child: Column(mainAxisSize: MainAxisSize.min, children: [
        CircularProgressIndicator(), SizedBox(height: 12), Text('Opening the library…'),
      ]));
    } else if (error != null || port == null) {
      body = _Message(
        icon: Icons.error_outline,
        title: 'Could not open the library',
        text: error ?? 'The library did not start.',
        action: ('Retry', restartCore),
        firstAid: tab == 0,
      );
    } else {
      // keep both WebViews alive so switching tabs keeps their pages
      body = IndexedStack(index: tab == mapTab ? 1 : 0, children: [
        s.hasLibrary ? WebTab(key: ValueKey('lib$generation'), url: 'http://127.0.0.1:$port/') : const SizedBox(),
        s.hasMaps ? WebTab(key: ValueKey('map$generation'), url: 'http://127.0.0.1:$port/map') : const SizedBox(),
      ]);
    }
    return Scaffold(
      body: SafeArea(child: body),
      bottomNavigationBar: Column(mainAxisSize: MainAxisSize.min, children: [
        if (share.on) _SharingStrip(share: share),
        NavigationBar(
          selectedIndex: tab,
          onDestinationSelected: (i) => setState(() => tab = i),
          destinations: const [
            NavigationDestination(icon: Icon(Icons.travel_explore_outlined), selectedIcon: Icon(Icons.travel_explore), label: 'Library'),
            NavigationDestination(icon: Icon(Icons.auto_stories_outlined), selectedIcon: Icon(Icons.auto_stories), label: 'Books'),
            NavigationDestination(icon: Icon(Icons.map_outlined), selectedIcon: Icon(Icons.map), label: 'Map'),
            NavigationDestination(icon: Icon(Icons.sticky_note_2_outlined), selectedIcon: Icon(Icons.sticky_note_2), label: 'Notes'),
            NavigationDestination(icon: Icon(Icons.sports_esports_outlined), selectedIcon: Icon(Icons.sports_esports), label: 'Games'),
            NavigationDestination(icon: Icon(Icons.download_outlined), selectedIcon: Icon(Icons.download), label: 'Content'),
          ],
        ),
      ]),
    );
  }
}

/// Shown above the tabs while this phone shares: where people connect, and the way to the share screen.
class _SharingStrip extends StatelessWidget {
  const _SharingStrip({required this.share});
  final ShareController share;
  @override
  Widget build(BuildContext context) {
    final t = Theme.of(context);
    return Material(
      color: t.colorScheme.primaryContainer,
      child: InkWell(
        onTap: () => Navigator.of(context).push(MaterialPageRoute<void>(builder: (_) => SharePage(share: share))),
        child: Padding(
          padding: const EdgeInsets.symmetric(horizontal: 16, vertical: 8),
          child: Row(children: [
            Icon(Icons.wifi_tethering, size: 18, color: t.colorScheme.primary),
            const SizedBox(width: 10),
            Expanded(
              child: Text(
                share.url == null ? 'Sharing nearby' : 'Sharing nearby · ${share.url}',
                style: t.textTheme.labelLarge?.copyWith(color: t.colorScheme.primary),
                overflow: TextOverflow.ellipsis,
              ),
            ),
            Icon(Icons.chevron_right, size: 18, color: t.colorScheme.primary),
          ]),
        ),
      ),
    );
  }
}

class _Message extends StatelessWidget {
  const _Message({required this.icon, required this.title, required this.text, required this.action, this.secondary, this.firstAid = false});
  final IconData icon;
  final String title, text;
  final (String, VoidCallback) action;
  final (String, VoidCallback)? secondary;
  /// The built-in first aid cards, for the Library tab when the library can't show its own First aid page.
  final bool firstAid;

  @override
  Widget build(BuildContext context) {
    final t = Theme.of(context);
    return Center(
      child: Padding(
        padding: const EdgeInsets.all(32),
        child: Column(mainAxisSize: MainAxisSize.min, children: [
          Container(
            width: 72,
            height: 72,
            decoration: BoxDecoration(color: t.colorScheme.primaryContainer, borderRadius: BorderRadius.circular(22)),
            child: Icon(icon, size: 36, color: t.colorScheme.primary),
          ),
          const SizedBox(height: 18),
          Text(title, style: t.textTheme.titleLarge, textAlign: TextAlign.center),
          const SizedBox(height: 8),
          Text(text, textAlign: TextAlign.center, style: t.textTheme.bodyMedium?.copyWith(color: t.colorScheme.onSurfaceVariant)),
          const SizedBox(height: 20),
          FilledButton(onPressed: action.$2, child: Text(action.$1)),
          if (secondary != null) ...[
            const SizedBox(height: 8),
            TextButton(onPressed: secondary!.$2, child: Text(secondary!.$1)),
          ],
          if (firstAid) ...[const SizedBox(height: 28), const FirstAidButton()],
        ]),
      ),
    );
  }
}

/// A WebView onto the embedded server. Links leaving 127.0.0.1 are blocked (offline app).
class WebTab extends StatefulWidget {
  const WebTab({super.key, required this.url});
  final String url;
  @override
  State<WebTab> createState() => _WebTabState();
}

class _WebTabState extends State<WebTab> {
  late final WebViewController c;

  @override
  void initState() {
    super.initState();
    c = WebViewController()
      ..setJavaScriptMode(JavaScriptMode.unrestricted)
      ..setNavigationDelegate(NavigationDelegate(
        onNavigationRequest: (r) {
          final u = Uri.tryParse(r.url);
          if (u != null && u.host == '127.0.0.1') return NavigationDecision.navigate;
          if (u != null && u.scheme == 'tel') {
            dialEmergency(context, u.path); // First aid page's Call links
            return NavigationDecision.prevent;
          }
          ScaffoldMessenger.of(context).showSnackBar(
              SnackBar(content: Text('Offline: external link not opened\n${r.url}')));
          return NavigationDecision.prevent;
        },
      ))
      ..loadRequest(Uri.parse(widget.url));
    final a = c.platform;
    if (a is AndroidWebViewController) {
      // the map's locate button: GPS works offline; Android asks the person first
      a.setGeolocationPermissionsPromptCallbacks(
        onShowPrompt: (_) async {
          bool ok;
          try {
            ok = await const MethodChannel('survivalpedia/files').invokeMethod<bool>('askPermission', {'kind': 'location'}) ?? false;
          } catch (_) {
            ok = false;
          }
          return GeolocationPermissionsResponse(allow: ok, retain: false);
        },
      );
    }
  }

  @override
  Widget build(BuildContext context) {
    return PopScope(
      canPop: false,
      onPopInvokedWithResult: (didPop, _) async {
        if (!didPop && await c.canGoBack()) await c.goBack();
      },
      child: WebViewWidget(controller: c),
    );
  }
}

class ContentPage extends StatefulWidget {
  const ContentPage({super.key, required this.store, required this.downloads, this.share});
  final ContentStore store;
  final Downloads downloads;
  final ShareController? share;
  @override
  State<ContentPage> createState() => _ContentPageState();
}

class _ContentPageState extends State<ContentPage> {
  final hubCtl = TextEditingController();
  Catalog? catalog;
  final Set<String> want = {};
  String? error;

  Downloads get dl => widget.downloads;
  bool get busy => dl.active;

  @override
  void initState() {
    super.initState();
    want.addAll(widget.store.installed.keys);
    want.addAll(dl.queue.map((c) => c.id));
    dl.addListener(_changed);
    SharedPreferences.getInstance().then((p) {
      hubCtl.text = p.getString('hub') ?? defaultHub;
      _load();
    });
  }

  @override
  void dispose() {
    dl.removeListener(_changed);
    super.dispose();
  }

  void _changed() {
    if (mounted) setState(() {});
  }

  Uri get hub => Uri.parse(hubCtl.text.trim());

  Future<void> _load() async {
    setState(() {
      error = null;
      catalog = null;
    });
    if (hubCtl.text.trim().isEmpty) {
      setState(() => error = 'No hub yet. Tap the search button to find a Bothy hub or a phone sharing nearby, '
          'or type a hub address.');
      return;
    }
    try {
      final c = await ContentStore.fetchCatalog(hub);
      (await SharedPreferences.getInstance()).setString('hub', hubCtl.text.trim());
      setState(() => catalog = c);
    } catch (e) {
      setState(() => error = 'Could not reach the hub: $e');
    }
  }

  // a phone sharing nearby (or a hub) on this Wi-Fi; the Books tab uses the same address
  Future<void> _findNearby() async {
    final u = await pickNearby(context);
    if (u == null || !mounted) return;
    hubCtl.text = u.toString();
    await _load();
  }

  void toggle(Component c, bool on) {
    final byId = {for (final x in catalog!.components) x.id: x};
    setState(() {
      if (on) {
        want.add(c.id);
        want.addAll(c.requires); // a semantic index needs its collection and the model
      } else {
        // drop it and whatever depends on it (images and indexes of a collection)
        final drop = {c.id};
        var grew = true;
        while (grew) {
          final more = want.where((id) => !drop.contains(id) && (byId[id]?.requires.any(drop.contains) ?? false)).toList();
          drop.addAll(more);
          grew = more.isNotEmpty;
        }
        want.removeAll(drop);
      }
    });
  }

  Future<void> apply() async {
    setState(() => error = null);
    await dl.apply(hub, catalog!, {...want});
  }

  Future<void> cancelAll() async {
    await dl.cancel();
    setState(() {
      want
        ..clear()
        ..addAll(widget.store.installed.keys);
    });
  }

  static bool _isRegion(Component c) => c.id.startsWith('map:') && c.id != 'map:world';

  String _sub(Component c) => [
        formatBytes(c.size),
        if (widget.store.installed.containsKey(c.id)) 'on this phone' else if (dl.queue.any((q) => q.id == c.id)) 'downloading',
      ].join(' · ');

  Widget _check(Component c, {String? title, String? subtitle, bool enabled = true}) => CheckboxListTile(
        value: want.contains(c.id),
        onChanged: busy || !enabled ? null : (v) => toggle(c, v ?? false),
        title: Text(title ?? c.title),
        subtitle: Text(subtitle ?? _sub(c)),
        dense: true,
        controlAffinity: ListTileControlAffinity.trailing,
      );

  Widget _collection(Group g, List<Component> comps) {
    final t = Theme.of(context);
    final main = comps.where((c) => c.id == g.id).firstOrNull;
    final images = comps.where((c) => c.id == '${g.id}:images').firstOrNull;
    final vecs = comps.where((c) => c.id.startsWith('${g.id}:vec:')).toList();
    final on = main != null && want.contains(main.id);
    final icon = switch (g.kind) {
      'books' => Icons.auto_stories_outlined,
      'guides' => Icons.health_and_safety_outlined,
      _ => Icons.public,
    };
    return Padding(
      padding: const EdgeInsets.fromLTRB(16, 0, 16, 12),
      child: Card(
        child: Column(crossAxisAlignment: CrossAxisAlignment.stretch, children: [
          if (main != null)
            SwitchListTile(
              value: on,
              onChanged: busy ? null : (v) => toggle(main, v),
              secondary: Container(
                width: 44,
                height: 44,
                decoration: BoxDecoration(color: t.colorScheme.primaryContainer, borderRadius: BorderRadius.circular(12)),
                child: Icon(icon, color: t.colorScheme.primary),
              ),
              title: Text(g.title, style: t.textTheme.titleMedium),
              subtitle: Text([
                if (g.description.isNotEmpty) g.description,
                '${formatCount(g.docs)} ${g.kind == 'books' ? 'chapters' : 'articles'} · ${_sub(main)}',
              ].join('\n')),
              isThreeLine: g.description.isNotEmpty,
            ),
          if (on && (images != null || vecs.isNotEmpty)) ...[
            const Divider(indent: 16, endIndent: 16),
            if (images != null) _check(images, title: 'Images'),
            for (final v in vecs) _check(v, subtitle: '${_sub(v)} · finds articles by meaning, not just words'),
          ],
        ]),
      ),
    );
  }

  Widget _gameGrid(List<Component> games) {
    final t = Theme.of(context);
    return Padding(
      padding: const EdgeInsets.symmetric(horizontal: 16),
      child: GridView.count(
        crossAxisCount: 3,
        shrinkWrap: true,
        physics: const NeverScrollableScrollPhysics(),
        mainAxisSpacing: 10,
        crossAxisSpacing: 10,
        childAspectRatio: 0.78,
        children: [
          for (final c in games)
            Builder(builder: (context) {
              final bundled = bundledGames.contains(c.id.substring('game:'.length));
              final on = bundled || want.contains(c.id);
              return Card(
                child: InkWell(
                  onTap: busy || bundled ? null : () => toggle(c, !on),
                  child: Column(crossAxisAlignment: CrossAxisAlignment.stretch, children: [
                    AspectRatio(
                      aspectRatio: 4 / 3,
                      child: Stack(fit: StackFit.expand, children: [
                        c.cover == null
                            ? Container(color: t.colorScheme.primaryContainer)
                            : Image.network(hubUrl(hub, 'download/${c.cover}').toString(), fit: BoxFit.cover,
                                errorBuilder: (_, _, _) => Container(color: t.colorScheme.primaryContainer)),
                        Positioned(
                          right: 6,
                          top: 6,
                          child: Icon(on ? Icons.check_circle : Icons.add_circle_outline,
                              color: on ? t.colorScheme.primary : Colors.white, shadows: const [Shadow(blurRadius: 4)]),
                        ),
                      ]),
                    ),
                    Padding(
                      padding: const EdgeInsets.fromLTRB(8, 6, 8, 6),
                      child: Column(crossAxisAlignment: CrossAxisAlignment.start, children: [
                        Text(c.title, maxLines: 1, overflow: TextOverflow.ellipsis, style: t.textTheme.titleSmall),
                        Text(bundled ? 'Included' : formatBytes(c.size),
                            style: t.textTheme.bodySmall?.copyWith(color: t.colorScheme.onSurfaceVariant)),
                      ]),
                    ),
                  ]),
                ),
              );
            }),
        ],
      ),
    );
  }

  @override
  Widget build(BuildContext context) {
    final t = Theme.of(context);
    final s = widget.store;
    final cat = catalog;
    final comps = cat?.components ?? const <Component>[];
    final pending = comps.where((c) => want.contains(c.id) && !s.installed.containsKey(c.id)).fold<int>(0, (a, c) => a + c.size);
    final removing = s.installed.keys.where((id) => !want.contains(id)).length;
    final model = comps.where((c) => c.id == 'model').firstOrNull;
    final llm = comps.where((c) => c.id == 'llm').firstOrNull;
    final maps = comps.where((c) => c.group == 'maps').toList();
    final games = comps.where((c) => c.group == 'games').toList();
    return Column(children: [
      Padding(
        padding: const EdgeInsets.fromLTRB(20, 16, 20, 0),
        child: Row(children: [
          Expanded(child: Text('Content', style: t.textTheme.headlineMedium)),
          if (widget.share != null)
            FilledButton.tonalIcon(
              onPressed: () => Navigator.of(context).push(MaterialPageRoute<void>(builder: (_) => SharePage(share: widget.share!))),
              icon: const Icon(Icons.wifi_tethering),
              label: Text(widget.share!.on ? 'Sharing' : 'Share nearby'),
            ),
        ]),
      ),
      Padding(
        padding: const EdgeInsets.fromLTRB(20, 4, 20, 8),
        child: Text('Everything is optional. Pick what you want on this phone.',
            style: t.textTheme.bodyMedium?.copyWith(color: t.colorScheme.onSurfaceVariant)),
      ),
      Padding(
        padding: const EdgeInsets.fromLTRB(16, 4, 16, 4),
        child: Row(children: [
          Expanded(
            child: TextField(
              controller: hubCtl,
              decoration: const InputDecoration(hintText: 'Hub address', prefixIcon: Icon(Icons.wifi)),
              keyboardType: TextInputType.url,
              enabled: !busy,
              onSubmitted: (_) => _load(),
            ),
          ),
          const SizedBox(width: 8),
          IconButton.filledTonal(onPressed: busy ? null : _load, icon: const Icon(Icons.refresh)),
          const SizedBox(width: 4),
          IconButton.filledTonal(
            tooltip: 'Find phones sharing nearby, and hubs',
            onPressed: busy ? null : _findNearby,
            icon: const Icon(Icons.wifi_find),
          ),
        ]),
      ),
      if (error != null)
        Padding(padding: const EdgeInsets.all(12), child: Text(error!, style: TextStyle(color: t.colorScheme.error))),
      if (dl.error != null)
        Padding(padding: const EdgeInsets.fromLTRB(20, 4, 20, 4), child: Text(dl.error!, style: TextStyle(color: t.colorScheme.error))),
      if (dl.active) DownloadCard(downloads: dl, onCancel: cancelAll),
      Expanded(
        child: cat == null
            ? (error == null ? const Center(child: CircularProgressIndicator()) : const SizedBox())
            : ListView(children: [
                if (cat.phone)
                  Padding(
                    padding: const EdgeInsets.fromLTRB(20, 4, 20, 8),
                    child: Text(
                      [
                        comps.isEmpty ? 'This phone nearby is sharing only books.' : 'From a phone sharing nearby: what it has.',
                        if (cat.books) 'Its books are in the Books tab → Get books.',
                      ].join(' '),
                      style: t.textTheme.bodyMedium?.copyWith(color: t.colorScheme.onSurfaceVariant),
                    ),
                  ),
                if (cat.groups.isNotEmpty) const SectionLabel('Library'),
                for (final g in cat.groups) _collection(g, comps.where((c) => c.group == g.id).toList()),
                if (model != null)
                  Padding(
                    padding: const EdgeInsets.fromLTRB(16, 0, 16, 12),
                    child: Card(
                      child: _check(model,
                          title: 'Semantic search model',
                          subtitle: '${_sub(model)} · shared by every semantic index; added with the first one'),
                    ),
                  ),
                if (llm != null) ...[
                  const SectionLabel('AI answers'),
                  Padding(
                    padding: const EdgeInsets.fromLTRB(16, 0, 16, 0),
                    child: Card(child: _check(llm, title: 'Offline AI answers', subtitle: '${_sub(llm)} · writes answers from the library; needs ~3 GB RAM')),
                  ),
                ],
                if (maps.isNotEmpty) ...[
                  const SectionLabel('Maps'),
                  Padding(
                    padding: const EdgeInsets.symmetric(horizontal: 16),
                    child: Card(
                      child: Column(children: [
                        // the renderer, the world overview, and the regions chosen or on the phone; the rest in the picker
                        for (final m in maps.where((m) => !_isRegion(m) || want.contains(m.id) || s.installed.containsKey(m.id)))
                          _check(m, title: m.id == 'map-assets' ? 'Map renderer and style' : m.mapName),
                        if (maps.any(_isRegion))
                          ListTile(
                            leading: const Icon(Icons.travel_explore),
                            title: const Text('Choose map regions'),
                            subtitle: Text('${maps.where(_isRegion).length} countries and regions, cut along borders'),
                            trailing: const Icon(Icons.chevron_right),
                            enabled: !busy,
                            onTap: () => Navigator.of(context).push(MaterialPageRoute<void>(
                              builder: (_) => MapPicker(
                                regions: maps.where(_isRegion).toList(),
                                want: want,
                                installed: s.installed.containsKey,
                                onToggle: toggle,
                              ),
                            )),
                          ),
                      ]),
                    ),
                  ),
                ],
                if (games.isNotEmpty) ...[
                  const SectionLabel('Games'),
                  _gameGrid(games),
                ],
                const SizedBox(height: 24),
              ]),
      ),
      Padding(
        padding: const EdgeInsets.fromLTRB(16, 8, 16, 12),
        child: SizedBox(
          width: double.infinity,
          child: busy
              ? const SizedBox.shrink()
              : FilledButton.icon(
                  onPressed: cat == null || (pending == 0 && removing == 0) ? null : apply,
                  icon: const Icon(Icons.download),
                  label: Text(pending > 0
                      ? 'Download ${formatBytes(pending)}${removing > 0 ? ' · remove $removing' : ''}'
                      : removing > 0 ? 'Remove $removing' : 'Up to date'),
                ),
        ),
      ),
    ]);
  }
}
