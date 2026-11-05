// Downloads against a live hub: pause part-way, "restart the app", resume, and check the result.
//   SP_HUB=http://127.0.0.1:8082 LD_LIBRARY_PATH=../core/target/release flutter test test/downloads_test.dart
// (libsp_ffi.so from `cargo build --release -p sp-ffi` provides BLAKE3.) Skipped without SP_HUB.

import 'dart:async';
import 'dart:io';

import 'package:flutter_test/flutter_test.dart';
import 'package:survivalpedia/content.dart';
import 'package:survivalpedia/downloads.dart';

void main() {
  final hub = Platform.environment['SP_HUB'];

  test('download, pause, resume after restart, install', () async {
    final tmp = await Directory.systemTemp.createTemp('sp-dl');
    addTearDown(() => tmp.delete(recursive: true));
    final cat = await ContentStore.fetchCatalog(Uri.parse(hub!));
    final want = {'col:guides', 'col:guides:images'};
    final size = cat.components.where((c) => want.contains(c.id)).fold<int>(0, (a, c) => a + c.size);

    // first run: pause once a third is on disk
    var store = await ContentStore.at(tmp);
    var installs = 0;
    final paused = Completer<void>();
    late Downloads dl;
    dl = Downloads(store, onInstalled: () async => installs++, fetcher: _PauseAt(size ~/ 3, () {
      if (!paused.isCompleted) {
        paused.complete();
        dl.pause();
      }
    }));
    await dl.apply(Uri.parse(hub), cat, want);
    expect(paused.isCompleted, isTrue);
    expect(dl.paused, isTrue);
    expect(dl.queue, isNotEmpty);
    final parts = tmp.listSync(recursive: true).whereType<File>().where((f) => f.path.endsWith('.part')).toList();
    final onDisk = parts.fold<int>(0, (a, f) => a + f.lengthSync());
    expect(onDisk, greaterThan(0));
    // whatever counts as installed is complete; the rest is still queued
    expect(dl.queue, isNotEmpty);
    for (final c in store.installed.values) {
      for (final f in c.files) {
        expect(File('${tmp.path}/${f.path}').lengthSync(), f.size, reason: f.path);
      }
    }

    // "restart": a new store and queue from disk; it stays paused until asked
    store = await ContentStore.at(tmp);
    dl = Downloads(store, onInstalled: () async => installs++, fetcher: DartFetcher());
    await dl.load();
    expect(dl.paused, isTrue);
    expect(dl.running, isFalse);
    var maxSeen = 0, minSeen = size;
    dl.addListener(() {
      if (dl.running && dl.note == null) {
        if (dl.done > maxSeen) maxSeen = dl.done;
        if (dl.done < minSeen && dl.done > 0) minSeen = dl.done;
      }
    });
    await dl.resume();
    expect(dl.error, isNull);
    expect(dl.queue, isEmpty);
    expect(store.installed.keys.toSet(), want);
    expect(store.collectionPaths, ['${tmp.path}/collections/guides']);
    expect(installs, greaterThanOrEqualTo(1));
    // resumed: the first progress already counted the bytes from the first run
    expect(minSeen, greaterThanOrEqualTo(onDisk ~/ 2));
    final left = tmp.listSync(recursive: true).whereType<File>().where((f) => f.path.endsWith('.part'));
    expect(left, isEmpty);
    for (final c in cat.components.where((c) => want.contains(c.id))) {
      for (final f in c.files) {
        expect(File('${tmp.path}/${f.path}').lengthSync(), f.size, reason: f.path);
      }
    }
  }, skip: hub == null ? 'set SP_HUB' : false, timeout: const Timeout(Duration(minutes: 5)));

  test('a corrupt part is fetched again', () async {
    final tmp = await Directory.systemTemp.createTemp('sp-dl');
    addTearDown(() => tmp.delete(recursive: true));
    final cat = await ContentStore.fetchCatalog(Uri.parse(hub!));
    final c = cat.components.firstWhere((c) => c.id == 'col:guides');
    final f = c.files.firstWhere((f) => f.size > 1000);
    final part = File('${tmp.path}/${f.path}.part');
    await part.parent.create(recursive: true);
    await part.writeAsBytes(List.filled(f.size, 7)); // right size, wrong bytes
    final store = await ContentStore.at(tmp);
    final dl = Downloads(store, onInstalled: () async {}, fetcher: DartFetcher());
    await dl.apply(Uri.parse(hub), cat, {'col:guides'});
    expect(dl.error, isNull);
    expect(store.installed.keys, ['col:guides']);
  }, skip: hub == null ? 'set SP_HUB' : false, timeout: const Timeout(Duration(minutes: 2)));
}

/// DartFetcher that calls `hit` once the bytes on disk pass `at` (pausing mid-file, deterministically).
class _PauseAt extends DartFetcher {
  _PauseAt(this.at, this.hit);
  final int at;
  final void Function() hit;

  @override
  Future<FetchEnd> fetch(String title, List<FetchJob> jobs, void Function(int bytes, String? note) onProgress,
      void Function(int index) onFile) =>
      super.fetch(title, jobs, (b, n) {
        onProgress(b, n);
        if (b > at) hit();
      }, onFile);
}
