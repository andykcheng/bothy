// The Notes tab: plain notes and checklists kept on the phone (phone numbers, what to do in a power cut,
// what's in the grab bag…). Nothing here needs content, the core or a network. Notes are one JSON file
// in the app's private storage, separate from downloaded content, written atomically after each change.

import 'dart:async';
import 'dart:convert';
import 'dart:io';

import 'package:flutter/material.dart';
import 'package:flutter/services.dart';
import 'package:path_provider/path_provider.dart';

import 'notes_search.dart';

class CheckItem {
  CheckItem(this.text, [this.done = false]);
  String text;
  bool done;
}

class Note {
  Note({required this.id, this.title = '', this.text = '', this.items, this.pinned = false, DateTime? created, DateTime? updated})
      : created = created ?? DateTime.now(),
        updated = updated ?? DateTime.now();

  factory Note.fromJson(Map<String, dynamic> j) => Note(
        id: j['id'] as String,
        title: j['title'] as String? ?? '',
        text: j['text'] as String? ?? '',
        items: j['items'] == null
            ? null
            : [for (final i in j['items'] as List) CheckItem((i as Map)['t'] as String? ?? '', i['d'] == true)],
        pinned: j['pinned'] == true,
        created: DateTime.fromMillisecondsSinceEpoch((j['created'] as num?)?.toInt() ?? 0),
        updated: DateTime.fromMillisecondsSinceEpoch((j['updated'] as num?)?.toInt() ?? 0),
      );

  final String id;
  String title, text;
  List<CheckItem>? items; // null for a plain note
  bool pinned;
  final DateTime created;
  DateTime updated;

  bool get isChecklist => items != null;
  bool get isEmpty => title.trim().isEmpty && text.trim().isEmpty && (items ?? []).every((i) => i.text.trim().isEmpty);

  /// Everything in the note as plain text (search, copying).
  String get plain => [
        if (title.trim().isNotEmpty) title.trim(),
        if (!isChecklist && text.trim().isNotEmpty) text.trim(),
        for (final i in items ?? <CheckItem>[])
          if (i.text.trim().isNotEmpty) '${i.done ? '[x]' : '[ ]'} ${i.text.trim()}',
      ].join('\n');

  Map<String, dynamic> toJson() => {
        'id': id,
        'title': title,
        if (!isChecklist) 'text': text,
        if (isChecklist) 'items': [for (final i in items!) {'t': i.text, 'd': i.done}],
        'pinned': pinned,
        'created': created.millisecondsSinceEpoch,
        'updated': updated.millisecondsSinceEpoch,
      };
}

/// All notes, saved to `<app support>/notes.json`. Changes are saved shortly after they happen; [flush]
/// saves at once.
class NotesStore extends ChangeNotifier {
  NotesStore._(this.file);
  final File file;
  final List<Note> notes = [];
  Timer? _timer;

  /// Cached vectors for search by meaning (notes_search.dart), kept beside notes.json.
  late final NoteVectors vectors = NoteVectors.open(file.parent);

  static Future<NotesStore> open([Directory? dir]) async {
    dir ??= await getApplicationSupportDirectory();
    await dir.create(recursive: true);
    final s = NotesStore._(File('${dir.path}/notes.json'));
    await s._load();
    return s;
  }

  Future<void> _load() async {
    if (!await file.exists()) return;
    try {
      final j = jsonDecode(await file.readAsString()) as Map<String, dynamic>;
      notes.addAll([for (final n in j['notes'] as List) Note.fromJson(n as Map<String, dynamic>)]);
    } catch (_) {
      // never overwrite notes we could not read: keep the file aside and start afresh
      await file.rename('${file.path}.unreadable-${DateTime.now().millisecondsSinceEpoch}');
    }
  }

  /// Pinned first, then most recently changed.
  List<Note> get sorted => [...notes]..sort((a, b) {
      if (a.pinned != b.pinned) return a.pinned ? -1 : 1;
      return b.updated.compareTo(a.updated);
    });

  Note add({String title = '', String text = '', List<CheckItem>? items}) {
    final n = Note(id: DateTime.now().microsecondsSinceEpoch.toRadixString(36), title: title, text: text, items: items);
    notes.add(n);
    changed(n);
    return n;
  }

