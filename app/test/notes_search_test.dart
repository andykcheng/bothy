import 'dart:io';

import 'package:flutter_test/flutter_test.dart';
import 'package:survivalpedia/notes.dart';
import 'package:survivalpedia/notes_search.dart';

void main() {
  late Directory tmp;
  setUp(() => tmp = Directory.systemTemp.createTempSync('sp-notes-search'));
  tearDown(() => tmp.deleteSync(recursive: true));

  Future<NotesStore> store() async {
    final s = await NotesStore.open(tmp);
    s.add(title: 'Water', text: 'Stopcock is under the kitchen sink');
    s.add(title: 'Power cut', items: [CheckItem('Torch and spare batteries'), CheckItem('Power bank charged')]);
    s.add(title: 'Contacts', text: 'Doctor 01632 960123\nWater company 0800 000 000');
    s.add(title: "Dad's tablets", text: 'Amlodipine 5 mg\nAllergic to penicillin');
    return s;
  }

  test('a note is embedded whole and line by line, with its title', () {
    final n = Note(id: 'x', title: 'Car', text: 'MOT due 14 June\n\nSpare key at number 12');
    expect(embedParts(n), ['Car\nMOT due 14 June\n\nSpare key at number 12', 'Car: MOT due 14 June', 'Car: Spare key at number 12']);
    expect(embedParts(Note(id: 'y', text: 'just one line')), ['just one line']);
  });

  test('little words do not match everything; only the word being typed matches word starts', () async {
    final s = await store();
    expect(searchNotes('who to call if someone is ill', s.notes), isEmpty);
    expect(searchNotes('stop', s.notes).single.note.title, 'Water'); // typing "stop…"
    expect(searchNotes('stop ', s.notes), isEmpty); // a finished word must match whole
  });

  test('keyword search ranks by BM25, title first, and matches word starts while typing', () async {
    final s = await store();
    final hits = searchNotes('water', s.notes);
    expect([for (final h in hits) h.note.title], ['Water', 'Contacts']); // the title counts more
    expect(searchNotes('batt', s.notes).single.note.title, 'Power cut');
    expect(searchNotes('01632', s.notes).single.note.title, 'Contacts');
    expect(searchNotes('PENICILLIN', s.notes).single.note.title, "Dad's tablets");
    expect(searchNotes('earthquake', s.notes), isEmpty);
    expect(searchNotes('stopcock sink', s.notes).first.keyword, isTrue);
  });

  // A stand-in model: one dimension per topic, so "meaning" is known exactly.
  const topics = ['water', 'power', 'medicine', 'phone'];
  List<double> fake(String t) {
    final l = t.toLowerCase();
    final v = [
      for (final k in [
        ['water', 'stopcock', 'tap'],
        ['power', 'torch', 'electricity'],
        ['tablets', 'penicillin', 'blood pressure', 'medicine'],
        ['doctor', 'call', 'contacts'],
      ])
        k.any(l.contains) ? 1.0 : 0.0
    ];
    final n = v.fold<double>(0, (a, x) => a + x * x);
    return n == 0 ? List.filled(topics.length, 0.0) : [for (final x in v) x / n];
  }

  var calls = 0;
  Future<List<List<double>>> embed(List<String> texts, {required bool query}) async {
    calls += texts.length;
    return [for (final t in texts) fake(t)];
  }

  test('search by meaning finds notes with none of the words, merged with keyword hits', () async {
    final s = await store();
    expect(await s.vectors.refresh(s.notes, embed), 4);
    final q = fake('blood pressure');
    final hits = searchNotes('blood pressure', s.notes, queryVector: q, vectors: s.vectors);
    expect(hits.first.note.title, "Dad's tablets");
    expect(hits.first.meaning, isTrue);
    expect(hits.first.keyword, isFalse);
    // unrelated meaning stays out
    expect(searchNotes('qwerty', s.notes, queryVector: List.filled(4, 0.0), vectors: s.vectors), isEmpty);
  });

  test('vectors are cached on disk, redone only for edited notes, and dropped for deleted ones', () async {
    final s = await store();
    calls = 0;
    expect(await s.vectors.refresh(s.notes, embed), 4);
    expect(calls, 2 + 3 + 3 + 3); // each note whole, plus each of its lines
    calls = 0;
    expect(await s.vectors.refresh(s.notes, embed), 0); // nothing changed
    final water = s.notes.first;
    water.text = 'Stopcock is under the stairs';
    expect(await s.vectors.refresh(s.notes, embed), 1);
    s.remove(s.notes.last);
    await s.vectors.refresh(s.notes, embed);

    final again = await NotesStore.open(tmp);
    expect(again.vectors.count, 3);
    calls = 0;
    expect(await again.vectors.refresh(again.notes, embed), 0);
    expect(calls, 0);
    expect(again.vectors.of(again.notes.firstWhere((n) => n.title == 'Water'))!.length, 2);
  });

  test('an embed failure leaves keyword search working', () async {
    final s = await store();
    Future<List<List<double>>> broken(List<String> t, {required bool query}) async => throw StateError('no model');
    await expectLater(s.vectors.refresh(s.notes, broken), throwsStateError);
    expect(searchNotes('torch', s.notes, vectors: s.vectors).single.note.title, 'Power cut');
  });
}
