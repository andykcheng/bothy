// Books tab: public-domain ebooks (Project Gutenberg EPUBs) downloaded one by one or by category from a
// hub's shelf (/api/books, built by ingest/books/epub_shelf.py) and read in the built-in reader
// (assets/reader/reader.html, epub.js) or in another app. Books are not full-text searched; the shelf
// is searched by title and author.
//
// A book is a content component `book:<id>` (group "books") with its EPUB and cover under
// <content>/books/, downloaded through the same background queue as everything else (downloads.dart).
// Reader settings, the place in each book and epub.js's page locations are kept in <content>/books/.

import 'dart:async';
import 'dart:convert';
import 'dart:io';
import 'dart:isolate';
import 'dart:math';

import 'package:flutter/material.dart';
import 'package:flutter/services.dart';
import 'package:http/http.dart' as http;
import 'package:shared_preferences/shared_preferences.dart';
import 'package:webview_flutter/webview_flutter.dart';

import 'book_import.dart';
import 'content.dart';
import 'download_card.dart';
import 'downloads.dart';
import 'theme.dart';

/// The hub address before the user picks one: set at build time (`--dart-define=SP_DEFAULT_HUB=http://…`, from the
/// CI variable of the same name), else the public download server (online; a hub nearby works offline).
const _builtHub = String.fromEnvironment('SP_DEFAULT_HUB');
const defaultHub = _builtHub == '' ? 'https://data.pocketbothy.com/v1' : _builtHub;

Future<Uri> savedHub() async => Uri.parse((await SharedPreferences.getInstance()).getString('hub') ?? defaultHub);

class ShelfBook {
  ShelfBook(Map<String, dynamic> j)
    : id = j['id'] as int,
      title = j['t'] as String,
      author = j['a'] as String? ?? '',
      cats = [for (final c in j['c'] as List) c as int],
      rank = j['r'] as int,
      size = j['s'] as int,
      illustratedSize = j['si'] as int?,
      coverSize = j['cs'] as int?;
  final int id, rank, size;
  final String title, author;
  final List<int> cats;
  final int? illustratedSize, coverSize;

  String get key => 'book:$id';
  String get coverPath => 'books/covers/$id.jpg';

  Component component({bool illustrated = false}) {
    final ill = illustrated && illustratedSize != null;
    final epub = {
      'path': ill ? 'books/epub-images/$id.epub' : 'books/epub/$id.epub',
      'size': ill ? illustratedSize : size,
      'blake3': '',
    };
    final files = [
      epub,
      if (coverSize != null) {'path': coverPath, 'size': coverSize, 'blake3': ''},
    ];
    return Component({
      'id': key,
      'title': title,
      'group': 'books',
      'description': author,
      'cover': coverSize == null ? null : coverPath,
      'size': files.fold<int>(0, (a, f) => a + (f['size'] as int)),
      'files': files,
    });
  }
}

class ShelfGroup {
  ShelfGroup(this.name, this.categories);
  final String name;
  final List<int> categories;
}

class Shelf {
  Shelf(this.categories, this.counts, this.groups, this.books) : byId = {for (final b in books) b.id: b};
  final List<String> categories;
  final List<int> counts;
  final List<ShelfGroup> groups;
  final List<ShelfBook> books; // most-read first
  final Map<int, ShelfBook> byId;

  static Future<Shelf> fetch(Uri hub) async {
    final r = await http.get(hubUrl(hub, 'api/books')).timeout(const Duration(seconds: 60));
    if (r.statusCode == 404) throw const HttpException('This hub has no books.');
    if (r.statusCode != 200) throw HttpException('HTTP ${r.statusCode} from ${hubUrl(hub, 'api/books')}');
    final bytes = r.bodyBytes;
    return Isolate.run(() {
      final j = jsonDecode(utf8.decode(bytes)) as Map<String, dynamic>;
      final cats = [for (final c in j['categories'] as List) c as Map<String, dynamic>];
      return Shelf(
        [for (final c in cats) c['name'] as String],
        [for (final c in cats) c['count'] as int],
        [
          for (final g in j['groups'] as List)
            ShelfGroup((g as Map)['name'] as String, [for (final c in g['categories'] as List) c as int]),
        ],
        [for (final b in j['books'] as List) ShelfBook(b as Map<String, dynamic>)],
      );
    });
  }

  /// The hub's search over titles, authors, subjects and descriptions (best first).
  Future<List<ShelfBook>> searchHub(Uri hub, String q, {int limit = 100}) async {
    final r = await http
        .get(hubUrl(hub, 'api/books/search').replace(queryParameters: {'q': q, 'limit': '$limit'}))
        .timeout(const Duration(seconds: 10));
    if (r.statusCode != 200) throw HttpException('HTTP ${r.statusCode}');
    final j = jsonDecode(utf8.decode(r.bodyBytes)) as Map<String, dynamic>;
    return [for (final h in j['results'] as List) ?byId[(h as Map)['id'] as int]];
  }