  /// Call after editing a note.
  void changed(Note n, {bool touch = true}) {
    if (touch) n.updated = DateTime.now();
    notifyListeners();
    _timer?.cancel();
    _timer = Timer(const Duration(milliseconds: 400), _save);
  }

  void remove(Note n) {
    notes.remove(n);
    notifyListeners();
    flush();
  }

  /// Puts a removed note back (undo).
  void restore(Note n) {
    if (!notes.contains(n)) notes.add(n);
    notifyListeners();
    flush();
  }

  /// Saves now.
  void flush() {
    _timer?.cancel();
    _save();
  }

  // Synchronous: the file is small (a few KB), and a save is complete before the app can be killed.
  // Written to a temporary file and renamed over the old one, so a crash never leaves half a file.
  void _save() {
    final data = jsonEncode({
      'version': 1,
      'notes': [for (final n in notes) if (!n.isEmpty) n.toJson()],
    });
    final tmp = File('${file.path}.tmp');
    tmp.writeAsStringSync(data, flush: true);
    tmp.renameSync(file.path);
  }
}

// Starter notes: offered while there are none, and from the New menu.
const _starters = <(String, IconData, String, List<String>?)>[
  (
    'Emergency contacts',
    Icons.contact_phone_outlined,
    'Emergency services: 112 works from most mobile phones, even without a SIM in many countries\n'
        'Local emergency number: \n'
        'Power cut / electricity company: \n'
        'Water company: \n'
        'Gas emergency: \n'
        'Doctor: \n'
        'Family: \n'
        'Neighbours: \n'
        'Work / school: \n'
        'Meeting place if we are separated: ',
    null,
  ),
  (
    'Power cut checklist',
    Icons.flashlight_on_outlined,
    '',
    [
      'Torch and spare batteries (avoid candles)',
      'Phone charged, power bank charged, low-power mode on',
      'Important numbers written on paper too',
      'Fridge and freezer kept closed',
      'Sensitive electronics unplugged; one light left on to see when power returns',
      'Drinking water: about 3 litres per person per day',
      'Food that needs no cooking or fridge',
      'Warm layers and blankets (or keep cool in the heat)',
      'Battery or wind-up radio for news',
      'Some cash (card machines may not work)',
      'Check on elderly or vulnerable neighbours',
      'Never use a generator, barbecue or camping stove indoors (carbon monoxide)',
    ],
  ),
  (
    'Grab bag',
    Icons.backpack_outlined,
    '',
    [
      'Water and snacks',
      'Torch, radio and spare batteries',
      'First aid kit',
      'Medicines and a copy of prescriptions',
      'Copies of ID, insurance and other documents',
      'Cash',
      'Phone charger and power bank',
      'Warm layer and rain jacket',
      'Spare glasses or contact lenses',
      'Whistle',
      'Notebook and pencil',
      'Spare keys',
    ],
  ),
];

Note _addStarter(NotesStore store, (String, IconData, String, List<String>?) s) =>
    store.add(title: s.$1, text: s.$3, items: s.$4 == null ? null : [for (final t in s.$4!) CheckItem(t)]);

/// Phone numbers in a note's text: runs of at least 7 digits (spaces, dashes, dots and brackets allowed,
/// an optional leading +), and the common emergency numbers.
List<String> phoneNumbers(String text) {
  final out = <String>{};
  // emergency numbers first
  for (final m in RegExp(r'(?<![\d+])(112|911|999|000)(?!\d)').allMatches(text)) {
    out.add(m.group(1)!);
  }
  for (final m in RegExp(r'\+?\(?\d[\d \-.()]{5,}\d').allMatches(text)) {
    final s = m.group(0)!.trim();
    if (s.replaceAll(RegExp(r'\D'), '').length >= 7) out.add(s);
  }
  return out.toList();
}

const _files = MethodChannel('survivalpedia/files');

