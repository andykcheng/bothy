// Map region picker for the Content tab: search, or browse by continent and country. Big countries
// (US, Canada, Russia, China, Brazil, Germany, France) come in parts (states, regions).
import 'package:flutter/material.dart';
import 'content.dart';

class MapPicker extends StatefulWidget {
  const MapPicker({super.key, required this.regions, required this.want, required this.installed, required this.onToggle});
  final List<Component> regions;
  final Set<String> want;
  final bool Function(String id) installed;
  final void Function(Component c, bool on) onToggle;

  @override
  State<MapPicker> createState() => _MapPickerState();
}

class _MapPickerState extends State<MapPicker> {
  final search = TextEditingController();
  String q = '';

  void _set(Component c, bool on) {
    widget.onToggle(c, on);
    setState(() {});
  }

  Widget _row(Component c, {bool showCountry = false}) {
    final sub = [
      formatBytes(c.size),
      if (showCountry && c.countryTitle != null) c.countryTitle!,
      if (widget.installed(c.id)) 'on this phone',
    ].join(' · ');
    return CheckboxListTile(
      value: widget.want.contains(c.id),
      onChanged: (v) => _set(c, v ?? false),
      title: Text(c.mapName),
      subtitle: Text(sub),
      dense: true,
      controlAffinity: ListTileControlAffinity.trailing,
    );
  }

  String _count(List<Component> cs) {
    final n = cs.where((c) => widget.want.contains(c.id)).length;
    final size = cs.fold<int>(0, (a, c) => a + c.size);
    return '${cs.length} ${cs.length == 1 ? 'map' : 'maps'} · ${formatBytes(size)}${n > 0 ? ' · $n chosen' : ''}';
  }

  @override
  Widget build(BuildContext context) {
    final t = Theme.of(context);
    final words = q.toLowerCase().split(' ').where((w) => w.isNotEmpty).toList();
    bool hit(Component c) {
      final hay = '${c.mapName} ${c.countryTitle ?? ''} ${c.id.substring(4)} ${c.continent ?? ''}'.toLowerCase();
      return words.every(hay.contains);
    }

    final List<Widget> body;
    if (words.isNotEmpty) {
      final found = widget.regions.where(hit).toList()..sort((a, b) => a.mapName.compareTo(b.mapName));
      body = found.isEmpty
          ? [Padding(padding: const EdgeInsets.all(24), child: Text('No map region matches “$q”.'))]
          : [for (final c in found) _row(c, showCountry: true)];
    } else {
      // continent → country → parts
      final byCont = <String, List<Component>>{};
      for (final c in widget.regions) {
        byCont.putIfAbsent(c.continent ?? 'Other', () => []).add(c);
      }
      final conts = byCont.keys.toList()..sort();
      body = [
        for (final cont in conts)
          ExpansionTile(
            title: Text(cont, style: t.textTheme.titleMedium),
            subtitle: Text(_count(byCont[cont]!)),
            children: () {
              final whole = byCont[cont]!.where((c) => c.country == null).toList();
              final parts = <String, List<Component>>{};
              for (final c in byCont[cont]!.where((c) => c.country != null)) {
                parts.putIfAbsent(c.countryTitle ?? c.country!, () => []).add(c);
              }
              final names = {for (final c in whole) c.mapName: c, for (final p in parts.keys) p: null}.keys.toList()..sort();
              return [
                for (final n in names)
                  if (parts.containsKey(n))
                    ExpansionTile(
                      tilePadding: const EdgeInsets.only(left: 32, right: 16),
                      childrenPadding: const EdgeInsets.only(left: 16),
                      title: Text(n),
                      subtitle: Text('In parts: ${_count(parts[n]!)}'),
                      children: [for (final c in parts[n]!..sort((a, b) => a.mapName.compareTo(b.mapName))) _row(c)],
                    )
                  else
                    Padding(padding: const EdgeInsets.only(left: 16), child: _row(whole.firstWhere((c) => c.mapName == n))),
              ];
            }(),
          ),
      ];
    }
    final chosen = widget.regions.where((c) => widget.want.contains(c.id)).toList();
    return Scaffold(
      appBar: AppBar(title: const Text('Map regions')),
      body: Column(children: [
        Padding(
          padding: const EdgeInsets.fromLTRB(16, 8, 16, 4),
          child: TextField(
            controller: search,
            decoration: InputDecoration(
              hintText: 'Search a country, state or region',
              prefixIcon: const Icon(Icons.search),
              suffixIcon: q.isEmpty ? null : IconButton(icon: const Icon(Icons.clear), onPressed: () => setState(() => q = search.text = '')),
            ),
            onChanged: (v) => setState(() => q = v),
          ),
        ),
        Padding(
          padding: const EdgeInsets.fromLTRB(20, 4, 20, 4),
          child: Text(
            chosen.isEmpty
                ? 'Street-level maps with place search, cut along borders. Pick where you live and where you travel.'
                : 'Chosen: ${chosen.length} · ${formatBytes(chosen.fold<int>(0, (a, c) => a + c.size))}. Go back and tap Download.',
            style: t.textTheme.bodySmall?.copyWith(color: t.colorScheme.onSurfaceVariant),
          ),
        ),
        Expanded(child: ListView(children: body)),
      ]),
    );
  }
}
