// Bindings to the Rust core (core/crates/sp-ffi): the embedded sp-server and BLAKE3 hashing.
// Calls that can take a while (server start loads the pack and the embedding model, hashing
// reads whole files) run in a background isolate.

import 'dart:convert';
import 'dart:ffi';
import 'dart:isolate';

import 'package:ffi/ffi.dart';

typedef _StrFn = Pointer<Utf8> Function(Pointer<Utf8>);
typedef _VoidFn = Void Function();
typedef _VoidDart = void Function();
typedef _NoArgStr = Pointer<Utf8> Function();
typedef _ShareC = Pointer<Utf8> Function(Uint16);
typedef _ShareDart = Pointer<Utf8> Function(int);
typedef _FreeC = Void Function(Pointer<Utf8>);
typedef _FreeDart = void Function(Pointer<Utf8>);

class _Lib {
  _Lib._(DynamicLibrary l)
      : start = l.lookupFunction<_StrFn, _StrFn>('sp_start'),
        stop = l.lookupFunction<_VoidFn, _VoidDart>('sp_stop'),
        share = l.lookupFunction<_ShareC, _ShareDart>('sp_share'),
        blake3 = l.lookupFunction<_StrFn, _StrFn>('sp_blake3'),
        version = l.lookupFunction<_NoArgStr, _NoArgStr>('sp_version'),
        free = l.lookupFunction<_FreeC, _FreeDart>('sp_free');

  static final _Lib instance = _Lib._(DynamicLibrary.open('libsp_ffi.so'));

  final _StrFn start;
  final _VoidDart stop;
  final _ShareDart share;
  final _StrFn blake3;
  final _NoArgStr version;
  final _FreeDart free;

  /// Calls a string -> JSON function and unwraps {"ok": …} / {"error": …}.
  dynamic call(_StrFn f, String arg) {
    final a = arg.toNativeUtf8();
    try {
      return _unwrap(f(a));
    } finally {
      malloc.free(a);
    }
  }

  dynamic _unwrap(Pointer<Utf8> p) {
    final s = p.toDartString();
    free(p);
    final j = jsonDecode(s) as Map<String, dynamic>;
    if (j.containsKey('error')) throw CoreError(j['error'] as String);
    return j['ok'];
  }
}

class CoreError implements Exception {
  CoreError(this.message);
  final String message;
  @override
  String toString() => message;
}

class Core {
  /// Starts (or restarts) the embedded server on 127.0.0.1. Returns its port.
  /// `packs`: the installed collection directories (one pack each; may be empty);
  /// `model`: the query embedding GGUF (without it search is keyword-only);
  /// `llm`: directory holding the chat model for answers (loaded on the first question);
  /// `games`: served at /games for people nearby (the app plays them itself); `chat`: chat's directory;
  /// `installed`: the content store's installed.json, whose books are offered to browsers at /books and
  /// everything to other phones running the app; `apk`: this app's APK, offered at /app.apk.
  static Future<int> start(
      {required List<String> packs, String? model, String? maps, String? llm, String? games, String? chat, String? installed, String? apk}) {
    final cfg = jsonEncode(
        {'packs': packs, 'model': model, 'maps': maps, 'llm': llm, 'games': games, 'chat': chat, 'installed': installed, 'apk': apk});
    return Isolate.run(() {
      final r = _Lib.instance.call(_Lib.instance.start, cfg) as Map<String, dynamic>;
      return r['port'] as int;
    });
  }

  static Future<void> stop() => Isolate.run(() => _Lib.instance.stop());

  /// Serves the running server on the network too (Wi-Fi, hotspot) at `port` or the next free port up to
  /// port+9, and keeps doing so across restarts; 0 stops. Returns the port in use (0 when stopped, or when
  /// the server isn't running yet: it opens on the next start).
  static Future<int> share(int port) => Isolate.run(() {
        final l = _Lib.instance;
        final r = l._unwrap(l.share(port)) as Map<String, dynamic>;
        return r['port'] as int;
      });

  /// BLAKE3 of a file, hex.
  static Future<String> blake3(String path) =>
      Isolate.run(() => _Lib.instance.call(_Lib.instance.blake3, path) as String);

  static String version() => _Lib.instance._unwrap(_Lib.instance.version()) as String;
}