/// Call or copy a phone number.
void numberMenu(BuildContext context, String number) {
  showModalBottomSheet<void>(
    context: context,
    showDragHandle: true,
    builder: (ctx) => SafeArea(
      child: Column(mainAxisSize: MainAxisSize.min, children: [
        ListTile(title: Text(number, style: Theme.of(ctx).textTheme.titleLarge)),
        if (Platform.isAndroid)
          ListTile(
            leading: const Icon(Icons.call_outlined),
            title: const Text('Call'),
            subtitle: const Text('Opens the phone app with the number filled in'),
            onTap: () async {
              Navigator.pop(ctx);
              try {
                await _files.invokeMethod<void>('dial', {'number': number});
              } on PlatformException catch (e) {
                if (context.mounted) {
                  ScaffoldMessenger.of(context).showSnackBar(SnackBar(content: Text(e.message ?? 'Could not open the phone app')));
                }
              }
            },
          ),
        ListTile(
          leading: const Icon(Icons.copy_outlined),
          title: const Text('Copy'),
          onTap: () {
            Navigator.pop(ctx);
            Clipboard.setData(ClipboardData(text: number));
            ScaffoldMessenger.of(context).showSnackBar(const SnackBar(content: Text('Number copied')));
          },
        ),
      ]),
    ),
  );
}

String _when(DateTime d) {
  final now = DateTime.now();
  String two(int n) => n.toString().padLeft(2, '0');
  final day = DateTime(d.year, d.month, d.day);
  final today = DateTime(now.year, now.month, now.day);
  if (day == today) return '${two(d.hour)}:${two(d.minute)}';
  if (day == today.subtract(const Duration(days: 1))) return 'Yesterday';
  const m = ['Jan', 'Feb', 'Mar', 'Apr', 'May', 'Jun', 'Jul', 'Aug', 'Sep', 'Oct', 'Nov', 'Dec'];
  return '${d.day} ${m[d.month - 1]}${d.year == now.year ? '' : ' ${d.year}'}';
}

class NotesPage extends StatefulWidget {
  const NotesPage({super.key, required this.store, this.embed});
  final NotesStore store;

  /// The library's embedding model, when installed: adds search by meaning to keyword search.
  final Embed? embed;
  @override
  State<NotesPage> createState() => _NotesPageState();
}

class _NotesPageState extends State<NotesPage> {
  String _query = '';
  Timer? _queryTimer, _refreshTimer;
  List<double>? _qvec; // the vector of [_qvecFor]
  String? _qvecFor;
  bool _meaning = false; // the model answered: search by meaning works

  @override
  void initState() {
    super.initState();
    widget.store.addListener(_changed);
    _refreshVectors();
  }

  @override
  void didUpdateWidget(NotesPage old) {
    super.didUpdateWidget(old);
    if (widget.embed != null && old.embed == null) _refreshVectors(); // the library (and model) just started
    if (widget.embed == null) _meaning = false;
  }

  @override
  void dispose() {
    widget.store.removeListener(_changed);
    _queryTimer?.cancel();
    _refreshTimer?.cancel();
    super.dispose();
  }

  void _changed() {
    if (mounted) setState(() {});
    _refreshTimer?.cancel();
    _refreshTimer = Timer(const Duration(seconds: 2), _refreshVectors);
  }

  /// Embeds new and edited notes in the background, so searches by meaning only wait for the query.
  Future<void> _refreshVectors() async {
    final embed = widget.embed;
    if (embed == null) return;
    try {
      await widget.store.vectors.refresh(widget.store.notes, embed);
      if (mounted && !_meaning) setState(() => _meaning = true);
    } catch (_) {
      // no model, or the server is restarting: keyword search carries on alone
    }
  }

  void _search(String v) {
    setState(() => _query = v);
    _queryTimer?.cancel();
    final q = v.trim();
    final embed = widget.embed;
    if (q.isEmpty || embed == null) return;
    _queryTimer = Timer(const Duration(milliseconds: 300), () async {
      try {
        final r = await embed([q], query: true);
        if (!mounted || _query.trim() != q) return;
        setState(() {
          _qvec = r.first;
          _qvecFor = q;
          _meaning = true;
        });
      } catch (_) {}
    });
  }

  Future<void> _open(Note n) async {
    await Navigator.of(context).push(MaterialPageRoute<void>(builder: (_) => NoteEditor(store: widget.store, note: n)));
  }