  /// Gutenberg's description and subjects of a book, from the hub.
  static Future<(String, List<String>)> details(Uri hub, int id) async {
    final r = await http.get(hubUrl(hub, 'api/books/$id')).timeout(const Duration(seconds: 10));
    if (r.statusCode != 200) return ('', <String>[]);
    final j = jsonDecode(utf8.decode(r.bodyBytes)) as Map<String, dynamic>;
    return (j['summary'] as String? ?? '', [for (final x in j['subjects'] as List? ?? []) x as String]);
  }

  List<ShelfBook> inCategory(int c) => [
    for (final b in books)
      if (b.cats.contains(c)) b,
  ];

  /// Titles and authors containing every word of `q` (case-insensitive), most-read first. Used when the
  /// hub's search can't be reached.
  List<ShelfBook> search(String q, {int limit = 200}) {
    final words = q.toLowerCase().split(RegExp(r'\s+')).where((w) => w.isNotEmpty).toList();
    if (words.isEmpty) return [];
    final out = <ShelfBook>[];
    for (final b in books) {
      final hay = '${b.title} ${b.author}'.toLowerCase();
      if (words.every(hay.contains)) {
        out.add(b);
        if (out.length >= limit) break;
      }
    }
    return out;
  }
}

/// The installed books (newest reading first) and their files.
class Library {
  Library(this.store);
  final ContentStore store;
  String get dir => '${store.root.path}/books';

  List<Component> get books => [
    for (final c in store.installed.values)
      if (c.group == 'books') c,
  ];
  bool has(int id) => store.installed.containsKey('book:$id');
  String epubOf(Component c) => '${store.root.path}/${c.files.first.path}';
  String? coverOf(Component c) => c.cover == null ? null : '${store.root.path}/${c.cover}';
  /// The reader's id of a book: a Gutenberg number, or `imp-<key>` for imported ones.
  String idOf(Component c) => c.id.substring(5);

  File _state(String id) => File('$dir/state/$id.json');

  /// Share read (0..1) and when it was last opened, from the reader's saved state.
  (double?, DateTime?) progress(String id) {
    final f = _state(id);
    if (!f.existsSync()) return (null, null);
    try {
      final j = jsonDecode(f.readAsStringSync()) as Map<String, dynamic>;
      return ((j['pct'] as num?)?.toDouble(), f.lastModifiedSync());
    } catch (_) {
      return (null, f.lastModifiedSync());
    }
  }

  Future<void> remove(Component c) async {
    await store.remove(c.id);
    for (final f in [_state(idOf(c)), File('$dir/locations/${idOf(c)}.json')]) {
      if (await f.exists()) await f.delete();
    }
  }
}

/// Serves the reader page, the book and the reader's saved state to the WebView on 127.0.0.1.
class BookServer {
  BookServer._(this._server, this._token, this.lib);
  final HttpServer _server;
  final String _token;
  final Library lib;
  static BookServer? _instance;

  static Future<BookServer> get(Library lib) async {
    if (_instance != null) return _instance!;
    final s = await HttpServer.bind(InternetAddress.loopbackIPv4, 0);
    final r = Random.secure();
    final token = List.generate(16, (_) => r.nextInt(16).toRadixString(16)).join();
    final b = BookServer._(s, token, lib);
    s.listen(b._handle);
    return _instance = b;
  }

  String url(Component c) =>
      'http://127.0.0.1:${_server.port}/$_token/reader.html?book=${lib.idOf(c)}&title=${Uri.encodeQueryComponent(c.title)}';

  static final _safe = RegExp(r'^[A-Za-z0-9-]+$');

  static const _assets = {
    'reader.html': ('assets/reader/reader.html', 'text/html; charset=utf-8'),
    'epub.min.js': ('assets/reader/epub.min.js', 'application/javascript'),
    'jszip.min.js': ('assets/reader/jszip.min.js', 'application/javascript'),
    'Inter-Regular.ttf': ('assets/fonts/Inter-Regular.ttf', 'font/ttf'),
  };

