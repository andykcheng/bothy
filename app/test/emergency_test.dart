import 'dart:convert';
import 'dart:io';

import 'package:flutter/material.dart';
import 'package:flutter_test/flutter_test.dart';
import 'package:survivalpedia/emergency.dart';

void main() {
  final data = FirstAid.fromJson(jsonDecode(File('assets/emergency/cards.json').readAsStringSync()) as Map<String, dynamic>);

  test('every card has steps and a reason to call', () {
    expect(data.cards.length, greaterThanOrEqualTo(15));
    for (final c in data.cards) {
      expect(c.steps, isNotEmpty, reason: c.id);
      expect(c.callIf, isNotEmpty, reason: c.id);
    }
    expect(data.cards.map((c) => c.id), containsAll(['cpr', 'choking', 'bleeding', 'poisoning', 'burns', 'stroke']));
  });

  testWidgets('list opens a card with numbered steps', (tester) async {
    await tester.pumpWidget(MaterialApp(home: EmergencyPage(data: Future.value(data))));
    await tester.pumpAndSettle();
    expect(find.text('Not breathing: CPR'), findsOneWidget);
    await tester.scrollUntilVisible(find.text('Poisoning'), 200);
    await tester.tap(find.text('Poisoning'));
    await tester.pumpAndSettle();
    expect(find.text('What to do'), findsOneWidget);
    expect(find.text('1'), findsOneWidget);
    final dont = find.textContaining("Don't make them sick");
    await tester.scrollUntilVisible(dont, 200,
        scrollable: find.descendant(of: find.byType(FirstAidCardPage), matching: find.byType(Scrollable)));
    expect(dont, findsOneWidget);
  });
}
