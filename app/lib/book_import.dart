// Importing the user's own EPUBs into the Books tab: a folder (with its subfolders) or picked files.
// On Android the system picker gives access and BookImport (MainActivity.kt) copies every .epub into
// <content>/books/imported/<key>.epub. This side reads each book's title, author and cover from the EPUB
// and puts it on the shelf as a `book:imp-<key>` component with nothing to download.
// Imported books never leave the phone.

import 'dart:convert';
import 'dart:io';
import 'dart:isolate';

import 'package:archive/archive.dart';
import 'package:flutter/services.dart';

import 'content.dart';

class EpubMeta {
  EpubMeta(this.title, this.author, this.cover, this.coverExt);
  final String? title, author;
  final List<int>? cover;
  final String? coverExt;
}

String _unescape(String s) => s
    .replaceAllMapped(RegExp(r'&#x([0-9a-fA-F]+);'), (m) => String.fromCharCode(int.parse(m[1]!, radix: 16)))
    .replaceAllMapped(RegExp(r'&#(\d+);'), (m) => String.fromCharCode(int.parse(m[1]!)))
    .replaceAll('&lt;', '<')
    .replaceAll('&gt;', '>')
    .replaceAll('&quot;', '"')
    .replaceAll('&apos;', "'")
    .replaceAll('&amp;', '&')
    .replaceAll(RegExp(r'<[^>]+>'), '')
    .replaceAll(RegExp(r'\s+'), ' ')
    .trim();

String? _attr(String tag, String name) => RegExp('\\b$name\\s*=\\s*["\']([^"\']*)["\']').firstMatch(tag)?.group(1);

/// Title, author and cover image of an EPUB (from its OPF package file). Missing parts are null.
EpubMeta readEpubMeta(List<int> bytes) {
  final zip = ZipDecoder().decodeBytes(bytes);
  String? text(String path) {
    final f = zip.findFile(path);
    return f == null ? null : utf8.decode(f.content, allowMalformed: true);
  }

  final container = text('META-INF/container.xml');
  final opfPath = container == null ? null : RegExp(r'full-path\s*=\s*"([^"]+)"').firstMatch(container)?.group(1);
  final opf = opfPath == null ? null : text(opfPath);
  if (opf == null) return EpubMeta(null, null, null, null);
  String? dc(String el) {
    final m = RegExp('<dc:$el\\b[^>]*>([\\s\\S]*?)</dc:$el>').firstMatch(opf);
    final v = m == null ? null : _unescape(m.group(1)!);
    return v == null || v.isEmpty ? null : v;
  }

  // cover: <meta name="cover" content="id">, else an item with properties="cover-image", else an
  // image item called cover
  final items = [for (final m in RegExp(r'<item\b[^>]*>').allMatches(opf)) m.group(0)!];
  String? coverHref;
  for (final m in RegExp(r'<meta\b[^>]*>').allMatches(opf)) {
    final t = m.group(0)!;
    if (_attr(t, 'name') == 'cover') {
      final id = _attr(t, 'content');
      coverHref = items.where((i) => _attr(i, 'id') == id).map((i) => _attr(i, 'href')).firstOrNull;
    }
  }
  coverHref ??= items.where((i) => (_attr(i, 'properties') ?? '').contains('cover-image')).map((i) => _attr(i, 'href')).firstOrNull;
  coverHref ??= items
      .where((i) => (_attr(i, 'media-type') ?? '').startsWith('image/') && '${_attr(i, 'id')} ${_attr(i, 'href')}'.toLowerCase().contains('cover'))
      .map((i) => _attr(i, 'href'))
      .firstOrNull;
  List<int>? cover;
  String? ext;
  if (coverHref != null) {
    final base = opfPath!.contains('/') ? opfPath.substring(0, opfPath.lastIndexOf('/') + 1) : '';
    final path = Uri.decodeFull(_normalise('$base$coverHref'));
    final f = zip.findFile(path);
    if (f != null) {
      cover = f.content;
      ext = path.toLowerCase().endsWith('.png') ? 'png' : path.toLowerCase().endsWith('.gif') ? 'gif' : 'jpg';
    }
  }
  return EpubMeta(dc('title'), dc('creator'), cover, ext);
}

String _normalise(String p) {
  final out = <String>[];
  for (final s in p.split('/')) {
    if (s == '..') {
      if (out.isNotEmpty) out.removeLast();
    } else if (s != '.' && s.isNotEmpty) {
      out.add(s);
    }
  }
  return out.join('/');
}

class ImportResult {
  ImportResult(this.added, this.skipped, this.failed);
  final int added, skipped, failed;
}

