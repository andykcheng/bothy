// Content manager: the hub's catalog (/api/catalog), what is installed, and removal. Downloads
// (background, resumable, BLAKE3-verified) are in downloads.dart. Files keep the catalog's relative paths under the content root
// (collections/<id>/…, models/…, maps/…, games/…, llm/…), so whatever subset is installed is valid for
// the core. Nothing is required: a phone may hold only games, only maps, or one collection.

import 'dart:convert';
import 'dart:io';

import 'package:http/http.dart' as http;
import 'package:path_provider/path_provider.dart';


class CatalogFile {
  CatalogFile(Map<String, dynamic> j)
      : path = j['path'] as String,
        size = (j['size'] as num).toInt(),
        blake3 = j['blake3'] as String;
  final String path;
  final int size;
  final String blake3;
  Map<String, dynamic> toJson() => {'path': path, 'size': size, 'blake3': blake3};
}

class Component {
  Component(Map<String, dynamic> j)
      : id = j['id'] as String,
        title = j['title'] as String,
        group = j['group'] as String? ?? 'library',
        description = j['description'] as String? ?? '',
        cover = j['cover'] as String?,
        size = (j['size'] as num).toInt(),
        requires = [for (final r in (j['requires'] as List? ?? [])) r as String],
        name = j['name'] as String?,
        continent = j['continent'] as String?,
        country = j['country'] as String?,
        countryTitle = j['country_title'] as String?,
        files = [for (final f in j['files'] as List) CatalogFile(f as Map<String, dynamic>)];
  final String id, title, group, description;
  final String? cover;
  /// Map regions: short name, continent, and for parts of a big country its id and name
  final String? name, continent, country, countryTitle;
  final int size;
  final List<String> requires;
  final List<CatalogFile> files;

  Map<String, dynamic> toJson() => {
        'id': id, 'title': title, 'group': group, 'description': description, 'cover': cover, 'size': size,
        'requires': requires, 'files': [for (final f in files) f.toJson()],
        if (name != null) 'name': name,
        if (continent != null) 'continent': continent,
        if (country != null) 'country': country,
        if (countryTitle != null) 'country_title': countryTitle,
      };

  /// A map region's name for lists: "California", or "United Kingdom".
  String get mapName => name ?? title.replaceFirst('Map · ', '');
}

/// A collection on the hub (Wikipedia Essentials, wikiHow, survival guides, books…); its components share
/// `group == id` (`col:ID`).
class Group {
  Group(Map<String, dynamic> j)
      : id = j['id'] as String,
        title = j['title'] as String? ?? j['id'] as String,
        description = j['description'] as String? ?? '',
        kind = j['kind'] as String? ?? 'articles',
        docs = (j['docs'] as num?)?.toInt() ?? 0;
  final String id, title, description, kind;
  final int docs;
}

class Catalog {
  Catalog(this.groups, this.components, {this.books = false, this.phone = false});
  final List<Group> groups;
  final List<Component> components;
  /// It has books too (they are in the Books tab), and it is a phone sharing nearby rather than a hub.
  final bool books, phone;
}

/// What is on the device: component id -> the component as it was installed.
class ContentStore {
  ContentStore._(this.root);
  final Directory root;
  Map<String, Component> installed = {};

  static Future<ContentStore> open() async {
    // App-specific external storage when present: big, and reachable over USB for copying packs.
    final base = await getExternalStorageDirectory() ?? await getApplicationSupportDirectory();
    final s = ContentStore._(Directory('${base.path}/content'));
    await s.root.create(recursive: true);
    await s._load();
    return s;
  }

  /// A store at `root` (tests, screenshots).
  static Future<ContentStore> at(Directory root) async {
    final s = ContentStore._(root);
    await s.root.create(recursive: true);
    await s._load();
    return s;
  }

  File get _state => File('${root.path}/installed.json');
  String get collectionsDir => '${root.path}/collections';
  String get mapsDir => '${root.path}/maps';
  String get gamesDir => '${root.path}/games';
  String get llmDir => '${root.path}/llm';

  /// Installed collections (`col:ID` without a sub-component).
  Iterable<String> get collections => installed.keys.where((k) => k.startsWith('col:') && k.indexOf(':', 4) < 0);
  bool get hasLibrary => collections.isNotEmpty;
  bool get hasMaps => installed.keys.any((k) => k.startsWith('map:'));
  bool get hasLlm => installed.containsKey('llm');

