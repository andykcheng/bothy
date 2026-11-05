import 'dart:io';

import 'package:flutter/material.dart';
import 'package:flutter_test/flutter_test.dart';
import 'package:survivalpedia/notes.dart';
import 'package:survivalpedia/theme.dart';

void main() {
  late Directory tmp;
  setUp(() => tmp = Directory.systemTemp.createTempSync('sp-notes'));
  tearDown(() => tmp.deleteSync(recursive: true));

  test('notes and checklists survive a restart; empty notes are not kept', () async {
    final s = await NotesStore.open(tmp);
    s.add(title: 'Contacts', text: 'Mum 07700 900123');
    final c = s.add(title: 'Power cut', items: [CheckItem('Torch'), CheckItem('Water', true)]);
    c.pinned = true;
    s.add(); // empty
    s.flush();

    final again = await NotesStore.open(tmp);
    expect(again.notes.length, 2);
    final first = again.sorted.first;
    expect(first.title, 'Power cut');
    expect(first.pinned, isTrue);
    expect([for (final i in first.items!) (i.text, i.done)], [('Torch', false), ('Water', true)]);
    expect(again.sorted[1].text, 'Mum 07700 900123');
    expect(again.sorted[1].isChecklist, isFalse);
  });

  test('an unreadable notes file is kept aside, never overwritten', () async {
    File('${tmp.path}/notes.json').writeAsStringSync('{not json');
    final s = await NotesStore.open(tmp);
    expect(s.notes, isEmpty);
    s.add(text: 'new');
    s.flush();
    final kept = tmp.listSync().whereType<File>().where((f) => f.path.contains('notes.json.unreadable-'));
    expect(kept.single.readAsStringSync(), '{not json');
  });

  test('phone numbers', () {
    expect(phoneNumbers('Mum: 07700 900123, office +44 (0)20 7946 0018'), ['07700 900123', '+44 (0)20 7946 0018']);
    expect(phoneNumbers('Emergency 112 or 999'), ['112', '999']);
    expect(phoneNumbers('3 litres, 12 items, room 1120'), isEmpty);
  });

  testWidgets('make a checklist, tick an item, and it is saved', (tester) async {
    final s = (await tester.runAsync(() => NotesStore.open(tmp)))!;
    await tester.pumpWidget(MaterialApp(theme: buildTheme(Brightness.light), home: Scaffold(body: NotesPage(store: s))));
    await tester.tap(find.text('New checklist'));
    await tester.pumpAndSettle();
    await tester.enterText(find.widgetWithText(TextField, 'Title'), 'Grab bag');
    await tester.enterText(find.widgetWithText(TextField, 'Item'), 'Torch');
    await tester.testTextInput.receiveAction(TextInputAction.next); // adds the next item
    await tester.pump();
    await tester.enterText(find.widgetWithText(TextField, 'Item').last, 'Call 07700 900123');
    await tester.pump();
    expect(find.widgetWithText(ActionChip, '07700 900123'), findsOneWidget);
    await tester.tap(find.byType(Checkbox).first);
    await tester.pump();
    expect(find.text('TICKED · 1'), findsOneWidget);
    await tester.pageBack();
    await tester.pumpAndSettle();
    expect(find.text('1 of 2 done'), findsOneWidget);
    s.flush();

    final again = (await tester.runAsync(() => NotesStore.open(tmp)))!;
    final n = again.notes.single;
    expect(n.title, 'Grab bag');
    expect([for (final i in n.items!) (i.text, i.done)], [('Torch', true), ('Call 07700 900123', false)]);
  });

  testWidgets('an untouched new note is dropped', (tester) async {
    final s = (await tester.runAsync(() => NotesStore.open(tmp)))!;
    await tester.pumpWidget(MaterialApp(home: Scaffold(body: NotesPage(store: s))));
    await tester.tap(find.text('New note'));
    await tester.pumpAndSettle();
    await tester.pageBack();
    await tester.pumpAndSettle();
    expect(s.notes, isEmpty);
    expect(find.text('New note'), findsOneWidget); // back to the empty page
  });
}