  Future<void> _handle(HttpRequest req) async {
    final res = req.response;
    try {
      final seg = req.uri.pathSegments;
      if (seg.length < 2 || seg[0] != _token) {
        res.statusCode = 404;
        return;
      }
      final name = seg[1];
      final arg = seg.length > 2 ? seg[2] : '';
      if (_assets.containsKey(name)) {
        final (path, type) = _assets[name]!;
        final data = await rootBundle.load(path);
        res.headers.contentType = ContentType.parse(type);
        res.add(data.buffer.asUint8List(data.offsetInBytes, data.lengthInBytes));
      } else if (name == 'book') {
        final id = arg.replaceAll('.epub', '');
        final c = _safe.hasMatch(id) ? lib.store.installed['book:$id'] : null;
        final f = c == null ? null : File(lib.epubOf(c));
        if (f == null || !await f.exists()) {
          res.statusCode = 404;
          return;
        }
        res.headers.contentType = ContentType('application', 'epub+zip');
        res.headers.contentLength = await f.length();
        await res.addStream(f.openRead());
      } else if (name == 'settings' || name == 'state' || name == 'locations') {
        if (name != 'settings' && !_safe.hasMatch(arg)) {
          res.statusCode = 404;
          return;
        }
        final file = File(name == 'settings' ? '${lib.dir}/settings.json' : '${lib.dir}/$name/$arg.json');
        if (req.method == 'PUT') {
          final body = await utf8.decodeStream(req);
          await file.parent.create(recursive: true);
          await file.writeAsString(body);
        } else if (await file.exists()) {
          res.headers.contentType = ContentType.json;
          res.write(await file.readAsString());
        } else {
          res.statusCode = 404;
        }
      } else {
        res.statusCode = 404;
      }
    } catch (e) {
      res.statusCode = 500;
    } finally {
      await res.close();
    }
  }
}

const _files = MethodChannel('survivalpedia/files');

/// Hands the EPUB to another app (Android's "Open with").
Future<void> openWith(BuildContext context, Library lib, Component c) async {
  try {
    await _files.invokeMethod('openWith', {'path': lib.epubOf(c), 'name': '${c.title}.epub'});
  } on PlatformException catch (e) {
    if (context.mounted) {
      ScaffoldMessenger.of(context).showSnackBar(SnackBar(content: Text(e.message ?? 'No app can open EPUB books')));
    }
  } on MissingPluginException {
    if (context.mounted) {
      ScaffoldMessenger.of(context).showSnackBar(const SnackBar(content: Text('Not available on this device')));
    }
  }
}

Future<void> openReader(BuildContext context, Library lib, Component c) async {
  final server = await BookServer.get(lib);
  if (!context.mounted) return;
  await Navigator.of(context).push(
    MaterialPageRoute<void>(
      builder: (_) => ReaderPage(url: server.url(c), lib: lib, book: c),
    ),
  );
}

class ReaderPage extends StatefulWidget {
  const ReaderPage({super.key, required this.url, required this.lib, required this.book});
  final String url;
  final Library lib;
  final Component book;
  @override
  State<ReaderPage> createState() => _ReaderPageState();
}

class _ReaderPageState extends State<ReaderPage> {
  late final WebViewController c;
  Color bg = const Color(0xFFFBFAF7);
  static const themes = {'light': Color(0xFFFBFAF7), 'sepia': Color(0xFFF4ECD8), 'dark': Color(0xFF151A1E)};

  @override
  void initState() {
    super.initState();
    c = WebViewController()
      ..setJavaScriptMode(JavaScriptMode.unrestricted)
      ..setBackgroundColor(bg)
      ..addJavaScriptChannel(
        'Reader',
        onMessageReceived: (m) {
          final msg = m.message;
          if (msg == 'close') {
            if (mounted) Navigator.of(context).maybePop();
          } else if (msg == 'openWith') {
            openWith(context, widget.lib, widget.book);
          } else if (msg.startsWith('theme:')) {
            final col = themes[msg.substring(6)];
            if (col != null && mounted) setState(() => bg = col);
          }
        },
      )
      ..setNavigationDelegate(
        NavigationDelegate(
          // links inside a book (Gutenberg licence, footnotes to the web) stay offline
          onNavigationRequest: (r) =>
              Uri.tryParse(r.url)?.host == '127.0.0.1' ? NavigationDecision.navigate : NavigationDecision.prevent,
        ),
      )
      ..loadRequest(Uri.parse(widget.url));
  }

  @override
  Widget build(BuildContext context) {
    final dark = bg.computeLuminance() < 0.3;
    return AnnotatedRegion<SystemUiOverlayStyle>(
      value: dark ? SystemUiOverlayStyle.light : SystemUiOverlayStyle.dark,
      child: Scaffold(
        backgroundColor: bg,
        body: SafeArea(child: WebViewWidget(controller: c)),
      ),
    );
  }
}

/// The Books tab: books on this phone, and the way to get more.
class BooksPage extends StatefulWidget {
  const BooksPage({super.key, required this.store, required this.downloads});
  final ContentStore store;
  final Downloads downloads;
  @override
  State<BooksPage> createState() => _BooksPageState();
}

class _BooksPageState extends State<BooksPage> {
  late final Library lib = Library(widget.store);

  @override
  void initState() {
    super.initState();
    widget.downloads.addListener(_changed);
  }

  @override
  void dispose() {
    widget.downloads.removeListener(_changed);
    super.dispose();
  }

  void _changed() {
    if (mounted) setState(() {});
  }

