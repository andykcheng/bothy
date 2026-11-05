// Importing the user's own EPUBs from a folder.
//   SP_IMPORT_DIR=<folder with .epub files, in subfolders too> flutter test test/import_test.dart

import 'dart:io';

import 'package:flutter_test/flutter_test.dart';
import 'package:survivalpedia/book_import.dart';
import 'package:survivalpedia/books.dart';
import 'package:survivalpedia/content.dart';

void main() {
  final src = Platform.environment['SP_IMPORT_DIR'];

  test('import a folder: metadata, covers, duplicates, remove', () async {
    final tmp = await Directory.systemTemp.createTemp('sp-import');
    addTearDown(() => tmp.delete(recursive: true));
    final store = await ContentStore.at(tmp);
    final r = await importDirectory(store, Directory(src!));
    final lib = Library(store);
    for (final b in lib.books) {
      // ignore: avoid_print
      print('${b.id} | ${b.title} | ${b.description} | cover ${b.cover != null}');
    }
    expect(r.added, greaterThan(0));
    expect(sideCover('A429.epub', ['429.jpg']), '429.jpg');
    expect(sideCover('C240a.epub', ['240a.jpg', '240b.jpg', '240d.jpg']), '240a.jpg');
    expect(sideCover('x.epub', ['a.jpg', 'b.jpg']), isNull);
    expect(lib.books.every((b) => b.cover != null), isTrue);
    expect(r.failed, 0);
    for (final b in lib.books) {
      expect(File(lib.epubOf(b)).existsSync(), isTrue);
      expect(b.title, isNotEmpty);
      expect(lib.idOf(b), startsWith('imp-'));
    }
    // a second import of the same folder adds nothing
    final again = await importDirectory(store, Directory(src));
    expect(again.added, 0);
    expect(Library(store).books.length, lib.books.length);
    // the reader's server serves an imported book by its string id
    final server = await BookServer.get(lib);
    final b = lib.books.first;
    final url = Uri.parse(server.url(b));
    final http = HttpClient();
    final res = await (await http.getUrl(url.resolve('book/${lib.idOf(b)}.epub'))).close();
    expect(res.statusCode, 200);
    expect(await res.fold<int>(0, (a, c) => a + c.length), File(lib.epubOf(b)).lengthSync());
    http.close();
    // remove deletes its files
    final path = lib.epubOf(b);
    await lib.remove(b);
    expect(File(path).existsSync(), isFalse);
  }, skip: src == null ? 'set SP_IMPORT_DIR' : false);
}
