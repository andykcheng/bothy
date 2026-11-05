// First aid: hand-written cards (CPR, choking, bleeding, poisoning…) bundled in the APK, so they work with
// nothing downloaded and no library running. The hub serves the same file at /emergency and puts the matching
// card above search results and Ask answers (core/crates/sp-server/src/emergency.rs).

import 'dart:convert';
import 'dart:io';

import 'package:flutter/material.dart';
import 'package:flutter/services.dart';

const _files = MethodChannel('survivalpedia/files');
const emergencyRed = Color(0xFFC62828);

class FirstAidCard {
  FirstAidCard.fromJson(Map<String, dynamic> j)
      : id = j['id'] as String,
        title = j['title'] as String,
        when = j['when'] as String,
        steps = _strings(j['steps']),
        children = _strings(j['children']),
        dont = _strings(j['dont']),
        callIf = j['call_if'] as String;
  final String id, title, when, callIf;
  final List<String> steps, children, dont;
}

List<String> _strings(Object? v) => [for (final s in (v as List?) ?? const []) s as String];

class FirstAid {
  FirstAid.fromJson(Map<String, dynamic> j)
      : call = j['call'] as String,
        disclaimer = j['disclaimer'] as String,
        cards = [for (final c in j['cards'] as List) FirstAidCard.fromJson(c as Map<String, dynamic>)];
  final String call, disclaimer;
  final List<FirstAidCard> cards;

  static Future<FirstAid>? _loaded;
  static Future<FirstAid> load([AssetBundle? bundle]) => _loaded ??= (bundle ?? rootBundle)
      .loadString('assets/emergency/cards.json')
      .then((s) => FirstAid.fromJson(jsonDecode(s) as Map<String, dynamic>));
}

/// The numbers offered for a call: 112 works from most mobiles worldwide (and in the UK beside 999).
const emergencyNumbers = ['112', '999', '911', '000'];

Future<void> dialEmergency(BuildContext context, String number) async {
  try {
    await _files.invokeMethod<void>('dial', {'number': number});
  } on PlatformException catch (e) {
    if (context.mounted) {
      ScaffoldMessenger.of(context).showSnackBar(SnackBar(content: Text(e.message ?? 'Could not open the phone app')));
    }
  } on MissingPluginException {
    // tests and desktop: no phone app
  }
}

class _CallBar extends StatelessWidget {
  const _CallBar({required this.text});
  final String text;
  @override
  Widget build(BuildContext context) {
    final t = Theme.of(context);
    return Container(
      padding: const EdgeInsets.all(12),
      decoration: BoxDecoration(
        color: emergencyRed.withValues(alpha: 0.08),
        border: Border.all(color: emergencyRed, width: 1.5),
        borderRadius: BorderRadius.circular(14),
      ),
      child: Column(crossAxisAlignment: CrossAxisAlignment.start, children: [
        Text(text, style: t.textTheme.bodyMedium?.copyWith(fontWeight: FontWeight.w600)),
        if (Platform.isAndroid) ...[
          const SizedBox(height: 8),
          Wrap(spacing: 8, runSpacing: 8, children: [
            for (final n in emergencyNumbers)
              FilledButton.icon(
                style: FilledButton.styleFrom(backgroundColor: emergencyRed, foregroundColor: Colors.white),
                onPressed: () => dialEmergency(context, n),
                icon: const Icon(Icons.call, size: 18),
                label: Text(n),
              ),
          ]),
        ],
      ]),
    );
  }
}

/// The list of cards, opened from the red First aid button.
class EmergencyPage extends StatelessWidget {
  const EmergencyPage({super.key, this.data});
  final Future<FirstAid>? data;