  Future<void> _getBooks() async {
    await Navigator.of(context).push(
      MaterialPageRoute<void>(
        builder: (_) => BookStorePage(lib: lib, downloads: widget.downloads),
      ),
    );
    if (mounted) setState(() {});
  }

  bool _importing = false;

  void _importMenu() {
    showModalBottomSheet<void>(
      context: context,
      showDragHandle: true,
      builder: (ctx) => SafeArea(
        child: Column(
          mainAxisSize: MainAxisSize.min,
          children: [
            ListTile(
              title: Text('Import your own books', style: Theme.of(ctx).textTheme.titleMedium),
              subtitle: const Text('EPUB books from this phone or its SD card. They are copied into the app and stay on this phone.'),
            ),
            ListTile(
              leading: const Icon(Icons.folder_open_outlined),
              title: const Text('Import a folder'),
              subtitle: const Text('Every EPUB in it and its subfolders'),
              onTap: () {
                Navigator.pop(ctx);
                _import(folder: true);
              },
            ),
            ListTile(
              leading: const Icon(Icons.file_open_outlined),
              title: const Text('Import EPUB files'),
              onTap: () {
                Navigator.pop(ctx);
                _import(folder: false);
              },
            ),
          ],
        ),
      ),
    );
  }

  Future<void> _import({required bool folder}) async {
    final messenger = ScaffoldMessenger.of(context);
    setState(() => _importing = true);
    try {
      final r = await importBooks(widget.store, folder: folder);
      if (r == null) return;
      final parts = [
        'Added ${r.added} book${r.added == 1 ? '' : 's'}',
        if (r.skipped > 0) '${r.skipped} already here',
        if (r.failed > 0) '${r.failed} could not be read',
      ];
      messenger.showSnackBar(SnackBar(content: Text(r.added + r.skipped + r.failed == 0 ? 'No EPUB books found there' : parts.join(' · '))));
    } on PlatformException catch (e) {
      messenger.showSnackBar(SnackBar(content: Text('Import failed: ${e.message ?? e.code}')));
    } finally {
      if (mounted) setState(() => _importing = false);
    }
  }

  void _menu(Component c) {
    showModalBottomSheet<void>(
      context: context,
      showDragHandle: true,
      builder: (ctx) => SafeArea(
        child: Column(
          mainAxisSize: MainAxisSize.min,
          children: [
            ListTile(
              title: Text(c.title, style: Theme.of(ctx).textTheme.titleMedium),
              subtitle: Text(c.description),
            ),
            ListTile(
              leading: const Icon(Icons.menu_book_outlined),
              title: const Text('Read'),
              onTap: () {
                Navigator.pop(ctx);
                openReader(context, lib, c).then((_) => _changed());
              },
            ),
            ListTile(
              leading: const Icon(Icons.open_in_new),
              title: const Text('Open in another app'),
              onTap: () {
                Navigator.pop(ctx);
                openWith(context, lib, c);
              },
            ),
            ListTile(
              leading: const Icon(Icons.delete_outline),
              title: Text('Remove from this phone · ${formatBytes(c.size)}'),
              onTap: () async {
                Navigator.pop(ctx);
                await lib.remove(c);
                _changed();
              },
            ),
          ],
        ),
      ),
    );
  }

