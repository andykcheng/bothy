// The ebook shelf against a live hub with --books: index, title search, and downloading books through
// the background queue.
//   SP_HUB=http://127.0.0.1:8082 LD_LIBRARY_PATH=../core/target/release flutter test test/books_test.dart

import 'dart:io';

import 'package:flutter_test/flutter_test.dart';
import 'package:survivalpedia/books.dart';
import 'package:survivalpedia/content.dart';
import 'package:survivalpedia/downloads.dart';

void main() {
  final hub = Platform.environment['SP_HUB'];

  test('shelf, search, download two books', () async {
    final shelf = await Shelf.fetch(Uri.parse(hub!));
    expect(shelf.books, isNotEmpty);
    expect(shelf.groups.map((g) => g.name), contains('Fiction'));
    final pride = shelf.search('pride prejudice austen');
    expect(pride.first.id, 1342);
    // the hub's search also reads descriptions: Dracula is "about" a vampire
    final vamp = await shelf.searchHub(Uri.parse(hub), 'vampire');
    expect(vamp.first.title, 'Dracula');
    final (about, subjects) = await Shelf.details(Uri.parse(hub), 1342);
    expect(about, contains('Elizabeth Bennet'));
    expect(subjects, isNotEmpty);
    final novels = shelf.inCategory(shelf.categories.indexOf('Novels'));
    expect(novels.first.rank, lessThan(novels.last.rank));

    final tmp = await Directory.systemTemp.createTemp('sp-books');
    addTearDown(() => tmp.delete(recursive: true));
    final store = await ContentStore.at(tmp);
    var restarts = 0;
    final dl = Downloads(store, onInstalled: () async => restarts++, fetcher: DartFetcher());
    final two = [pride.first.component(), shelf.books[1].component()];
    await dl.enqueue(Uri.parse(hub), two);
    expect(dl.error, isNull);
    final lib = Library(store);
    expect(lib.books.map((c) => c.id).toSet(), two.map((c) => c.id).toSet());
    for (final c in lib.books) {
      expect(File(lib.epubOf(c)).lengthSync(), c.files.first.size);
      if (c.cover != null) expect(File(lib.coverOf(c)!).existsSync(), isTrue);
    }
    expect(restarts, 0); // books don't restart the core
    expect(store.collectionPaths, isEmpty);

    // the reader's server: page, book, saved state
    final server = await BookServer.get(lib);
    final c = lib.books.firstWhere((c) => c.id == 'book:1342');
    final url = Uri.parse(server.url(c));
    final http = HttpClient();
    Future<(int, int)> get(String rel) async {
      final r = await (await http.getUrl(url.resolve(rel))).close();
      final n = await r.fold<int>(0, (a, b) => a + b.length);
      return (r.statusCode, n);
    }
    expect((await get('book/1342.epub')).$2, c.files.first.size);
    expect((await get('book/999999.epub')).$1, 404);
    final put = await http.putUrl(url.resolve('state/1342'));
    put.write('{"cfi":"x","pct":0.25}');
    await (await put.close()).drain<void>();
    expect(lib.progress('1342').$1, 0.25);
    expect((await get('../wrong-token/book/1342.epub')).$1, 404);
    http.close();
  }, skip: hub == null ? 'set SP_HUB' : false, timeout: const Timeout(Duration(minutes: 3)));
}
