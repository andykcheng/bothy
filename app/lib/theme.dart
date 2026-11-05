// "Calm outdoor" look: warm paper background, forest-green accent, slate dark mode, rounded cards, Inter.
// The hub's HTML pages (core/crates/sp-server/src/page.rs) use the same palette.

import 'package:flutter/material.dart';

class Palette {
  static const bg = Color(0xFFF6F4EF), surface = Color(0xFFFFFFFF), fg = Color(0xFF1E2421), muted = Color(0xFF5F6B64);
  static const accent = Color(0xFF2F6B4F), accentSoft = Color(0xFFE3EEE7), border = Color(0xFFE2DED5);
  static const darkBg = Color(0xFF151A1E), darkSurface = Color(0xFF1E252A), darkFg = Color(0xFFE6EAE7);
  static const darkMuted = Color(0xFF97A39C), darkAccent = Color(0xFF7CC4A0), darkAccentSoft = Color(0xFF22342C);
  static const darkBorder = Color(0xFF2C353B);
}

ThemeData buildTheme(Brightness b) {
  final dark = b == Brightness.dark;
  final scheme = ColorScheme.fromSeed(seedColor: Palette.accent, brightness: b).copyWith(
    primary: dark ? Palette.darkAccent : Palette.accent,
    onPrimary: dark ? const Color(0xFF0E1A14) : Colors.white,
    primaryContainer: dark ? Palette.darkAccentSoft : Palette.accentSoft,
    onPrimaryContainer: dark ? Palette.darkAccent : Palette.accent,
    secondaryContainer: dark ? Palette.darkAccentSoft : Palette.accentSoft,
    onSecondaryContainer: dark ? Palette.darkAccent : Palette.accent,
    surface: dark ? Palette.darkBg : Palette.bg,
    onSurface: dark ? Palette.darkFg : Palette.fg,
    onSurfaceVariant: dark ? Palette.darkMuted : Palette.muted,
    surfaceContainerLowest: dark ? Palette.darkSurface : Palette.surface,
    surfaceContainerLow: dark ? Palette.darkSurface : Palette.surface,
    surfaceContainer: dark ? Palette.darkSurface : Palette.surface,
    outlineVariant: dark ? Palette.darkBorder : Palette.border,
  );
  final radius = BorderRadius.circular(14);
  final base = ThemeData(colorScheme: scheme, useMaterial3: true, fontFamily: 'Inter');
  return base.copyWith(
    scaffoldBackgroundColor: scheme.surface,
    textTheme: base.textTheme.copyWith(
      headlineMedium: base.textTheme.headlineMedium?.copyWith(fontWeight: FontWeight.w700, letterSpacing: -0.4),
      titleLarge: base.textTheme.titleLarge?.copyWith(fontWeight: FontWeight.w700, letterSpacing: -0.2),
      titleMedium: base.textTheme.titleMedium?.copyWith(fontWeight: FontWeight.w600),
      titleSmall: base.textTheme.titleSmall?.copyWith(fontWeight: FontWeight.w600),
      labelSmall: base.textTheme.labelSmall?.copyWith(fontWeight: FontWeight.w600, letterSpacing: 0.8),
    ),
    cardTheme: CardThemeData(
      color: scheme.surfaceContainerLowest,
      elevation: 0,
      margin: EdgeInsets.zero,
      shape: RoundedRectangleBorder(borderRadius: radius, side: BorderSide(color: scheme.outlineVariant)),
      clipBehavior: Clip.antiAlias,
    ),
    navigationBarTheme: NavigationBarThemeData(
      backgroundColor: dark ? Palette.darkSurface : Palette.surface,
      indicatorColor: scheme.primaryContainer,
      surfaceTintColor: Colors.transparent,
      elevation: 0,
      labelTextStyle: WidgetStatePropertyAll(base.textTheme.labelMedium?.copyWith(fontWeight: FontWeight.w600)),
    ),
    inputDecorationTheme: InputDecorationTheme(
      filled: true,
      fillColor: scheme.surfaceContainerLowest,
      isDense: true,
      border: OutlineInputBorder(borderRadius: BorderRadius.circular(999), borderSide: BorderSide(color: scheme.outlineVariant)),
      enabledBorder: OutlineInputBorder(borderRadius: BorderRadius.circular(999), borderSide: BorderSide(color: scheme.outlineVariant)),
      contentPadding: const EdgeInsets.symmetric(horizontal: 16, vertical: 12),
    ),
    filledButtonTheme: FilledButtonThemeData(
      style: FilledButton.styleFrom(
        shape: const StadiumBorder(),
        padding: const EdgeInsets.symmetric(horizontal: 20, vertical: 14),
        textStyle: const TextStyle(fontFamily: 'Inter', fontWeight: FontWeight.w600, fontSize: 15),
      ),
    ),
    outlinedButtonTheme: OutlinedButtonThemeData(
      style: OutlinedButton.styleFrom(shape: const StadiumBorder(), side: BorderSide(color: scheme.outlineVariant)),
    ),
    snackBarTheme: const SnackBarThemeData(behavior: SnackBarBehavior.floating),
    dividerTheme: DividerThemeData(color: scheme.outlineVariant, space: 1),
  );
}

/// Small uppercase section label ("LIBRARY", "MAPS"…).
class SectionLabel extends StatelessWidget {
  const SectionLabel(this.text, {super.key, this.trailing});
  final String text;
  final Widget? trailing;
  @override
  Widget build(BuildContext context) {
    final t = Theme.of(context);
    return Padding(
      padding: const EdgeInsets.fromLTRB(20, 24, 20, 8),
      child: Row(children: [
        Expanded(
          child: Text(text.toUpperCase(), style: t.textTheme.labelSmall?.copyWith(color: t.colorScheme.onSurfaceVariant)),
        ),
        ?trailing,
      ]),
    );
  }
}