  @override
  Widget build(BuildContext context) {
    final t = Theme.of(context);
    final books = lib.books;
    final prog = {for (final b in books) b.id: lib.progress(lib.idOf(b))};
    books.sort((a, b) {
      final ta = prog[a.id]!.$2, tb = prog[b.id]!.$2;
      if (ta != null || tb != null) return (tb ?? DateTime(0)).compareTo(ta ?? DateTime(0));
      return a.title.compareTo(b.title);
    });
    final downloadingBooks = widget.downloads.queue.where((c) => c.group == 'books').length;
    return CustomScrollView(
      slivers: [
        SliverToBoxAdapter(
          child: Padding(
            padding: const EdgeInsets.fromLTRB(20, 16, 12, 0),
            child: Row(
              children: [
                Expanded(child: Text('Books', style: t.textTheme.headlineMedium)),
                if (Platform.isAndroid)
                  IconButton(
                    tooltip: 'Import your own books',
                    onPressed: _importing ? null : _importMenu,
                    icon: const Icon(Icons.drive_folder_upload_outlined),
                  ),
                FilledButton.tonalIcon(
                  onPressed: _getBooks,
                  icon: const Icon(Icons.add),
                  label: const Text('Get books'),
                ),
              ],
            ),
          ),
        ),
        SliverToBoxAdapter(
          child: Padding(
            padding: const EdgeInsets.fromLTRB(20, 4, 20, 8),
            child: Text(
              books.isEmpty
                  ? 'Public-domain classics and old how-to manuals from Project Gutenberg. Download single books or whole categories, then read them here.'
                  : '${books.length} book${books.length == 1 ? '' : 's'} on this phone${downloadingBooks > 0 ? ' · $downloadingBooks downloading' : ''}',
              style: t.textTheme.bodyMedium?.copyWith(color: t.colorScheme.onSurfaceVariant),
            ),
          ),
        ),
        if (widget.downloads.active) SliverToBoxAdapter(child: DownloadCard(downloads: widget.downloads)),
        if (_importing)
          const SliverToBoxAdapter(
            child: Padding(
              padding: EdgeInsets.fromLTRB(20, 4, 20, 8),
              child: Row(
                children: [
                  SizedBox(width: 18, height: 18, child: CircularProgressIndicator(strokeWidth: 2)),
                  SizedBox(width: 12),
                  Text('Importing books…'),
                ],
              ),
            ),
          ),
        if (books.isEmpty)
          SliverFillRemaining(
            hasScrollBody: false,
            child: Center(
              child: Padding(
                padding: const EdgeInsets.all(32),
                child: Column(
                  mainAxisSize: MainAxisSize.min,
                  children: [
                    Icon(Icons.auto_stories_outlined, size: 56, color: t.colorScheme.primary),
                    const SizedBox(height: 12),
                    Text('No books yet', style: t.textTheme.titleLarge),
                    const SizedBox(height: 16),
                    FilledButton(onPressed: _getBooks, child: const Text('Browse the shelf')),
                    if (Platform.isAndroid) ...[
                      const SizedBox(height: 8),
                      TextButton(onPressed: _importing ? null : _importMenu, child: const Text('Import your own EPUBs')),
                    ],
                  ],
                ),
              ),
            ),
          )
        else
          SliverPadding(
            padding: const EdgeInsets.fromLTRB(16, 8, 16, 24),
            sliver: SliverGrid.count(
              crossAxisCount: 3,
              mainAxisSpacing: 14,
              crossAxisSpacing: 12,
              childAspectRatio: 0.5,
              children: [
                for (final b in books)
                  _BookTile(
                    title: b.title,
                    author: b.description,
                    cover: lib.coverOf(b) == null ? null : FileImage(File(lib.coverOf(b)!)),
                    progress: prog[b.id]!.$1,
                    onTap: () => openReader(context, lib, b).then((_) => _changed()),
                    onLongPress: () => _menu(b),
                  ),
              ],
            ),
          ),
      ],
    );
  }
}

class _BookTile extends StatelessWidget {
  const _BookTile({required this.title, required this.author, this.cover, this.progress, this.onTap, this.onLongPress});
  final String title, author;
  final ImageProvider? cover;
  final double? progress;
  final VoidCallback? onTap, onLongPress;

  @override
  Widget build(BuildContext context) {
    final t = Theme.of(context);
    return InkWell(
      onTap: onTap,
      onLongPress: onLongPress,
      borderRadius: BorderRadius.circular(10),
      child: Column(
        crossAxisAlignment: CrossAxisAlignment.stretch,
        children: [
          Expanded(
            child: ClipRRect(
              borderRadius: BorderRadius.circular(8),
              child: Stack(
                fit: StackFit.expand,
                children: [
                  cover == null
                      ? _PlainCover(title: title, author: author)
                      : Image(
                          image: cover!,
                          fit: BoxFit.cover,
                          errorBuilder: (_, _, _) => _PlainCover(title: title, author: author),
                        ),
                  if (progress != null && progress! > 0)
                    Positioned(
                      left: 0,
                      right: 0,
                      bottom: 0,
                      child: LinearProgressIndicator(value: progress, minHeight: 4, backgroundColor: Colors.black26),
                    ),
                ],
              ),
            ),
          ),
          const SizedBox(height: 6),
          SizedBox(
            height: 36,
            child: Text(
              title,
              maxLines: 2,
              overflow: TextOverflow.ellipsis,
              style: t.textTheme.titleSmall?.copyWith(height: 1.2),
            ),
          ),
          Text(
            author,
            maxLines: 1,
            overflow: TextOverflow.ellipsis,
            style: t.textTheme.bodySmall?.copyWith(color: t.colorScheme.onSurfaceVariant),
          ),
        ],
      ),
    );
  }
}

/// A cover drawn from the title, for books without a cover image.
class _PlainCover extends StatelessWidget {
  const _PlainCover({required this.title, required this.author});
  final String title, author;
  @override
  Widget build(BuildContext context) {
    final t = Theme.of(context);
    return Container(
      color: t.colorScheme.primaryContainer,
      padding: const EdgeInsets.all(8),
      child: Column(
        mainAxisAlignment: MainAxisAlignment.center,
        children: [
          Text(
            title,
            textAlign: TextAlign.center,
            maxLines: 5,
            overflow: TextOverflow.ellipsis,
            style: t.textTheme.titleSmall?.copyWith(color: t.colorScheme.onPrimaryContainer),
          ),
          const SizedBox(height: 6),
          Text(
            author,
            textAlign: TextAlign.center,
            maxLines: 2,
            overflow: TextOverflow.ellipsis,
            style: t.textTheme.bodySmall?.copyWith(color: t.colorScheme.onPrimaryContainer),
          ),
        ],
      ),
    );
  }
}