  void _newMenu() {
    showModalBottomSheet<void>(
      context: context,
      showDragHandle: true,
      builder: (ctx) => SafeArea(
        child: Column(mainAxisSize: MainAxisSize.min, children: [
          ListTile(
            leading: const Icon(Icons.notes_outlined),
            title: const Text('Note'),
            subtitle: const Text('Text, phone numbers, anything to remember'),
            onTap: () {
              Navigator.pop(ctx);
              _open(widget.store.add());
            },
          ),
          ListTile(
            leading: const Icon(Icons.checklist_outlined),
            title: const Text('Checklist'),
            subtitle: const Text('Things to do or to pack, ticked off one by one'),
            onTap: () {
              Navigator.pop(ctx);
              _open(widget.store.add(items: [CheckItem('')]));
            },
          ),
          const Divider(),
          for (final s in _starters)
            ListTile(
              leading: Icon(s.$2),
              title: Text(s.$1),
              subtitle: const Text('Ready-made, edit it to suit you'),
              onTap: () {
                Navigator.pop(ctx);
                _open(_addStarter(widget.store, s));
              },
            ),
        ]),
      ),
    );
  }

  void _menu(Note n) {
    showModalBottomSheet<void>(
      context: context,
      showDragHandle: true,
      builder: (ctx) => SafeArea(
        child: Column(mainAxisSize: MainAxisSize.min, children: [
          ListTile(
            leading: Icon(n.pinned ? Icons.push_pin : Icons.push_pin_outlined),
            title: Text(n.pinned ? 'Unpin' : 'Pin to the top'),
            onTap: () {
              Navigator.pop(ctx);
              n.pinned = !n.pinned;
              widget.store.changed(n, touch: false);
            },
          ),
          ListTile(
            leading: const Icon(Icons.copy_outlined),
            title: const Text('Copy as text'),
            onTap: () {
              Navigator.pop(ctx);
              Clipboard.setData(ClipboardData(text: n.plain));
              ScaffoldMessenger.of(context).showSnackBar(const SnackBar(content: Text('Copied')));
            },
          ),
          ListTile(
            leading: const Icon(Icons.delete_outline),
            title: const Text('Delete'),
            onTap: () {
              Navigator.pop(ctx);
              deleteWithUndo(context, widget.store, n);
            },
          ),
        ]),
      ),
    );
  }

  @override
  Widget build(BuildContext context) {
    final t = Theme.of(context);
    final all = widget.store.sorted.where((n) => !n.isEmpty).toList();
    final q = _query.trim();
    final hits = q.isEmpty
        ? null
        : searchNotes(q, all, queryVector: _qvecFor == q ? _qvec : null, vectors: widget.store.vectors);
    final notes = hits == null ? all : [for (final h in hits) h.note];
    return CustomScrollView(slivers: [
      SliverToBoxAdapter(
        child: Padding(
          padding: const EdgeInsets.fromLTRB(20, 16, 12, 0),
          child: Row(children: [
            Expanded(child: Text('Notes', style: t.textTheme.headlineMedium)),
            FilledButton.tonalIcon(onPressed: _newMenu, icon: const Icon(Icons.add), label: const Text('New')),
          ]),
        ),
      ),
      SliverToBoxAdapter(
        child: Padding(
          padding: const EdgeInsets.fromLTRB(20, 4, 20, 8),
          child: Text(
            all.isEmpty
                ? 'Phone numbers, checklists and anything you need to remember. Kept on this phone; no signal or power needed.'
                : '${all.length} note${all.length == 1 ? '' : 's'} · saved on this phone',
            style: t.textTheme.bodyMedium?.copyWith(color: t.colorScheme.onSurfaceVariant),
          ),
        ),
      ),
      if (all.isNotEmpty)
        SliverToBoxAdapter(
          child: Padding(
            padding: const EdgeInsets.fromLTRB(16, 4, 16, 8),
            child: TextField(
              decoration: InputDecoration(
                prefixIcon: const Icon(Icons.search),
                hintText: _meaning ? 'Search notes by words or meaning' : 'Search notes',
              ),
              onChanged: _search,
            ),
          ),
        ),
      if (all.isEmpty)
        SliverFillRemaining(
          hasScrollBody: false,
          child: Padding(
            padding: const EdgeInsets.fromLTRB(20, 8, 20, 24),
            child: Column(crossAxisAlignment: CrossAxisAlignment.stretch, children: [
              Row(children: [
                Expanded(
                  child: _StartCard(icon: Icons.notes_outlined, label: 'New note', onTap: () => _open(widget.store.add())),
                ),
                const SizedBox(width: 12),
                Expanded(
                  child: _StartCard(
                    icon: Icons.checklist_outlined,
                    label: 'New checklist',
                    onTap: () => _open(widget.store.add(items: [CheckItem('')])),
                  ),
                ),
              ]),
              const SizedBox(height: 8),
              Padding(
                padding: const EdgeInsets.only(top: 16, bottom: 8),
                child: Text('READY-MADE', style: t.textTheme.labelSmall?.copyWith(color: t.colorScheme.onSurfaceVariant)),
              ),
              for (final s in _starters)
                Padding(
                  padding: const EdgeInsets.only(bottom: 8),
                  child: Card(
                    child: ListTile(
                      leading: Icon(s.$2, color: t.colorScheme.primary),
                      title: Text(s.$1),
                      subtitle: Text(s.$4 == null ? 'Fill in the numbers that matter to you' : '${s.$4!.length} things to check'),
                      trailing: const Icon(Icons.add),
                      onTap: () => _open(_addStarter(widget.store, s)),
                    ),
                  ),
                ),
            ]),
          ),
        )
      else if (notes.isEmpty)
        SliverToBoxAdapter(
          child: Padding(
            padding: const EdgeInsets.all(32),
            child: Text('No notes match', textAlign: TextAlign.center, style: t.textTheme.bodyMedium),
          ),
        )
      else
        SliverPadding(
          padding: const EdgeInsets.fromLTRB(16, 4, 16, 24),
          sliver: SliverList.separated(
            itemCount: notes.length,
            separatorBuilder: (_, _) => const SizedBox(height: 10),
            itemBuilder: (_, i) => _NoteCard(note: notes[i], onTap: () => _open(notes[i]), onLongPress: () => _menu(notes[i])),
          ),
        ),
    ]);
  }
}