  @override
  Widget build(BuildContext context) {
    final t = Theme.of(context);
    return Scaffold(
      appBar: AppBar(title: const Text('First aid'), foregroundColor: emergencyRed),
      body: FutureBuilder<FirstAid>(
        future: data ?? FirstAid.load(),
        builder: (context, snap) {
          final fa = snap.data;
          if (fa == null) {
            return Center(child: snap.hasError ? Text('Could not load the cards: ${snap.error}') : const CircularProgressIndicator());
          }
          return ListView(padding: const EdgeInsets.fromLTRB(16, 4, 16, 24), children: [
            _CallBar(text: fa.call),
            const SizedBox(height: 12),
            for (final c in fa.cards)
              Padding(
                padding: const EdgeInsets.only(bottom: 8),
                child: Card(
                  child: Container(
                    decoration: const BoxDecoration(border: Border(left: BorderSide(color: emergencyRed, width: 4))),
                    child: ListTile(
                      title: Text(c.title, style: t.textTheme.titleMedium),
                      subtitle: Text(c.when, maxLines: 2, overflow: TextOverflow.ellipsis),
                      trailing: const Icon(Icons.chevron_right),
                      onTap: () => Navigator.of(context)
                          .push(MaterialPageRoute<void>(builder: (_) => FirstAidCardPage(card: c, call: fa.call, disclaimer: fa.disclaimer))),
                    ),
                  ),
                ),
              ),
            const SizedBox(height: 8),
            Text(fa.disclaimer, style: t.textTheme.bodySmall?.copyWith(color: t.colorScheme.onSurfaceVariant)),
          ]);
        },
      ),
    );
  }
}

/// One card: numbered steps in large type, then children and babies, what not to do, and when to call.
class FirstAidCardPage extends StatelessWidget {
  const FirstAidCardPage({super.key, required this.card, required this.call, required this.disclaimer});
  final FirstAidCard card;
  final String call, disclaimer;

  @override
  Widget build(BuildContext context) {
    final t = Theme.of(context);
    final big = t.textTheme.titleMedium?.copyWith(fontWeight: FontWeight.w400, height: 1.4, fontSize: 19);
    Widget head(String s) => Padding(
          padding: const EdgeInsets.only(top: 20, bottom: 6),
          child: Text(s, style: t.textTheme.titleMedium?.copyWith(color: emergencyRed)),
        );
    Widget bullets(List<String> items) => Column(children: [
          for (final i in items)
            Padding(
              padding: const EdgeInsets.only(bottom: 6),
              child: Row(crossAxisAlignment: CrossAxisAlignment.start, children: [
                const Padding(padding: EdgeInsets.only(top: 8, right: 10), child: Icon(Icons.circle, size: 7)),
                Expanded(child: Text(i, style: t.textTheme.bodyLarge)),
              ]),
            ),
        ]);
    return Scaffold(
      appBar: AppBar(title: Text(card.title), foregroundColor: emergencyRed),
      body: ListView(padding: const EdgeInsets.fromLTRB(16, 4, 16, 32), children: [
        Text(card.when, style: t.textTheme.bodyLarge?.copyWith(color: t.colorScheme.onSurfaceVariant)),
        head('What to do'),
        for (var i = 0; i < card.steps.length; i++)
          Padding(
            padding: const EdgeInsets.only(bottom: 12),
            child: Row(crossAxisAlignment: CrossAxisAlignment.start, children: [
              CircleAvatar(
                radius: 14,
                backgroundColor: emergencyRed,
                foregroundColor: Colors.white,
                child: Text('${i + 1}', style: const TextStyle(fontWeight: FontWeight.w700, fontSize: 14)),
              ),
              const SizedBox(width: 12),
              Expanded(child: Text(card.steps[i], style: big)),
            ]),
          ),
        if (card.children.isNotEmpty) ...[head('Children and babies'), bullets(card.children)],
        if (card.dont.isNotEmpty) ...[head("Don't"), bullets(card.dont)],
        head('Call the emergency number'),
        Text(card.callIf, style: t.textTheme.bodyLarge?.copyWith(fontWeight: FontWeight.w600)),
        const SizedBox(height: 12),
        _CallBar(text: call),
        const SizedBox(height: 16),
        Text(disclaimer, style: t.textTheme.bodySmall?.copyWith(color: t.colorScheme.onSurfaceVariant)),
      ]),
    );
  }
}

/// Opens the built-in cards. Shown on the Library tab when there is no library to show its own First aid page
/// (the library's home page and header link to /emergency, the same cards).
class FirstAidButton extends StatelessWidget {
  const FirstAidButton({super.key});
  @override
  Widget build(BuildContext context) => OutlinedButton.icon(
        style: OutlinedButton.styleFrom(foregroundColor: emergencyRed, side: const BorderSide(color: emergencyRed)),
        onPressed: () => Navigator.of(context).push(MaterialPageRoute<void>(builder: (_) => const EmergencyPage())),
        icon: const Icon(Icons.medical_services_outlined),
        label: const Text('First aid'),
      );
}