/// Browsing the hub's shelf: groups, categories, title/author search.
class BookStorePage extends StatefulWidget {
  const BookStorePage({super.key, required this.lib, required this.downloads});
  final Library lib;
  final Downloads downloads;
  @override
  State<BookStorePage> createState() => _BookStorePageState();
}

class _BookStorePageState extends State<BookStorePage> {
  Shelf? shelf;
  Uri? hub;
  String? error;
  String query = '';
  List<ShelfBook>? found; // the hub's results for `query`
  Timer? _debounce;
  int _seq = 0;

  @override
  void initState() {
    super.initState();
    widget.downloads.addListener(_changed);
    _load();
  }

  @override
  void dispose() {
    widget.downloads.removeListener(_changed);
    _debounce?.cancel();
    super.dispose();
  }

  void _changed() {
    if (mounted) setState(() {});
  }

  void _search(String v) {
    setState(() {
      query = v;
      found = null;
    });
    _debounce?.cancel();
    if (v.trim().length < 2) return;
    final seq = ++_seq;
    _debounce = Timer(const Duration(milliseconds: 250), () async {
      List<ShelfBook>? r;
      try {
        r = await shelf!.searchHub(hub!, v.trim());
      } catch (_) {
        r = shelf!.search(v); // offline from the hub: titles and authors only
      }
      if (mounted && seq == _seq) setState(() => found = r);
    });
  }

  Future<void> _load() async {
    setState(() => error = null);
    try {
      final h = await savedHub();
      final s = await Shelf.fetch(h);
      if (mounted) {
        setState(() {
          hub = h;
          shelf = s;
        });
      }
    } catch (e) {
      if (mounted) setState(() => error = 'Could not load the shelf: $e\nThe hub address is set in Content.');
    }
  }

  @override
  Widget build(BuildContext context) {
    final t = Theme.of(context);
    final s = shelf;
    return Scaffold(
      appBar: AppBar(title: const Text('Get books')),
      body: error != null
          ? Center(
              child: Padding(
                padding: const EdgeInsets.all(24),
                child: Text(error!, textAlign: TextAlign.center),
              ),
            )
          : s == null
          ? const Center(child: CircularProgressIndicator())
          : ListView(
              children: [
                Padding(
                  padding: const EdgeInsets.fromLTRB(16, 4, 16, 4),
                  child: TextField(
                    decoration: const InputDecoration(
                      hintText: 'Search titles, authors, topics',
                      prefixIcon: Icon(Icons.search),
                    ),
                    textInputAction: TextInputAction.search,
                    onChanged: _search,
                  ),
                ),
                if (widget.downloads.active) DownloadCard(downloads: widget.downloads),
                if (query.trim().length >= 2)
                  ...(found == null
                      ? [
                          const Padding(
                            padding: EdgeInsets.all(24),
                            child: Center(child: CircularProgressIndicator()),
                          ),
                        ]
                      : _results(found!))
                else ...[
                  Padding(
                    padding: const EdgeInsets.fromLTRB(20, 8, 20, 0),
                    child: Text(
                      s.groups.isEmpty
                          ? '${formatCount(s.books.length)} public-domain books on this device'
                          : '${formatCount(s.books.length)} public-domain books from Project Gutenberg',
                      style: t.textTheme.bodySmall?.copyWith(color: t.colorScheme.onSurfaceVariant),
                    ),
                  ),
                  // a phone nearby (Share nearby) offers just its own books, without categories: list them all
                  if (s.groups.isEmpty) ..._results(s.books),
                  for (final g in s.groups) ...[
                    SectionLabel(g.name),
                    Padding(
                      padding: const EdgeInsets.symmetric(horizontal: 16),
                      child: Card(
                        child: Column(
                          children: [
                            for (final c in g.categories)
                              ListTile(
                                title: Text(s.categories[c]),
                                trailing: Text(
                                  formatCount(s.counts[c]),
                                  style: t.textTheme.bodySmall?.copyWith(color: t.colorScheme.onSurfaceVariant),
                                ),
                                onTap: () => Navigator.of(context).push(
                                  MaterialPageRoute<void>(
                                    builder: (_) => CategoryPage(
                                      shelf: s,
                                      category: c,
                                      hub: hub!,
                                      lib: widget.lib,
                                      downloads: widget.downloads,
                                    ),
                                  ),
                                ),
                              ),
                          ],
                        ),
                      ),
                    ),
                  ],
                  const SizedBox(height: 24),
                ],
              ],
            ),
    );
  }

