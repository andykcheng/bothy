// Progress of the background download queue (downloads.dart), with Pause/Resume and Cancel. Shown on
// the Content and Books tabs.

import 'package:flutter/material.dart';

import 'content.dart';
import 'downloads.dart';

class DownloadCard extends StatelessWidget {
  const DownloadCard({super.key, required this.downloads, this.onCancel});
  final Downloads downloads;
  final VoidCallback? onCancel;

  @override
  Widget build(BuildContext context) {
    final t = Theme.of(context);
    final dl = downloads;
    final frac = dl.total == 0 ? null : (dl.done / dl.total).clamp(0.0, 1.0);
    final left = dl.queue.length;
    final status = dl.running
        ? (dl.note ?? 'Keeps going in the background, even with the app closed.')
        : 'Paused. Picks up where it stopped.';
    return Padding(
      padding: const EdgeInsets.fromLTRB(16, 4, 16, 4),
      child: Card(
        child: Padding(
          padding: const EdgeInsets.fromLTRB(16, 12, 8, 8),
          child: Column(crossAxisAlignment: CrossAxisAlignment.start, children: [
            Text(dl.running ? 'Downloading ${dl.current ?? ''}' : 'Download paused', style: t.textTheme.titleSmall),
            const SizedBox(height: 8),
            Padding(
              padding: const EdgeInsets.only(right: 8),
              child: ClipRRect(
                borderRadius: BorderRadius.circular(99),
                child: LinearProgressIndicator(minHeight: 6, value: dl.running ? frac : (frac ?? 0)),
              ),
            ),
            const SizedBox(height: 6),
            Text('${formatBytes(dl.done)} of ${formatBytes(dl.total)} · $left item${left == 1 ? '' : 's'} left',
                style: t.textTheme.bodySmall),
            Text(status, style: t.textTheme.bodySmall?.copyWith(color: t.colorScheme.onSurfaceVariant)),
            Row(mainAxisAlignment: MainAxisAlignment.end, children: [
              TextButton(onPressed: onCancel ?? dl.cancel, child: const Text('Cancel')),
              const SizedBox(width: 4),
              dl.running
                  ? FilledButton.tonalIcon(onPressed: dl.pause, icon: const Icon(Icons.pause), label: const Text('Pause'))
                  : FilledButton.tonalIcon(onPressed: dl.resume, icon: const Icon(Icons.play_arrow), label: const Text('Resume')),
            ]),
          ]),
        ),
      ),
    );
  }
}