void deleteWithUndo(BuildContext context, NotesStore store, Note n) {
  store.remove(n);
  ScaffoldMessenger.of(context).showSnackBar(SnackBar(
    content: Text('Deleted “${n.title.trim().isEmpty ? 'note' : n.title.trim()}”'),
    action: SnackBarAction(label: 'Undo', onPressed: () => store.restore(n)),
  ));
}

class _StartCard extends StatelessWidget {
  const _StartCard({required this.icon, required this.label, required this.onTap});
  final IconData icon;
  final String label;
  final VoidCallback onTap;
  @override
  Widget build(BuildContext context) {
    final t = Theme.of(context);
    return Card(
      child: InkWell(
        onTap: onTap,
        child: Padding(
          padding: const EdgeInsets.symmetric(vertical: 22),
          child: Column(children: [
            Icon(icon, size: 30, color: t.colorScheme.primary),
            const SizedBox(height: 8),
            Text(label, style: t.textTheme.titleSmall),
          ]),
        ),
      ),
    );
  }
}

class _NoteCard extends StatelessWidget {
  const _NoteCard({required this.note, required this.onTap, required this.onLongPress});
  final Note note;
  final VoidCallback onTap, onLongPress;

  @override
  Widget build(BuildContext context) {
    final t = Theme.of(context);
    final muted = t.colorScheme.onSurfaceVariant;
    final items = (note.items ?? []).where((i) => i.text.trim().isNotEmpty).toList();
    final done = items.where((i) => i.done).length;
    final open = items.where((i) => !i.done).toList();
    final title = note.title.trim().isNotEmpty
        ? note.title.trim()
        : note.isChecklist
            ? 'Checklist'
            : note.text.trim().split('\n').first;
    final body = note.isChecklist ? '' : (note.title.trim().isEmpty ? note.text.trim().split('\n').skip(1).join('\n') : note.text.trim());
    return Card(
      child: InkWell(
        onTap: onTap,
        onLongPress: onLongPress,
        child: Padding(
          padding: const EdgeInsets.fromLTRB(16, 14, 16, 14),
          child: Column(crossAxisAlignment: CrossAxisAlignment.start, children: [
            Row(children: [
              if (note.pinned) Padding(padding: const EdgeInsets.only(right: 6), child: Icon(Icons.push_pin, size: 16, color: t.colorScheme.primary)),
              if (note.isChecklist) Padding(padding: const EdgeInsets.only(right: 6), child: Icon(Icons.checklist, size: 18, color: muted)),
              Expanded(child: Text(title, maxLines: 1, overflow: TextOverflow.ellipsis, style: t.textTheme.titleMedium)),
              const SizedBox(width: 8),
              Text(_when(note.updated), style: t.textTheme.bodySmall?.copyWith(color: muted)),
            ]),
            if (body.isNotEmpty) ...[
              const SizedBox(height: 6),
              Text(body, maxLines: 4, overflow: TextOverflow.ellipsis, style: t.textTheme.bodyMedium?.copyWith(color: muted)),
            ],
            if (note.isChecklist && items.isNotEmpty) ...[
              const SizedBox(height: 8),
              for (final i in open.take(3))
                Padding(
                  padding: const EdgeInsets.only(bottom: 3),
                  child: Row(children: [
                    Icon(Icons.check_box_outline_blank, size: 16, color: muted),
                    const SizedBox(width: 8),
                    Expanded(child: Text(i.text, maxLines: 1, overflow: TextOverflow.ellipsis, style: t.textTheme.bodyMedium)),
                  ]),
                ),
              const SizedBox(height: 4),
              Row(children: [
                Expanded(
                  child: ClipRRect(
                    borderRadius: BorderRadius.circular(4),
                    child: LinearProgressIndicator(
                      value: done / items.length,
                      minHeight: 4,
                      backgroundColor: t.colorScheme.outlineVariant,
                    ),
                  ),
                ),
                const SizedBox(width: 10),
                Text('$done of ${items.length} done', style: t.textTheme.bodySmall?.copyWith(color: muted)),
              ]),
            ],
          ]),
        ),
      ),
    );
  }
}

