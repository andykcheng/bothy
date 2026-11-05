// Background, resumable downloads of content components.
//
// The queue (components still to install, the hub, verified file hashes) is saved in
// <content>/downloads.json, so a download interrupted by a lost connection, the app being closed or
// the phone restarting carries on from the bytes already on disk the next time the app opens.
//
// Bytes are fetched by a Fetcher. On Android that is a foreground service (DownloadService.kt) with
// a progress notification, so downloads keep going with the screen off or the app in the
// background, and even after the app is swiped away. Elsewhere (tests, desktop) it is plain Dart HTTP.
// The fetcher only writes `<file>.part`. This side checks each finished part against its BLAKE3
// hash, and moves a component's parts into place only when every one of them is verified, so the
// core never sees a half-installed collection.

import 'dart:async';
import 'dart:convert';
import 'dart:io';
import 'dart:math';

import 'package:flutter/foundation.dart';
import 'package:flutter/services.dart';
import 'package:http/http.dart' as http;

import 'content.dart';
import 'core.dart';

/// One file for the fetcher: GET `url` into `part`, resuming from its current length.
class FetchJob {
  FetchJob(this.url, this.part, this.size, this.label);
  final String url, part, label;
  final int size;
  Map<String, dynamic> toJson() => {'url': url, 'part': part, 'size': size, 'label': label};
}

/// How a fetch ended.
sealed class FetchEnd {
  const FetchEnd();
}

class FetchDone extends FetchEnd {
  const FetchDone();
}

class FetchStopped extends FetchEnd {
  const FetchStopped(this.reason);
  final String? reason; // e.g. Android's time limit for background data transfers
}

class FetchFailed extends FetchEnd {
  const FetchFailed(this.message);
  final String message;
}

abstract class Fetcher {
  /// Fetches the jobs in order. Network errors are retried with backoff. Calls `onProgress` with
  /// the bytes on disk across all jobs and an optional note ("retrying in 20 s"), and `onFile` with
  /// a job's index once its part is complete.
  Future<FetchEnd> fetch(String title, List<FetchJob> jobs, void Function(int bytes, String? note) onProgress,
      void Function(int index) onFile);
  Future<void> stop();

  static Fetcher platform(String root) => Platform.isAndroid ? AndroidFetcher(root) : DartFetcher();
}

class Downloads extends ChangeNotifier {
  Downloads(this.store, {required this.onInstalled, Fetcher? fetcher})
      : fetcher = fetcher ?? Fetcher.platform(store.root.path);
  final ContentStore store;
  final Fetcher fetcher;

  /// Called after components were installed or removed (the app restarts the core).
  final Future<void> Function() onInstalled;

  Uri? hub;
  List<Component> queue = [];
  final Map<String, String> _verified = {}; // part path -> blake3 that matched the catalog
  final Map<String, int> _mismatches = {};
  bool paused = false, running = false;
  String? error, note;
  int done = 0, total = 0;
  String? current;
  DateTime _lastNotify = DateTime(0);

  bool get active => running || queue.isNotEmpty;
  File get _state => File('${store.root.path}/downloads.json');

  /// Loads the saved queue and carries on with it unless the user paused it.
  Future<void> load() async {
    if (await _state.exists()) {
      try {
        final j = jsonDecode(await _state.readAsString()) as Map<String, dynamic>;
        hub = j['hub'] == null ? null : Uri.parse(j['hub'] as String);
        queue = [for (final c in j['queue'] as List? ?? []) Component(c as Map<String, dynamic>)];
        paused = j['paused'] as bool? ?? false;
        _verified.addAll({for (final e in (j['verified'] as Map? ?? {}).entries) e.key as String: e.value as String});
      } catch (_) {
        queue = [];
      }
    }
    // Pause pressed in the notification while the app was closed (DownloadService.kt)
    final marker = File('${store.root.path}/downloads.paused');
    if (await marker.exists()) {
      paused = true;
      await marker.delete();
      await _save();
    }
    queue.removeWhere((c) => store.installed.containsKey(c.id));
    total = queue.fold(0, (a, c) => a + c.size);
    notifyListeners();
    if (queue.isNotEmpty && !paused && hub != null) unawaited(resume());
  }

