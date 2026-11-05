// Search for Notes: keyword search (BM25) that always works, plus search by meaning when the library's
// embedding model is installed. Each note is embedded whole and line by line (a note scores as its best
// part, so "vegetables" finds the garden note's potato line). Vectors come from the embedded server
// (/api/embed), are cached in `notes-emb.json` beside notes.json, and are refreshed in the background, so a
// search never waits for more than the query's own vector. The two rankings are merged by reciprocal rank
// fusion.

import 'dart:convert';
import 'dart:io';
import 'dart:math' as math;
import 'dart:typed_data';

import 'notes.dart';

/// Vectors for texts, from the embedding model: `query` adds the model's query instruction.
typedef Embed = Future<List<List<double>>> Function(List<String> texts, {required bool query});

/// Lower-cased words and numbers (any script).
List<String> noteTokens(String s) =>
    [for (final m in _word.allMatches(s.toLowerCase())) m.group(0)!];
final _word = RegExp(r'[\p{L}\p{N}]+', unicode: true);

/// What the model reads for a note: title and text, checklist items one per line, without tick marks.
String embedText(Note n) => [
      if (n.title.trim().isNotEmpty) n.title.trim(),
      if (!n.isChecklist && n.text.trim().isNotEmpty) n.text.trim(),
      for (final i in n.items ?? <CheckItem>[])
        if (i.text.trim().isNotEmpty) i.text.trim(),
    ].join('\n');

/// Little words that would match nearly every note ("who to call if someone is ill" is about calling and
/// illness). Ignored in queries unless the query has nothing else.
const _stop = {
  'a', 'an', 'and', 'are', 'as', 'at', 'be', 'by', 'can', 'do', 'does', 'for', 'from', 'goes', 'has', 'have', 'how',
  'i', 'if', 'in', 'is', 'it', 'me', 'must', 'my', 'of', 'off', 'on', 'or', 'our', 'should', 'so', 'some', 'that',
  'the', 'their', 'there', 'this', 'to', 'up', 'was', 'we', 'what', 'when', 'where', 'which', 'who', 'why', 'will',
  'with', 'you', 'your',
};

/// What gets a vector: the whole note, then each line or item (up to [maxLines]) with the title in front, so a
/// short line keeps its context ("Car: MOT due 14 June").
List<String> embedParts(Note n) {
  final whole = embedText(n);
  final lines = [
    for (final l in (n.isChecklist ? [for (final i in n.items!) i.text] : n.text.split('\n')))
      if (l.trim().isNotEmpty) l.trim()
  ];
  if (lines.length < 2 && n.title.trim().isEmpty) return [whole];
  final t = n.title.trim();
  return [whole, for (final l in lines.take(maxLines)) t.isEmpty ? l : '$t: $l'];
}

const maxLines = 40;

/// BM25 over the notes, built per search (there are tens or hundreds of notes, not thousands). The title counts
/// twice. The last query word, the one being typed, also matches words it starts ("batt" finds "batteries",
/// "0770" a phone number) at a little less weight, unless the query ends with a space.
Map<Note, double> keywordScores(String query, List<Note> notes, {double k1 = 1.2, double b = 0.75}) {
  final all = noteTokens(query);
  final content = all.where((w) => !_stop.contains(w)).toList();
  final q = (content.isEmpty ? all : content).toSet().toList();
  final typing = all.isNotEmpty && !query.endsWith(' ') && q.contains(all.last) ? all.last : null;
  if (q.isEmpty || notes.isEmpty) return {};
  final docs = [
    for (final n in notes) [...noteTokens(n.title), ...noteTokens(n.title), ...noteTokens(embedText(n))]
  ];
  final avg = docs.fold<int>(0, (a, d) => a + d.length) / docs.length;
  final out = <Note, double>{};
  // how much each document matches each query word: exact 1, prefix 0.8
  final tf = [
    for (final d in docs)
      [
        for (final w in q)
          d.fold<double>(0, (a, t) => a + (t == w ? 1 : (w == typing && t.startsWith(w) ? 0.8 : 0))),
      ]
  ];
  for (var j = 0; j < q.length; j++) {
    final df = tf.where((t) => t[j] > 0).length;
    if (df == 0) continue;
    final idf = math.log(1 + (docs.length - df + 0.5) / (df + 0.5));
    for (var i = 0; i < docs.length; i++) {
      final f = tf[i][j];
      if (f == 0) continue;
      final s = idf * f * (k1 + 1) / (f + k1 * (1 - b + b * docs[i].length / math.max(avg, 1)));
      out[notes[i]] = (out[notes[i]] ?? 0) + s;
    }
  }
  return out;
}

class NoteHit {
  NoteHit(this.note, this.score, {required this.keyword, required this.meaning});
  final Note note;
  final double score;
  final bool keyword, meaning;
}

/// A meaning match counts when its cosine (best part) is at least [minCos] and within [margin] of the best
/// note's. Measured with Qwen3-Embedding-0.6B (256 dims, the library's query instruction) on 10 everyday notes
/// and 20 queries: the right note came first for 15 of 17 (the misses: "blackout", and "doctor", which keyword
/// search finds), right answers scored 0.57-0.77, and a query about nothing in the notes ("earthquake") 0.55.
/// Nonsense can still bring up the closest note or two; keyword hits rank with them.
const minCos = 0.56, margin = 0.04;