class _Row {
  _Row(CheckItem i)
      : item = i,
        c = TextEditingController(text: i.text);
  final CheckItem item;
  final TextEditingController c;
  final FocusNode f = FocusNode();
}

/// Editing one note. Changes are saved as you type; an empty note is dropped when you leave.
class NoteEditor extends StatefulWidget {
  const NoteEditor({super.key, required this.store, required this.note});
  final NotesStore store;
  final Note note;
  @override
  State<NoteEditor> createState() => _NoteEditorState();
}

class _NoteEditorState extends State<NoteEditor> with WidgetsBindingObserver {
  Note get n => widget.note;
  late final title = TextEditingController(text: n.title);
  late final text = TextEditingController(text: n.text);
  final textFocus = FocusNode();
  List<_Row> rows = [];

  @override
  void initState() {
    super.initState();
    WidgetsBinding.instance.addObserver(this);
    _buildRows();
    // a new note opens ready to type
    if (n.isEmpty) {
      WidgetsBinding.instance.addPostFrameCallback((_) {
        if (!mounted) return;
        if (n.isChecklist && rows.isNotEmpty) {
          rows.first.f.requestFocus();
        } else {
          textFocus.requestFocus();
        }
      });
    }
  }

  void _buildRows() {
    for (final r in rows) {
      r.c.dispose();
      r.f.dispose();
    }
    rows = [for (final i in n.items ?? <CheckItem>[]) _row(i)];
  }

  _Row _row(CheckItem i) {
    final r = _Row(i);
    r.f.addListener(() {
      if (mounted) setState(() {}); // shows the remove button on the focused item
    });
    return r;
  }

  @override
  void didChangeAppLifecycleState(AppLifecycleState state) {
    if (state != AppLifecycleState.resumed) widget.store.flush();
  }

  @override
  void dispose() {
    WidgetsBinding.instance.removeObserver(this);
    if (n.isEmpty) {
      final store = widget.store, note = n;
      Future.microtask(() => store.remove(note)); // not while the tree is locked
    } else {
      widget.store.flush();
    }
    title.dispose();
    text.dispose();
    textFocus.dispose();
    for (final r in rows) {
      r.c.dispose();
      r.f.dispose();
    }
    super.dispose();
  }

  void _save() => widget.store.changed(n);

  void _syncItems() => n.items = [for (final r in rows) r.item];

  void _insertAfter(_Row? after, {String text = ''}) {
    final r = _row(CheckItem(text));
    setState(() {
      final at = after == null ? rows.length : rows.indexOf(after) + 1;
      rows.insert(at, r);
      _syncItems();
    });
    _save();
    WidgetsBinding.instance.addPostFrameCallback((_) => r.f.requestFocus());
  }