  List<Widget> _results(List<ShelfBook> found) => [
    if (found.isEmpty)
      const Padding(
        padding: EdgeInsets.all(24),
        child: Center(child: Text('No books found')),
      ),
    for (final b in found) BookRow(book: b, hub: hub!, lib: widget.lib, downloads: widget.downloads),
  ];
}

/// One category: most-read first, with "download the top N" choices.
class CategoryPage extends StatefulWidget {
  const CategoryPage({
    super.key,
    required this.shelf,
    required this.category,
    required this.hub,
    required this.lib,
    required this.downloads,
  });
  final Shelf shelf;
  final int category;
  final Uri hub;
  final Library lib;
  final Downloads downloads;
  @override
  State<CategoryPage> createState() => _CategoryPageState();
}

class _CategoryPageState extends State<CategoryPage> {
  late final List<ShelfBook> books = widget.shelf.inCategory(widget.category);
  int shown = 100;

  @override
  void initState() {
    super.initState();
    widget.downloads.addListener(_changed);
  }

  @override
  void dispose() {
    widget.downloads.removeListener(_changed);
    super.dispose();
  }

  void _changed() {
    if (mounted) setState(() {});
  }

  Future<void> _downloadTop(int n) async {
    final pick = books.take(n).where((b) => !widget.lib.has(b.id)).map((b) => b.component()).toList();
    if (pick.isEmpty) return;
    await widget.downloads.enqueue(widget.hub, pick);
  }

  @override
  Widget build(BuildContext context) {
    final t = Theme.of(context);
    final name = widget.shelf.categories[widget.category];
    final options = <int>{if (books.length > 50) 50, if (books.length > 200) 200, books.length}.toList()..sort();
    final queued = {for (final c in widget.downloads.queue) c.id};
    return Scaffold(
      appBar: AppBar(title: Text(name)),
      body: ListView.builder(
        itemCount: 2 + min<int>(shown, books.length) + (books.length > shown ? 1 : 0),
        itemBuilder: (context, i) {
          if (i == 0) {
            return Padding(
              padding: const EdgeInsets.fromLTRB(16, 4, 16, 8),
              child: Card(
                child: Padding(
                  padding: const EdgeInsets.all(14),
                  child: Column(
                    crossAxisAlignment: CrossAxisAlignment.start,
                    children: [
                      Text('Download this category', style: t.textTheme.titleSmall),
                      const SizedBox(height: 4),
                      Text(
                        'Most-read books first. Text editions; illustrated ones can be picked per book.',
                        style: t.textTheme.bodySmall?.copyWith(color: t.colorScheme.onSurfaceVariant),
                      ),
                      const SizedBox(height: 10),
                      Wrap(
                        spacing: 8,
                        runSpacing: 8,
                        children: [
                          for (final n in options)
                            Builder(
                              builder: (context) {
                                final top = books.take(n);
                                final missing = top
                                    .where((b) => !widget.lib.has(b.id) && !queued.contains(b.key))
                                    .toList();
                                final size = missing.fold<int>(0, (a, b) => a + b.size + (b.coverSize ?? 0));
                                final label = n == books.length ? 'All ${formatCount(n)}' : 'Top $n';
                                return OutlinedButton(
                                  onPressed: missing.isEmpty ? null : () => _downloadTop(n),
                                  child: Text(missing.isEmpty ? '$label ✓' : '$label · ${formatBytes(size)}'),
                                );
                              },
                            ),
                        ],
                      ),
                    ],
                  ),
                ),
              ),
            );
          }
          if (i == 1) {
            return widget.downloads.active ? DownloadCard(downloads: widget.downloads) : const SizedBox.shrink();
          }
          final k = i - 2;
          if (k >= min<int>(shown, books.length)) {
            return Padding(
              padding: const EdgeInsets.all(16),
              child: OutlinedButton(onPressed: () => setState(() => shown += 200), child: const Text('Show more')),
            );
          }
          return BookRow(
            book: books[k],
            hub: widget.hub,
            lib: widget.lib,
            downloads: widget.downloads,
            rankInList: k + 1,
          );
        },
      ),
    );
  }
}

/// A book in the shelf: cover, title, author, size, and its state on this phone.
class BookRow extends StatelessWidget {
  const BookRow({
    super.key,
    required this.book,
    required this.hub,
    required this.lib,
    required this.downloads,
    this.rankInList,
  });
  final ShelfBook book;
  final Uri hub;
  final Library lib;
  final Downloads downloads;
  final int? rankInList;