  Future<void> _save() async {
    final tmp = File('${_state.path}.tmp');
    await tmp.writeAsString(jsonEncode({
      'hub': hub?.toString(),
      'paused': paused,
      'queue': [for (final c in queue) c.toJson()],
      'verified': _verified,
    }));
    await tmp.rename(_state.path);
  }

  /// Removes installed components not in `want` and queues those in `want` that are missing, in
  /// catalog order, then starts downloading.
  Future<void> apply(Uri hub, Catalog cat, Set<String> want) async {
    if (running) return;
    var removed = false;
    final inCatalog = {for (final c in cat.components) c.id};
    for (final id in store.installed.keys.toList()) {
      // only what this catalog offers; books (books.dart) are managed in the Books tab
      if (inCatalog.contains(id) && !want.contains(id)) {
        await store.remove(id);
        removed = true;
      }
    }
    this.hub = hub;
    queue = [
      ...queue.where((c) => !inCatalog.contains(c.id)), // e.g. books still queued
      for (final c in cat.components) if (want.contains(c.id) && !store.installed.containsKey(c.id)) c,
    ];
    error = null;
    paused = false;
    await _save();
    if (queue.isEmpty) {
      notifyListeners();
      if (removed) await onInstalled();
      return;
    }
    await resume();
    if (removed && !running) await onInstalled();
  }

  /// Adds components (e.g. books) to the queue without touching anything else, then downloads.
  Future<void> enqueue(Uri hub, Iterable<Component> comps) async {
    final have = {...queue.map((c) => c.id), ...store.installed.keys};
    final add = comps.where((c) => !have.contains(c.id)).toList();
    if (add.isEmpty) return;
    queue = [...queue, ...add];
    if (!running) {
      this.hub = hub;
      error = null;
      paused = false;
    }
    total = queue.fold(0, (a, c) => a + c.size);
    await _save();
    notifyListeners();
    if (!running) await resume();
  }

  /// Takes a not-yet-installed component out of the queue (its partial file stays for later).
  Future<void> dequeue(String id) async {
    queue.removeWhere((c) => c.id == id);
    await _save();
    notifyListeners();
  }

  Future<void> pause() async {
    paused = true;
    await _save();
    await fetcher.stop();
    notifyListeners();
  }

  /// Stops and forgets the queue. Partial files stay on disk, so asking again later resumes them.
  Future<void> cancel() async {
    queue = [];
    paused = false;
    await _save();
    await fetcher.stop();
    notifyListeners();
  }

  void _progress(int bytes, {String? label, String? noteText, bool force = false}) {
    done = bytes;
    if (label != null) current = label;
    note = noteText;
    final now = DateTime.now();
    if (force || now.difference(_lastNotify).inMilliseconds > 250) {
      _lastNotify = now;
      notifyListeners();
    }
  }