  /// Whether the embedded server has anything to serve (library, maps or AI answers).
  bool get needsCore => hasLibrary || hasMaps || hasLlm;

  /// The query embedding model, when installed.
  String? get modelPath {
    final m = installed['model'];
    return m == null || m.files.isEmpty ? null : '${root.path}/${m.files.first.path}';
  }

  Future<void> _load() async {
    if (await _state.exists()) {
      final j = jsonDecode(await _state.readAsString()) as Map<String, dynamic>;
      installed = {for (final e in j.entries) e.key: Component(e.value as Map<String, dynamic>)};
    }
    // Before collections, the library was one pack under pack/ ("text", "images", "model", "vec:…").
    final legacy = installed.keys.where((k) => k == 'text' || k == 'images' || k.startsWith('vec:') ||
        (k == 'model' && installed[k]!.files.any((f) => f.path.startsWith('pack/')))).toList();
    if (legacy.isNotEmpty) {
      legacy.forEach(installed.remove);
      final old = Directory('${root.path}/pack');
      if (await old.exists()) await old.delete(recursive: true);
      await _save();
    }
  }

  Future<void> _save() async {
    final tmp = File('${_state.path}.tmp');
    await tmp.writeAsString(jsonEncode({for (final e in installed.entries) e.key: e.value.toJson()}));
    await tmp.rename(_state.path);
  }

  /// Records `c` as installed (its files are in place and verified; see downloads.dart).
  Future<void> markInstalled(Component c) async {
    installed[c.id] = c;
    await _save();
  }

  /// Saves `installed` after a batch of changes to it.
  Future<void> saveInstalled() => _save();

  /// Whether an installed component already has this exact file (shared files need no new check).
  bool isVerified(CatalogFile f) =>
      installed.values.any((c) => c.files.any((g) => g.path == f.path && g.blake3 == f.blake3));

  /// Directories of the installed collections, for the core. Collections still downloading are left
  /// out, so a half-filled directory is never opened.
  List<String> get collectionPaths => [for (final id in collections) '$collectionsDir/${id.substring(4)}'];

  /// Deletes a component's files that no other installed component uses.
  Future<void> remove(String id) async {
    final c = installed.remove(id);
    if (c == null) return;
    final keep = {for (final o in installed.values) for (final f in o.files) f.path};
    for (final f in c.files) {
      if (keep.contains(f.path)) continue;
      final file = File('${root.path}/${f.path}');
      if (await file.exists()) await file.delete();
    }
    await _save();
  }

  static Future<Catalog> fetchCatalog(Uri hub) async {
    final r = await http.get(hubUrl(hub, 'api/catalog')).timeout(const Duration(seconds: 15));
    if (r.statusCode != 200) throw HttpException('HTTP ${r.statusCode} from ${hubUrl(hub, 'api/catalog')}');
    final j = jsonDecode(utf8.decode(r.bodyBytes)) as Map<String, dynamic>;
    return Catalog(
      [for (final g in j['groups'] as List? ?? []) Group(g as Map<String, dynamic>)],
      [for (final c in j['components'] as List) Component(c as Map<String, dynamic>)],
      books: j['books'] != null,
      phone: j['source'] == 'phone',
    );
  }
}

/// [path] (no leading slash) under the hub address, keeping any base path: a hub at http://host:8080 and a
/// static download server at https://data.pocketbothy.com/v1 both work.
Uri hubUrl(Uri hub, String path) => (hub.path.endsWith('/') ? hub : hub.replace(path: '${hub.path}/')).resolve(path);

String formatBytes(int b) {
  if (b >= 1e9) return '${(b / 1e9).toStringAsFixed(2)} GB';
  if (b >= 1e6) return '${(b / 1e6).toStringAsFixed(1)} MB';
  if (b >= 1e3) return '${(b / 1e3).toStringAsFixed(0)} KB';
  return '$b B';
}

/// 101840 -> "101,840"
String formatCount(int n) => n.toString().replaceAllMapped(RegExp(r'\B(?=(\d{3})+$)'), (_) => ',');