  void _removeRow(_Row r) {
    final i = rows.indexOf(r);
    setState(() {
      rows.remove(r);
      _syncItems();
    });
    _save();
    // keep typing in the item above
    final open = rows.where((x) => !x.item.done).toList();
    if (open.isNotEmpty) open[(i - 1).clamp(0, open.length - 1)].f.requestFocus();
    WidgetsBinding.instance.addPostFrameCallback((_) {
      r.c.dispose();
      r.f.dispose();
    });
  }

  void _toggle(_Row r, bool done) {
    setState(() => r.item.done = done);
    _save();
  }

  void _convert() {
    setState(() {
      if (n.isChecklist) {
        n.text = [for (final i in n.items!) if (i.text.trim().isNotEmpty) i.text.trim()].join('\n');
        n.items = null;
        text.text = n.text;
      } else {
        n.items = [for (final l in n.text.split('\n')) if (l.trim().isNotEmpty) CheckItem(l.trim())];
        if (n.items!.isEmpty) n.items!.add(CheckItem(''));
        n.text = '';
      }
      _buildRows();
    });
    _save();
  }

  void _menu() {
    final anyDone = (n.items ?? []).any((i) => i.done);
    showModalBottomSheet<void>(
      context: context,
      showDragHandle: true,
      builder: (ctx) => SafeArea(
        child: Column(mainAxisSize: MainAxisSize.min, children: [
          if (n.isChecklist && anyDone)
            ListTile(
              leading: const Icon(Icons.restart_alt),
              title: const Text('Untick everything'),
              subtitle: const Text('Use the list again'),
              onTap: () {
                Navigator.pop(ctx);
                setState(() {
                  for (final i in n.items!) {
                    i.done = false;
                  }
                });
                _save();
              },
            ),
          if (n.isChecklist && anyDone)
            ListTile(
              leading: const Icon(Icons.playlist_remove),
              title: const Text('Remove ticked items'),
              onTap: () {
                Navigator.pop(ctx);
                setState(() {
                  rows.removeWhere((r) => r.item.done);
                  _syncItems();
                });
                _save();
              },
            ),
          ListTile(
            leading: Icon(n.isChecklist ? Icons.notes_outlined : Icons.checklist_outlined),
            title: Text(n.isChecklist ? 'Turn into a plain note' : 'Turn into a checklist'),
            subtitle: Text(n.isChecklist ? 'One line per item' : 'Each line becomes an item'),
            onTap: () {
              Navigator.pop(ctx);
              _convert();
            },
          ),
          ListTile(
            leading: const Icon(Icons.copy_outlined),
            title: const Text('Copy as text'),
            onTap: () {
              Navigator.pop(ctx);
              Clipboard.setData(ClipboardData(text: n.plain));
              ScaffoldMessenger.of(context).showSnackBar(const SnackBar(content: Text('Copied')));
            },
          ),
          ListTile(
            leading: const Icon(Icons.delete_outline),
            title: const Text('Delete'),
            onTap: () {
              Navigator.pop(ctx);
              final messenger = ScaffoldMessenger.of(context);
              Navigator.pop(context);
              if (!n.isEmpty) {
                widget.store.remove(n);
                messenger.showSnackBar(SnackBar(
                  content: const Text('Note deleted'),
                  action: SnackBarAction(label: 'Undo', onPressed: () => widget.store.restore(n)),
                ));
              }
            },
          ),
        ]),
      ),
    );
  }

  Widget _itemRow(_Row r) {
    final t = Theme.of(context);
    final done = r.item.done;
    return Row(crossAxisAlignment: CrossAxisAlignment.start, children: [
      Checkbox(value: done, onChanged: (v) => _toggle(r, v ?? false)),
      Expanded(
        child: TextField(
          controller: r.c,
          focusNode: r.f,
          maxLines: null,
          textCapitalization: TextCapitalization.sentences,
          textInputAction: TextInputAction.next,
          style: t.textTheme.bodyLarge?.copyWith(
            decoration: done ? TextDecoration.lineThrough : null,
            color: done ? t.colorScheme.onSurfaceVariant : null,
          ),
          decoration: const InputDecoration(
            border: InputBorder.none,
            enabledBorder: InputBorder.none,
            focusedBorder: InputBorder.none,
            filled: false,
            hintText: 'Item',
            contentPadding: EdgeInsets.symmetric(vertical: 12),
          ),
          onChanged: (v) {
            r.item.text = v;
            setState(() {}); // phone numbers
            _save();
          },
          onSubmitted: (_) => _insertAfter(r),
        ),
      ),
      if (r.f.hasFocus || done)
        IconButton(
          tooltip: 'Remove item',
          icon: const Icon(Icons.close, size: 20),
          onPressed: () => _removeRow(r),
        )
      else
        const SizedBox(width: 48),
    ]);
  }