  Future<void> resume() async {
    if (running || queue.isEmpty || hub == null) return;
    running = true;
    paused = false;
    error = null;
    note = null;
    await _save();
    notifyListeners();
    var installedAny = false;
    try {
      while (queue.isNotEmpty) {
        total = queue.fold(0, (a, c) => a + c.size);
        // What is already on disk and verified, and what still needs bytes.
        var have = 0;
        final jobs = <FetchJob>[];
        final jobFiles = <CatalogFile>[];
        final jobComps = <Component>[];
        final remaining = <String, int>{}; // component id -> files still being fetched
        final complete = <Component>[];
        for (final c in queue) {
          var all = true;
          for (final f in c.files) {
            final ok = await _ready(f, c.title, have);
            if (ok) {
              have += f.size;
            } else {
              all = false;
              final part = '${store.root.path}/${f.path}.part';
              jobs.add(FetchJob(hub!.resolve('/download/${f.path}').toString(), part, f.size, c.title));
              jobFiles.add(f);
              jobComps.add(c);
              remaining[c.id] = (remaining[c.id] ?? 0) + 1;
            }
          }
          if (all) complete.add(c);
        }
        for (final c in complete) {
          await _finish(c, save: false);
          installedAny |= c.group != 'books'; // books need no core restart
        }
        if (complete.isNotEmpty) await _saveAll();
        if (jobs.isEmpty) continue; // queue shrank; loop ends when it is empty
        final bad = jobFiles.where((f) => (_mismatches[f.path] ?? 0) >= 3).firstOrNull;
        if (bad != null) {
          error = '${bad.path} keeps failing its checksum. The hub copy may be damaged.';
          break;
        }
        final base = have;
        var finished = 0;
        var verifying = Future<void>.value();
        final end = await fetcher.fetch(
          'Bothy content',
          jobs,
          (bytes, n) => _progress(base + bytes, label: _labelAt(jobs, bytes), noteText: n),
          (i) => verifying = verifying.then((_) async {
            // install each component as soon as its last file lands (a category of books fills in as it goes)
            if (!await _verifyPart(jobFiles[i], jobs[i].part)) return;
            final c = jobComps[i];
            final left = (remaining[c.id] ?? 1) - 1;
            remaining[c.id] = left;
            if (left == 0 && c.files.every((f) => f.size == 0 || File('${store.root.path}/${f.path}.part').existsSync() ||
                File('${store.root.path}/${f.path}').existsSync())) {
              await _finish(c, save: ++finished % 25 == 0);
              installedAny |= c.group != 'books';
            }
          }),
        );
        await verifying;
        await _saveAll();
        if (end is FetchStopped) {
          if (end.reason != null) error = end.reason;
          if (!paused && queue.isNotEmpty && end.reason == null) {
            // stopped from the notification
            paused = true;
            await _save();
          }
          break;
        }
        if (end is FetchFailed) {
          error = end.message;
          break;
        }
      }
    } catch (e) {
      error = '$e';
    } finally {
      running = false;
      note = null;
      await _save();
      notifyListeners();
      if (installedAny) await onInstalled();
    }
  }

  String _labelAt(List<FetchJob> jobs, int bytes) {
    var acc = 0;
    for (final j in jobs) {
      acc += j.size;
      if (bytes < acc) return j.label;
    }
    return jobs.isEmpty ? '' : jobs.last.label;
  }

  /// Whether `f` needs no more bytes: its final file is in place, or its complete part is verified.
  /// Bad copies are deleted.
  Future<bool> _ready(CatalogFile f, String label, int have) async {
    final dest = File('${store.root.path}/${f.path}');
    if (f.size == 0) return true;
    if (await dest.exists()) {
      if (await dest.length() == f.size &&
          (f.blake3.isEmpty || store.isVerified(f) || _verified[dest.path] == f.blake3 || await _hash(dest.path, f, label, have) == f.blake3)) {
        _verified[dest.path] = f.blake3;
        return true;
      }
      await dest.delete();
    }
    final part = File('${dest.path}.part');
    if (!await part.exists()) return false;
    final len = await part.length();
    if (len > f.size) {
      await part.delete();
      return false;
    }
    if (len < f.size) return false;
    if (f.blake3.isEmpty || _verified[part.path] == f.blake3) return true;
    return _verifyPart(f, part.path, label: label, have: have);
  }

  Future<String> _hash(String path, CatalogFile f, String label, int have) {
    _progress(have, label: label, noteText: 'Checking ${f.path.split('/').last}', force: true);
    return Core.blake3(path);
  }

  Future<bool> _verifyPart(CatalogFile f, String part, {String? label, int? have}) async {
    if (f.blake3.isEmpty) return true; // no hash published (ebook shelf): the size check is all
    final h = label == null ? await Core.blake3(part) : await _hash(part, f, label, have ?? done);
    if (h == f.blake3) {
      _verified[part] = h;
      return true;
    }
    _mismatches[f.path] = (_mismatches[f.path] ?? 0) + 1;
    _verified.remove(part);
    final p = File(part);
    if (await p.exists()) await p.delete();
    return false;
  }

  Future<void> _saveAll() async {
    await store.saveInstalled();
    await _save();
    notifyListeners();
  }

  /// Moves a component's verified parts into place and records it as installed (saved now, or by the
  /// caller's _saveAll for a batch).
  Future<void> _finish(Component c, {bool save = true}) async {
    for (final f in c.files) {
      final dest = File('${store.root.path}/${f.path}');
      await dest.parent.create(recursive: true);
      if (f.size == 0) {
        if (!await dest.exists()) await dest.writeAsBytes(const []);
        continue;
      }
      final part = File('${dest.path}.part');
      if (await part.exists()) {
        await part.rename(dest.path);
        _verified.remove(part.path);
      }
    }
    store.installed[c.id] = c;
    queue.removeWhere((q) => q.id == c.id);
    for (final f in c.files) {
      _verified.remove('${store.root.path}/${f.path}');
    }
    if (save) await _saveAll();
  }
}