  @override
  Widget build(BuildContext context) {
    final t = Theme.of(context);
    final installed = lib.store.installed[book.key];
    final queued = downloads.queue.any((c) => c.id == book.key);
    Widget trailing;
    if (installed != null) {
      trailing = IconButton(
        icon: const Icon(Icons.menu_book),
        tooltip: 'Read',
        onPressed: () => openReader(context, lib, installed),
      );
    } else if (queued) {
      trailing = const Padding(
        padding: EdgeInsets.all(12),
        child: SizedBox(width: 20, height: 20, child: CircularProgressIndicator(strokeWidth: 2)),
      );
    } else {
      trailing = IconButton(
        icon: const Icon(Icons.download_outlined),
        tooltip: 'Download',
        onPressed: () => downloads.enqueue(hub, [book.component()]),
      );
    }
    return ListTile(
      leading: SizedBox(
        width: 40,
        height: 60,
        child: ClipRRect(
          borderRadius: BorderRadius.circular(4),
          child: book.coverSize == null
              ? Container(
                  color: t.colorScheme.primaryContainer,
                  child: Icon(Icons.menu_book, color: t.colorScheme.primary, size: 20),
                )
              : Image.network(
                  hubUrl(hub, 'download/${book.coverPath}').toString(),
                  fit: BoxFit.cover,
                  cacheWidth: 120,
                  errorBuilder: (_, _, _) => Container(color: t.colorScheme.primaryContainer),
                ),
        ),
      ),
      title: Text(book.title, maxLines: 2, overflow: TextOverflow.ellipsis),
      subtitle: Text(
        [book.author, formatBytes(book.size)].where((s) => s.isNotEmpty).join(' · '),
        maxLines: 1,
        overflow: TextOverflow.ellipsis,
      ),
      trailing: trailing,
      onTap: () => _details(context),
    );
  }

  void _details(BuildContext context) {
    final installed = lib.store.installed[book.key];
    final about = Shelf.details(hub, book.id);
    showModalBottomSheet<void>(
      context: context,
      showDragHandle: true,
      isScrollControlled: true,
      builder: (ctx) {
        final t = Theme.of(ctx);
        return SafeArea(
          child: ConstrainedBox(
            constraints: BoxConstraints(maxHeight: MediaQuery.of(ctx).size.height * 0.8),
            child: SingleChildScrollView(
              padding: const EdgeInsets.fromLTRB(20, 0, 20, 16),
              child: Column(
                mainAxisSize: MainAxisSize.min,
                crossAxisAlignment: CrossAxisAlignment.stretch,
                children: [
                  Text(book.title, style: t.textTheme.titleLarge),
                  if (book.author.isNotEmpty) Text(book.author, style: t.textTheme.bodyMedium),
                  const SizedBox(height: 4),
                  Text(
                    'Project Gutenberg #${book.id} · public domain',
                    style: t.textTheme.bodySmall?.copyWith(color: t.colorScheme.onSurfaceVariant),
                  ),
                  FutureBuilder<(String, List<String>)>(
                    future: about,
                    builder: (_, snap) {
                      final d = snap.data;
                      if (d == null || (d.$1.isEmpty && d.$2.isEmpty)) return const SizedBox(height: 8);
                      return Padding(
                        padding: const EdgeInsets.only(top: 12),
                        child: Column(
                          crossAxisAlignment: CrossAxisAlignment.start,
                          children: [
                            if (d.$1.isNotEmpty) Text(d.$1, style: t.textTheme.bodyMedium?.copyWith(height: 1.45)),
                            if (d.$2.isNotEmpty) ...[
                              const SizedBox(height: 8),
                              Text(
                                d.$2.toSet().join(' · '),
                                style: t.textTheme.bodySmall?.copyWith(color: t.colorScheme.onSurfaceVariant),
                              ),
                            ],
                          ],
                        ),
                      );
                    },
                  ),
                  const SizedBox(height: 16),
                  if (installed != null) ...[
                    FilledButton.icon(
                      onPressed: () {
                        Navigator.pop(ctx);
                        openReader(context, lib, installed);
                      },
                      icon: const Icon(Icons.menu_book),
                      label: const Text('Read'),
                    ),
                    const SizedBox(height: 8),
                    OutlinedButton(
                      onPressed: () {
                        Navigator.pop(ctx);
                        openWith(context, lib, installed);
                      },
                      child: const Text('Open in another app'),
                    ),
                  ] else ...[
                    FilledButton.icon(
                      onPressed: () {
                        Navigator.pop(ctx);
                        downloads.enqueue(hub, [book.component()]);
                      },
                      icon: const Icon(Icons.download),
                      label: Text('Download · ${formatBytes(book.size)}'),
                    ),
                    if (book.illustratedSize != null) ...[
                      const SizedBox(height: 8),
                      OutlinedButton(
                        onPressed: () {
                          Navigator.pop(ctx);
                          downloads.enqueue(hub, [book.component(illustrated: true)]);
                        },
                        child: Text('Illustrated edition · ${formatBytes(book.illustratedSize!)}'),
                      ),
                    ],
                  ],
                ],
              ),
            ),
          ),
        );
      },
    );
  }
}