  @override
  Widget build(BuildContext context) {
    final t = Theme.of(context);
    const plain = InputDecoration(
      border: InputBorder.none,
      enabledBorder: InputBorder.none,
      focusedBorder: InputBorder.none,
      filled: false,
      contentPadding: EdgeInsets.zero,
    );
    final open = rows.where((r) => !r.item.done).toList();
    final ticked = rows.where((r) => r.item.done).toList();
    final numbers = phoneNumbers('${n.title}\n${n.isChecklist ? (n.items ?? []).map((i) => i.text).join('\n') : n.text}');
    return Scaffold(
      appBar: AppBar(
        backgroundColor: t.colorScheme.surface,
        surfaceTintColor: Colors.transparent,
        actions: [
          IconButton(
            tooltip: n.pinned ? 'Unpin' : 'Pin to the top',
            icon: Icon(n.pinned ? Icons.push_pin : Icons.push_pin_outlined),
            onPressed: () {
              setState(() => n.pinned = !n.pinned);
              widget.store.changed(n, touch: false);
            },
          ),
          IconButton(tooltip: 'More', icon: const Icon(Icons.more_vert), onPressed: _menu),
        ],
      ),
      body: SafeArea(
        child: ListView(padding: const EdgeInsets.fromLTRB(20, 0, 12, 32), children: [
          Padding(
            padding: const EdgeInsets.only(right: 8),
            child: TextField(
              controller: title,
              style: t.textTheme.headlineSmall?.copyWith(fontWeight: FontWeight.w700),
              textCapitalization: TextCapitalization.sentences,
              decoration: plain.copyWith(hintText: 'Title'),
              onChanged: (v) {
                n.title = v;
                _save();
              },
            ),
          ),
          Padding(
            padding: const EdgeInsets.only(top: 6, bottom: 12),
            child: Text('Changed ${_when(n.updated)}', style: t.textTheme.bodySmall?.copyWith(color: t.colorScheme.onSurfaceVariant)),
          ),
          if (numbers.isNotEmpty)
            Padding(
              padding: const EdgeInsets.only(bottom: 12, right: 8),
              child: Wrap(spacing: 8, runSpacing: 8, children: [
                for (final p in numbers)
                  ActionChip(
                    avatar: const Icon(Icons.call_outlined, size: 18),
                    label: Text(p),
                    onPressed: () => numberMenu(context, p),
                  ),
              ]),
            ),
          if (!n.isChecklist)
            Padding(
              padding: const EdgeInsets.only(right: 8),
              child: TextField(
                controller: text,
                focusNode: textFocus,
                maxLines: null,
                minLines: 12,
                keyboardType: TextInputType.multiline,
                textCapitalization: TextCapitalization.sentences,
                style: t.textTheme.bodyLarge?.copyWith(height: 1.5),
                decoration: plain.copyWith(hintText: 'Write here — phone numbers, where things are, what to do…'),
                onChanged: (v) {
                  n.text = v;
                  setState(() {}); // phone numbers
                  _save();
                },
              ),
            )
          else ...[
            Transform.translate(
              offset: const Offset(-12, 0),
              child: Column(children: [for (final r in open) _itemRow(r)]),
            ),
            Align(
              alignment: Alignment.centerLeft,
              child: TextButton.icon(
                onPressed: () => _insertAfter(open.isEmpty ? null : open.last),
                icon: const Icon(Icons.add),
                label: const Text('Add item'),
              ),
            ),
            if (ticked.isNotEmpty) ...[
              Padding(
                padding: const EdgeInsets.only(top: 16, bottom: 4),
                child: Text('TICKED · ${ticked.length}', style: t.textTheme.labelSmall?.copyWith(color: t.colorScheme.onSurfaceVariant)),
              ),
              Transform.translate(
                offset: const Offset(-12, 0),
                child: Column(children: [for (final r in ticked) _itemRow(r)]),
              ),
            ],
          ],
        ]),
      ),
    );
  }
}
