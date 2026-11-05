// Phone to phone (DESIGN §19): phone A downloads from a hub, then serves what it has (sp-server reading
// A's installed.json, as the app's core does while sharing), and phone B downloads all of it from A, every
// file checked against the hashes A recorded.
//   SP_HUB=http://127.0.0.1:8082 SP_SERVER=../core/target/release/sp-server \
//     LD_LIBRARY_PATH=../core/target/release flutter test test/nearby_test.dart
// Skipped without SP_HUB and SP_SERVER.

import 'dart:convert';
import 'dart:io';

import 'package:flutter_test/flutter_test.dart';
import 'package:http/http.dart' as http;
import 'package:survivalpedia/books.dart';
import 'package:survivalpedia/content.dart';
import 'package:survivalpedia/downloads.dart';

void main() {
  final hub = Platform.environment['SP_HUB'];
  final server = Platform.environment['SP_SERVER'];

  test('a phone passes on what it has', () async {
    final a = await Directory.systemTemp.createTemp('sp-phone-a');
    final b = await Directory.systemTemp.createTemp('sp-phone-b');
    addTearDown(() async {
      await a.delete(recursive: true);
      await b.delete(recursive: true);
    });

    // phone A: guides with images, a game and a book, from the hub
    final hubUri = Uri.parse(hub!);
    final cat = await ContentStore.fetchCatalog(hubUri);
    final game = cat.components.firstWhere((c) => c.group == 'games').id;
    final want = {'col:guides', 'col:guides:images', game};
    final storeA = await ContentStore.at(a);
    final dlA = Downloads(storeA, onInstalled: () async {}, fetcher: DartFetcher());
    await dlA.apply(hubUri, cat, want);
    final shelf = await Shelf.fetch(hubUri);
    final book = shelf.byId[84] ?? shelf.books.first;
    await dlA.enqueue(hubUri, [book.component()]);
    expect(dlA.error, isNull);
    expect(storeA.installed.keys.toSet(), {...want, book.key});
    // a book of the person's own: never offered
    final own = File('${a.path}/books/imported/u-x1.epub');
    await own.parent.create(recursive: true);
    await own.writeAsString('private');
    await storeA.markInstalled(Component({
      'id': 'book:u-x1', 'title': 'Diary', 'group': 'books', 'size': 7,
      'files': [{'path': 'books/imported/u-x1.epub', 'size': 7, 'blake3': ''}],
    }));

    // A shares: its server reads installed.json
    const port = 18095;
    final p = await Process.start(server!, [
      ...storeA.collectionPaths,
      '--listen', '127.0.0.1:$port',
      '--installed', '${a.path}/installed.json',
    ]);
    addTearDown(p.kill);
    final aUri = Uri.parse('http://127.0.0.1:$port');
    for (var i = 0; i < 50; i++) {
      try {
        await http.get(aUri.resolve('/api/catalog'));
        break;
      } catch (_) {
        await Future<void>.delayed(const Duration(milliseconds: 100));
      }
    }

    // phone B: everything A offers
    final catA = await ContentStore.fetchCatalog(aUri);
    expect(catA.phone, isTrue);
    expect(catA.books, isTrue);
    expect(catA.components.map((c) => c.id).toSet(), want);
    expect(catA.groups.map((g) => g.id), ['col:guides']);
    final storeB = await ContentStore.at(b);
    final dlB = Downloads(storeB, onInstalled: () async {}, fetcher: DartFetcher());
    await dlB.apply(aUri, catA, want);
    expect(dlB.error, isNull);
    final shelfA = await Shelf.fetch(aUri);
    expect(shelfA.books.map((x) => x.id), [book.id]);
    await dlB.enqueue(aUri, [shelfA.books.single.component()]);
    expect(dlB.error, isNull);
    expect(storeB.installed.keys.toSet(), {...want, book.key});
    for (final c in storeA.installed.values.where((c) => c.id != 'book:u-x1')) {
      for (final f in c.files) {
        final fa = File('${a.path}/${f.path}'), fb = File('${b.path}/${f.path}');
        expect(await fb.readAsBytes(), await fa.readAsBytes(), reason: f.path);
      }
    }
    expect(File('${b.path}/books/imported/u-x1.epub').existsSync(), isFalse);
    expect((await http.get(aUri.resolve('/download/books/imported/u-x1.epub'))).statusCode, 404);
    expect((await http.get(aUri.resolve('/download/installed.json'))).statusCode, 404);
    expect(jsonDecode((await http.get(aUri.resolve('/api/catalog'))).body)['source'], 'phone');
  }, skip: hub == null || server == null ? 'set SP_HUB and SP_SERVER' : false, timeout: const Timeout(Duration(minutes: 5)));
}
