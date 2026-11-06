// Screenshots of the Games and Content pages for design review (not a regression test):
//   SP_HUB=http://127.0.0.1:8082 flutter test test/screens_test.dart --update-goldens
// Writes test/screens/*.png (git-ignored). Uses the bundled Inter font and a live hub for the catalog.

import 'dart:io';

import 'package:flutter/material.dart';
import 'package:flutter/services.dart';
import 'package:flutter_test/flutter_test.dart';
import 'package:shared_preferences/shared_preferences.dart';
import 'package:survivalpedia/books.dart';
import 'package:survivalpedia/content.dart';
import 'package:survivalpedia/downloads.dart';
import 'package:survivalpedia/games.dart';
import 'package:survivalpedia/main.dart';
import 'package:survivalpedia/notes.dart';
import 'package:survivalpedia/share.dart';
import 'package:survivalpedia/theme.dart';

Future<void> _font(String family, List<String> files) async {
  final l = FontLoader(family);
  for (final f in files) {
    l.addFont(Future.value(ByteData.sublistView(File(f).readAsBytesSync())));
  }
  await l.load();
}

void main() {
  final hub = Platform.environment['SP_HUB'] ?? 'http://127.0.0.1:8082';
  late Directory tmp;

  setUpAll(() async {
    await _font('Inter', [for (final w in ['Regular', 'Medium', 'SemiBold', 'Bold']) 'assets/fonts/Inter-$w.ttf']);
    final flutterRoot = Platform.environment['FLUTTER_ROOT'] ?? '${Platform.environment['HOME']}/work-projects/flutter';
    await _font('MaterialIcons', ['$flutterRoot/bin/cache/artifacts/material_fonts/MaterialIcons-Regular.otf']);
    tmp = Directory.systemTemp.createTempSync('sp-screens');
  });

  Future<void> shot(WidgetTester tester, String name, Brightness b, Widget page, {Duration wait = const Duration(seconds: 2)}) async {
    tester.view.physicalSize = const Size(1080, 2280);
    tester.view.devicePixelRatio = 2.75;
    await tester.pumpWidget(MaterialApp(
      debugShowCheckedModeBanner: false,
      theme: buildTheme(b),
      home: Scaffold(
        body: SafeArea(child: page),
        bottomNavigationBar: NavigationBar(
            selectedIndex: name.startsWith('games') ? 4 : name.startsWith('books') ? 1 : name.startsWith('notes') ? 3
                : name.startsWith('frame-library') ? 0 : name.startsWith('frame-map') ? 2 : 5, destinations: const [
          NavigationDestination(icon: Icon(Icons.travel_explore_outlined), label: 'Library'),
          NavigationDestination(icon: Icon(Icons.auto_stories_outlined), selectedIcon: Icon(Icons.auto_stories), label: 'Books'),
          NavigationDestination(icon: Icon(Icons.map_outlined), label: 'Map'),
          NavigationDestination(icon: Icon(Icons.sticky_note_2_outlined), selectedIcon: Icon(Icons.sticky_note_2), label: 'Notes'),
          NavigationDestination(icon: Icon(Icons.sports_esports_outlined), selectedIcon: Icon(Icons.sports_esports), label: 'Games'),
          NavigationDestination(icon: Icon(Icons.download_outlined), selectedIcon: Icon(Icons.download), label: 'Content'),
        ]),
      ),
    ));
    // real I/O (files, the hub over HTTP) needs runAsync
    for (var i = 0; i < 6; i++) {
      await tester.runAsync(() => Future.delayed(wait ~/ 6));
      await tester.pump();
    }
    // decode images loaded from files / network
    for (final e in find.byType(Image).evaluate()) {
      await tester.runAsync(() => precacheImage((e.widget as Image).image, e));
    }
    await tester.pump();
    await expectLater(find.byType(MaterialApp), matchesGoldenFile('screens/$name.png'));
    await tester.pumpWidget(const SizedBox());
    await tester.pump(const Duration(seconds: 30)); // let the HTTP timeout timer expire
  }

  for (final b in Brightness.values) {
    final tag = b == Brightness.dark ? 'dark' : 'light';
    testWidgets('games $tag', (tester) async {
      final store = (await tester.runAsync(() => ContentStore.at(Directory('${tmp.path}/c'))))!;
      final games = (await tester.runAsync(() => Games.open(store)))!;
      final list = (await tester.runAsync(() => games.list()))!;
      await shot(tester, 'games-$tag', b, GamesPage(games: games, onGetMore: () {}, preloaded: list));
    });
    testWidgets('books $tag', (tester) async {
      HttpOverrides.global = null;
      final store = (await tester.runAsync(() => ContentStore.at(Directory('${tmp.path}/b'))))!;
      final dl = Downloads(store, onInstalled: () async {}, fetcher: DartFetcher());
      if (Library(store).books.isEmpty) {
        await tester.runAsync(() async {
          final shelf = await Shelf.fetch(Uri.parse(hub));
          await dl.enqueue(Uri.parse(hub), shelf.books.take(9).map((b) => b.component()));
        });
      }
      await shot(tester, 'books-$tag', b, BooksPage(store: store, downloads: dl));
    });
    testWidgets('notes $tag', (tester) async {
      final dir = Directory('${tmp.path}/n-$tag')..createSync();
      final notes = (await tester.runAsync(() => NotesStore.open(dir)))!;
      await shot(tester, 'notes-empty-$tag', b, NotesPage(store: notes), wait: Duration.zero);
      notes.add(title: 'Grab bag', items: [
        CheckItem('Water and snacks', true), CheckItem('Torch, radio and spare batteries', true), CheckItem('First aid kit'),
        CheckItem('Medicines and a copy of prescriptions'), CheckItem('Cash'), CheckItem('Phone charger and power bank'),
      ]);
      notes.add(title: '', text: 'Water off until Thursday\nCollect from the bowser at the church car park, 8am–6pm. Bring containers.');
      notes.add(title: 'Emergency contacts', text: 'Emergency: 112\nMum: 07700 900123\nDr Patel (GP): 020 7946 0018\nNeighbour Sue, no. 14: 07700 900456')
          .pinned = true;
      await shot(tester, 'notes-$tag', b, NotesPage(store: notes), wait: Duration.zero);
      for (final (name, title) in [('contacts', 'Emergency contacts'), ('checklist', 'Grab bag')]) {
        await shot(tester, 'notes-$name-$tag', b, NoteEditor(store: notes, note: notes.notes.firstWhere((n) => n.title == title)),
            wait: Duration.zero);
      }
    });
    testWidgets('share $tag', (tester) async {
      final share = ShareController(ensureCore: () async {}, localPort: () => 41234);
      await shot(tester, 'share-off-$tag', b, SharePage(share: share), wait: Duration.zero);
      share
        ..on = true
        ..mode = ShareMode.hotspot
        ..ssid = 'AndroidShare_4821'
        ..password = 'k9mp2vl8qt'
        ..port = 8080
        ..addresses = ['192.168.43.1'];
      await shot(tester, 'share-on-$tag', b, SharePage(share: share), wait: Duration.zero);
    });
    // empty frames with the Library / Map tab selected, for the store screenshots (scripts/play_assets.py)
    testWidgets('frames $tag', (tester) async {
      await shot(tester, 'frame-library-$tag', b, const SizedBox.expand(), wait: Duration.zero);
      await shot(tester, 'frame-map-$tag', b, const SizedBox.expand(), wait: Duration.zero);
    });
    testWidgets('content $tag', (tester) async {
      HttpOverrides.global = null; // allow the real hub
      SharedPreferences.setMockInitialValues({'hub': hub});
      final store = (await tester.runAsync(() => ContentStore.at(Directory('${tmp.path}/c'))))!;
      await shot(tester, 'content-$tag', b, ContentPage(store: store, downloads: Downloads(store, onInstalled: () async {}, fetcher: DartFetcher())), wait: const Duration(seconds: 4));
    });
  }
}