/// Keyword and meaning results merged by reciprocal rank fusion (k = 60), best first. Without a query vector
/// (no model, or not ready yet) this is keyword search alone.
List<NoteHit> searchNotes(String query, List<Note> notes, {List<double>? queryVector, NoteVectors? vectors}) {
  final kw = keywordScores(query, notes).entries.toList()..sort((a, b) => b.value.compareTo(a.value));
  var dense = <(Note, double)>[];
  if (queryVector != null && vectors != null) {
    for (final n in notes) {
      final vs = vectors.of(n);
      if (vs != null && vs.isNotEmpty) dense.add((n, vs.map((v) => _dot(queryVector, v)).reduce(math.max)));
    }
    dense.sort((a, b) => b.$2.compareTo(a.$2));
    if (dense.isNotEmpty) {
      final cut = math.max(minCos, dense.first.$2 - margin);
      dense = dense.where((d) => d.$2 >= cut).toList();
    }
  }
  final score = <Note, double>{};
  final byKw = <Note>{}, byMeaning = <Note>{};
  for (var r = 0; r < kw.length; r++) {
    score[kw[r].key] = (score[kw[r].key] ?? 0) + 1 / (60 + r + 1);
    byKw.add(kw[r].key);
  }
  for (var r = 0; r < dense.length; r++) {
    score[dense[r].$1] = (score[dense[r].$1] ?? 0) + 1 / (60 + r + 1);
    byMeaning.add(dense[r].$1);
  }
  final hits = [
    for (final e in score.entries) NoteHit(e.key, e.value, keyword: byKw.contains(e.key), meaning: byMeaning.contains(e.key))
  ]..sort((a, b) => b.score.compareTo(a.score));
  return hits;
}

double _dot(List<double> a, Float32List b) {
  var s = 0.0;
  for (var i = 0; i < math.min(a.length, b.length); i++) {
    s += a[i] * b[i];
  }
  return s;
}

/// Cached note vectors in `<dir>/notes-emb.json`: per note id, a hash of what was embedded and the vectors of
/// its parts ([embedParts]). Only a cache: losing it costs re-embedding, never notes.
class NoteVectors {
  NoteVectors._(this.file);
  final File file;
  final Map<String, (int, List<Float32List>)> _v = {};
  bool _busy = false;

  /// The embedding model and query prefix are pinned (DESIGN §12); change this if they or [embedParts] change.
  static const model = 'qwen3-embedding-0.6b/256/parts1';

  static NoteVectors open(Directory dir) {
    final s = NoteVectors._(File('${dir.path}/notes-emb.json'));
    try {
      final j = jsonDecode(s.file.readAsStringSync()) as Map<String, dynamic>;
      if (j['model'] == model) {
        for (final e in (j['notes'] as Map<String, dynamic>).entries) {
          final m = e.value as Map<String, dynamic>;
          final vs = [
            for (final b in m['v'] as List) Uint8List.fromList(base64Decode(b as String)).buffer.asFloat32List() // aligned copy
          ];
          s._v[e.key] = ((m['h'] as num).toInt(), vs);
        }
      }
    } catch (_) {
      // missing or unreadable: start empty, the vectors are rebuilt
    }
    return s;
  }

  /// The note's part vectors, if they are up to date with the note's text.
  List<Float32List>? of(Note n) {
    final e = _v[n.id];
    return e != null && e.$1 == _hash(embedParts(n).join('\u0000')) ? e.$2 : null;
  }

  int get count => _v.length;

  /// Embeds new and changed notes (a note at a time, 16 parts per call) and forgets deleted ones. Returns how
  /// many notes were embedded.
  /// Overlapping calls do nothing; errors (no model, server restarting) leave the cache as it was.
  Future<int> refresh(List<Note> notes, Embed embed) async {
    if (_busy) return 0;
    _busy = true;
    var done = 0;
    try {
      final ids = {for (final n in notes) n.id};
      final gone = _v.keys.where((id) => !ids.contains(id)).toList();
      for (final id in gone) {
        _v.remove(id);
      }
      final stale = [for (final n in notes) if (!n.isEmpty && of(n) == null) n];
      for (final n in stale) {
        final parts = embedParts(n);
        final vs = <Float32List>[];
        for (var i = 0; i < parts.length; i += 16) {
          final r = await embed(parts.sublist(i, math.min(i + 16, parts.length)), query: false);
          vs.addAll([for (final v in r) Float32List.fromList(v)]);
        }
        _v[n.id] = (_hash(parts.join('\u0000')), vs);
        done++;
        if (done % 10 == 0) _save(); // a long first run keeps its progress
      }
      if (done > 0 || gone.isNotEmpty) _save();
    } finally {
      _busy = false;
    }
    return done;
  }

  void _save() {
    final data = jsonEncode({
      'model': model,
      'notes': {
        for (final e in _v.entries)
          e.key: {'h': e.value.$1, 'v': [for (final v in e.value.$2) base64Encode(v.buffer.asUint8List(v.offsetInBytes, v.lengthInBytes))]}
      },
    });
    final tmp = File('${file.path}.tmp');
    tmp.writeAsStringSync(data, flush: true);
    tmp.renameSync(file.path);
  }
}

/// FNV-1a over the UTF-8 bytes: notices any change to what was embedded.
int _hash(String s) {
  var h = 0x811c9dc5;
  for (final c in utf8.encode(s)) {
    h = ((h ^ c) * 0x01000193) & 0xffffffff;
  }
  return h;
}