/// Android: the DownloadService foreground service does the transfer.
class AndroidFetcher implements Fetcher {
  AndroidFetcher(this.root);
  final String root;
  static const _method = MethodChannel('survivalpedia/downloads');
  static const _events = EventChannel('survivalpedia/downloads/events');
  StreamSubscription<dynamic>? _sub;

  @override
  Future<FetchEnd> fetch(String title, List<FetchJob> jobs, void Function(int bytes, String? note) onProgress,
      void Function(int index) onFile) async {
    final list = File('$root/fetch-jobs.json');
    await list.writeAsString(jsonEncode({'title': title, 'jobs': [for (final j in jobs) j.toJson()]}));
    final end = Completer<FetchEnd>();
    await _sub?.cancel();
    _sub = _events.receiveBroadcastStream().listen((e) {
      final m = (e as Map).cast<String, dynamic>();
      switch (m['type']) {
        case 'progress':
          onProgress((m['bytes'] as num).toInt(), m['note'] as String?);
        case 'file':
          onFile((m['index'] as num).toInt());
        case 'done':
          if (!end.isCompleted) end.complete(const FetchDone());
        case 'stopped':
          if (!end.isCompleted) end.complete(FetchStopped(m['reason'] as String?));
        case 'error':
          if (!end.isCompleted) end.complete(FetchFailed(m['message'] as String? ?? 'download failed'));
      }
    }, onError: (Object e) {
      if (!end.isCompleted) end.complete(FetchFailed('$e'));
    });
    await _method.invokeMethod('start', {'jobs': list.path});
    final r = await end.future;
    await _sub?.cancel();
    _sub = null;
    return r;
  }

  @override
  Future<void> stop() => _method.invokeMethod('stop');
}

/// Tests and desktop: the same protocol over package:http, in this isolate.
class DartFetcher implements Fetcher {
  bool _stop = false;

  @override
  Future<FetchEnd> fetch(String title, List<FetchJob> jobs, void Function(int bytes, String? note) onProgress,
      void Function(int index) onFile) async {
    _stop = false;
    final client = http.Client();
    var before = 0; // bytes of the jobs before the current one
    var lastProgress = DateTime.now();
    try {
      for (var i = 0; i < jobs.length; i++) {
        final j = jobs[i];
        final part = File(j.part);
        await part.parent.create(recursive: true);
        var delay = 5;
        while (true) {
          if (_stop) return const FetchStopped(null);
          var have = await part.exists() ? await part.length() : 0;
          if (have >= j.size) break;
          try {
            final req = http.Request('GET', Uri.parse(j.url));
            if (have > 0) req.headers['Range'] = 'bytes=$have-';
            final resp = await client.send(req).timeout(const Duration(seconds: 30));
            if (resp.statusCode == 200 && have > 0) {
              have = 0; // the server ignored the range: start over
            } else if (resp.statusCode != 200 && resp.statusCode != 206) {
              return FetchFailed('HTTP ${resp.statusCode} for ${j.url}');
            }
            final sink = part.openWrite(mode: have > 0 ? FileMode.append : FileMode.write);
            try {
              await for (final chunk in resp.stream.timeout(const Duration(seconds: 60))) {
                if (_stop) break;
                sink.add(chunk);
                have += chunk.length;
                lastProgress = DateTime.now();
                delay = 5;
                onProgress(before + have, null);
              }
            } finally {
              await sink.close();
            }
          } on Exception catch (e) {
            if (DateTime.now().difference(lastProgress).inMinutes >= 15) {
              return FetchFailed('Lost the hub: $e');
            }
            onProgress(before + have, 'Connection lost, retrying in $delay s');
            await Future<void>.delayed(Duration(seconds: delay));
            delay = min(delay * 2, 60);
          }
        }
        before += j.size;
        onFile(i);
      }
      return const FetchDone();
    } finally {
      client.close();
    }
  }

  @override
  Future<void> stop() async => _stop = true;
}