/// Puts EPUBs already copied to `<content>/books/imported/` on the shelf. Books already there (same file,
/// or same title and author, e.g. the horizontal and vertical editions of one book) are skipped, and
/// their extra copy is deleted.
Future<ImportResult> registerImported(ContentStore store, List<String> paths) async {
  final root = store.root.path;
  final metas = await Isolate.run(() {
    return [
      for (final p in paths)
        () {
          try {
            final m = readEpubMeta(File(p).readAsBytesSync());
            return (p, m.title, m.author, m.cover, m.coverExt, true);
          } catch (_) {
            return (p, null, null, null, null, false);
          }
        }(),
    ];
  });
  final seen = {
    for (final c in store.installed.values)
      if (c.group == 'books') '${c.title.toLowerCase()}|${c.description.toLowerCase()}',
  };
  var added = 0, skipped = 0, failed = 0;
  for (final (path, title, author, cover, ext, ok) in metas) {
    final file = File(path);
    final key = file.uri.pathSegments.last.replaceAll(RegExp(r'\.epub$'), '');
    final id = 'book:imp-$key';
    if (!ok) {
      failed++;
      if (await file.exists()) await file.delete();
      continue;
    }
    final name = title ?? key;
    final who = author ?? '';
    final dup = '${name.toLowerCase()}|${who.toLowerCase()}';
    if (store.installed.containsKey(id) || seen.contains(dup)) {
      skipped++;
      if (!store.installed.containsKey(id) && await file.exists()) await file.delete();
      continue;
    }
    seen.add(dup);
    final rel = file.path.substring(root.length + 1);
    final files = [
      {'path': rel, 'size': await file.length(), 'blake3': ''},
    ];
    String? coverRel;
    if (cover == null) {
      // no cover inside: an image that sat next to the book in its folder (see sideCover)
      for (final e in const ['jpg', 'png']) {
        final side = File('${file.parent.path}/$key.side.$e');
        if (await side.exists()) {
          coverRel = 'books/imported/$key.cover.$e';
          await side.rename('$root/$coverRel');
          files.add({'path': coverRel, 'size': await File('$root/$coverRel').length(), 'blake3': ''});
          break;
        }
      }
    } else {
      coverRel = 'books/imported/$key.cover.$ext';
      await File('$root/$coverRel').writeAsBytes(cover);
      files.add({'path': coverRel, 'size': cover.length, 'blake3': ''});
    }
    store.installed[id] = Component({
      'id': id,
      'title': name,
      'group': 'books',
      'description': who,
      'cover': coverRel,
      'size': files.fold<int>(0, (a, f) => a + (f['size'] as int)),
      'files': files,
    });
    added++;
  }
  await store.saveInstalled();
  return ImportResult(added, skipped, failed);
}

const _files = MethodChannel('survivalpedia/files');

/// Asks Android for a folder (`folder: true`, searched with its subfolders) or for EPUB files, copies
/// the books into the app and puts them on the shelf. Null when the user cancelled the picker.
Future<ImportResult?> importBooks(ContentStore store, {required bool folder}) async {
  final dest = Directory('${store.root.path}/books/imported');
  await dest.create(recursive: true);
  final r = await _files.invokeMethod<List<Object?>>(folder ? 'importFolder' : 'importFiles', {'dest': dest.path});
  if (r == null) return null;
  return registerImported(store, [for (final p in r) p as String]);
}

/// The image to use as a book's cover when the EPUB has none: in the book's folder, an image whose name
/// is part of the book's file name ("429.jpg" for "A429.epub"), else the only image there.
String? sideCover(String epubName, List<String> images) {
  final stem = epubName.toLowerCase().replaceAll(RegExp(r'\.epub$'), '');
  final named = images.where((i) {
    final s = i.toLowerCase().replaceAll(RegExp(r'\.(jpe?g|png)$'), '');
    return s.isNotEmpty && stem.contains(s);
  }).toList()
    ..sort((a, b) => b.length.compareTo(a.length));
  if (named.isNotEmpty) return named.first;
  return images.length == 1 ? images.first : null;
}

/// Desktop and tests: import every .epub under a directory.
Future<ImportResult> importDirectory(ContentStore store, Directory src) async {
  final dest = Directory('${store.root.path}/books/imported');
  await dest.create(recursive: true);
  final paths = <String>[];
  await for (final e in src.list(recursive: true)) {
    if (e is File && e.path.toLowerCase().endsWith('.epub')) {
      final name = e.uri.pathSegments.last;
      final key = _fnv('$name|${await e.length()}');
      final to = File('${dest.path}/$key.epub');
      if (!await to.exists()) {
        await e.copy(to.path);
        final images = [
          for (final f in e.parent.listSync())
            if (f is File && RegExp(r'\.(jpe?g|png)$', caseSensitive: false).hasMatch(f.path)) f.uri.pathSegments.last,
        ];
        final side = sideCover(name, images);
        if (side != null) {
          final ext = side.toLowerCase().endsWith('.png') ? 'png' : 'jpg';
          await File('${e.parent.path}/$side').copy('${dest.path}/$key.side.$ext');
        }
      }
      paths.add(to.path);
    }
  }
  return registerImported(store, paths);
}

String _fnv(String s) {
  var h = 0xcbf29ce484222325;
  for (final b in utf8.encode(s)) {
    h = (h ^ b) * 0x100000001b3;
  }
  return (h & 0x7fffffffffffffff).toRadixString(16).padLeft(16, '0');
}
